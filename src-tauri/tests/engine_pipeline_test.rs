//! 引擎流水线（单遍复制）：字节级完整性、哈希/journal/资产一致、
//! 里程碑序列、进度节流、模板降级、`.part` 无残留、ImportPlan 契约。

mod common;

use xxhash_rust::xxh64::xxh64;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;

use common::{
    build_many, build_source, count_assets, expected_ungrouped_dir, find_part_files, open_db,
    plan_for,
    run_engine, shrink,
};
use devices::volume::VolumeSource;
use events::{AppEvent, EventBus, FileState};
use import::engine::{Engine, ImportMode, ImportPlan};

#[test]
fn copies_files_with_byte_and_hash_integrity() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());
    fs::write(src.path().join("DCIM/100CANON/MVI_0003.MP4"), b"video").unwrap();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert_eq!(stats.total_files, 3);
    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.skipped_duplicates, 0);
    assert_eq!(stats.failed_files, 0);
    assert_eq!(
        stats.total_bytes,
        files.iter().map(|(_, c)| c.len() as u64).sum::<u64>()
    );

    // 字节级比对 + 目标路径按模板落位
    let db = open_db(db_dir.path());
    for (rel, content) in &files {
        let dst = expected_ungrouped_dir(db_dir.path(), target.path())
            .join(rel.rsplit('/').next().unwrap());
        assert_eq!(fs::read(&dst).unwrap(), *content, "字节不一致: {rel}");
        // journal + assets 哈希一致
        let row = db
            .all_job_files(job_id)
            .unwrap()
            .into_iter()
            .find(|r| &r.src == rel)
            .expect("journal row");
        assert_eq!(row.state, FileState::Verified);
        assert_eq!(row.xxhash, Some(xxh64(content, 0))); // (size, xxhash) 复合键
        assert_eq!(
            row.dst.replace('\\', "/"),
            dst.to_string_lossy().replace('\\', "/")
        );
    }
    assert_eq!(count_assets(&db), 3);
    assert!(db.asset_id_by_path("MVI_0003.MP4").unwrap().is_none());
    assert_eq!(
        db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "done"
    );
}

#[test]
fn milestones_fire_in_order() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    // 追加一个等大 JPEG，视频不参与；共四张图片，每文件 25%。
    let mut extra = shrink(vec![0xff, 0xd8, 0xff, 0xe0], 4096);
    extra[10] = 1;
    fs::write(src.path().join("DCIM/100CANON/IMG_0004.JPG"), &extra).unwrap();

    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let db = open_db(db_dir.path());
    let mut plan = plan_for(target.path());
    // 0018 导入必落相册：引擎兜底校验前先落「未分组」
    plan.album_id = Some(db.ensure_default_album().unwrap());
    let engine = Engine::new(
        db,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    let stats = engine.run();

    assert_eq!(stats.done_files, 4);
    let mut milestones = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::ImportMilestoneReached { percent, .. } = ev {
            milestones.push(percent);
        }
    }
    assert_eq!(milestones, vec![25, 50, 75, 100]);
}

#[test]
fn progress_events_are_throttled() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 40);

    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let db = open_db(db_dir.path());
    let mut plan = plan_for(target.path());
    plan.album_id = Some(db.ensure_default_album().unwrap());
    let engine = Engine::new(db, bus, Box::new(VolumeSource::new(src.path())), plan);
    let stats = engine.run();
    assert_eq!(stats.done_files, 40);

    let mut progress_count = 0;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AppEvent::ImportFileProgress { .. }) {
            progress_count += 1;
        }
    }
    assert!(progress_count >= 1, "首个进度必须立即发出");
    assert!(
        progress_count < 40,
        "40 个小文件不应产生 40 次进度（节流失效）: {progress_count}"
    );
}

#[test]
fn exifless_file_lands_flat_in_album_home() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // 无 EXIF 相机字段的 NEF（TIFF 头但空 IFD）
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    let nef = shrink(b"II*\0\x00\x00\x00\x08\x00\x00".to_vec(), 1024);
    fs::write(src.path().join("DCIM/DSC_0001.NEF"), &nef).unwrap();
    // 先 ensure 兜底相册以固定创建时刻（run_engine 幂等复用同一条）
    let home = expected_ungrouped_dir(db_dir.path(), target.path());

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert_eq!(stats.done_files, 1);
    // 布局固定（2026-09-28）：无拍摄 EXIF 也照常落相册主目录平铺
    // （目录段为相册级字面量，与逐照片 EXIF 无关）
    let expected = home.join("DSC_0001.NEF");
    assert!(
        expected.exists(),
        "应落相册主目录平铺: {}",
        expected.display()
    );
    assert_eq!(fs::read(&expected).unwrap(), nef);
}

#[test]
fn no_part_residue_after_success() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());

    run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert!(!target.path().join(".smartphoto-part").exists());
    assert!(find_part_files(target.path()).is_empty());
}

#[test]
fn plan_mode_defaults_to_copy_and_round_trips() {
    // 旧版 plan JSON（无 mode 字段）→ copy（journal 里的历史计划可恢复）
    let json = r#"{
        "sourceId": "FOLDER:C:\\photos-in",
        "targetRoot": "C:\\vault",
        "dirTemplate": "{YYYY}/{MM-DD}",
        "nameTemplate": "{原文件名}",
        "duplicatePolicy": "skip",
        "skipImported": true,
        "streams": 2
    }"#;
    let plan: ImportPlan = serde_json::from_str(json).unwrap();
    assert_eq!(plan.mode, ImportMode::Copy);

    // 显式 move 序列化为 "move"；copy 序列化为 "copy"（小写）
    let mut moved = plan.clone();
    moved.mode = ImportMode::Move;
    let value = serde_json::to_value(&moved).unwrap();
    assert_eq!(value["mode"], "move");
    assert_eq!(value["targetRoot"], "C:\\vault", "字段保持 camelCase");
    assert_eq!(serde_json::to_value(&plan).unwrap()["mode"], "copy");
    let back: ImportPlan = serde_json::from_value(value).unwrap();
    assert_eq!(back.mode, ImportMode::Move);
}

// ---------------------------------------------------------------------------
// M2：plan.include 文件筛选（向导勾选）
// ---------------------------------------------------------------------------

#[test]
fn include_filters_queue_to_selected_files() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path()); // 三张图片 + 一个应忽略的视频
    fs::write(src.path().join("DCIM/100CANON/MVI_0003.MP4"), b"video").unwrap();

    // 即使旧计划勾选了视频，也只导入照片。
    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.include = Some(vec![
            "DCIM/100CANON/IMG_0001.jpg".into(),
            "DCIM/100CANON/MVI_0003.MP4".into(),
        ]);
    });

    assert_eq!(stats.total_files, 1, "只统计可导入图片: {stats:?}");
    assert_eq!(stats.done_files, 1);
    assert_eq!(stats.failed_files, 0);

    let db = open_db(db_dir.path());
    // journal 只有照片一行且 verified。
    let rows = db.all_job_files(job_id).unwrap();
    assert_eq!(rows.len(), 1, "视频和未勾选文件不得进 journal: {rows:?}");
    assert!(rows.iter().all(|r| r.state == FileState::Verified));
    assert!(rows.iter().any(|r| r.src.ends_with("IMG_0001.jpg")));
    assert!(!rows.iter().any(|r| r.src.ends_with("MVI_0003.MP4")));
    // assets 只有照片；视频与未勾选的 RAW 不落位。
    assert_eq!(count_assets(&db), 1);
    for (rel, content) in &files {
        let dst = expected_ungrouped_dir(db_dir.path(), target.path())
            .join(rel.rsplit('/').next().unwrap());
        if !rel.ends_with("IMG_0001.jpg") {
            assert!(!dst.exists(), "未勾选文件不得导入: {rel}");
        } else {
            assert_eq!(fs::read(&dst).unwrap(), *content, "勾选文件正常导入: {rel}");
        }
    }
}

#[test]
fn include_with_empty_intersection_is_invalid_plan() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());

    let db = open_db(db_dir.path());
    let mut plan = plan_for(target.path());
    plan.include = Some(vec!["DCIM/不存在.jpg".into()]);
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    let err = engine.begin().expect_err("勾选与源无交集应拒绝");
    assert!(err.to_string().contains("所选文件均不在源中"), "{err}");

    // 拒绝时不建任务
    let db = open_db(db_dir.path());
    let jobs: i64 =
        db.0.query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
            .unwrap();
    assert_eq!(jobs, 0);
}

#[test]
fn include_none_and_serde_round_trip() {
    // 缺省 None = 全量导入（回归：现有全部用例走此路径）
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        assert_eq!(plan.include, None);
        plan.include = None; // 显式 None 与缺省同义
    });
    assert_eq!(stats.done_files, 3, "None 导入全部图片: {stats:?}");

    // 旧 journal plan JSON（无 include 字段）→ None；Some 列表 round-trip
    let legacy = r#"{
        "sourceId": "FOLDER:C:\\photos-in",
        "targetRoot": "C:\\vault",
        "dirTemplate": "{YYYY}/{MM-DD}",
        "nameTemplate": "{原文件名}",
        "duplicatePolicy": "skip",
        "skipImported": true,
        "streams": 2
    }"#;
    let plan: ImportPlan = serde_json::from_str(legacy).unwrap();
    assert_eq!(plan.include, None);
    let mut partial = plan.clone();
    partial.include = Some(vec!["DCIM/A.jpg".into(), "DCIM/B.NEF".into()]);
    let value = serde_json::to_value(&partial).unwrap();
    assert_eq!(
        value["include"],
        serde_json::json!(["DCIM/A.jpg", "DCIM/B.NEF"]),
        "include 键 camelCase 透传 rel_path 列表"
    );
    let back: ImportPlan = serde_json::from_value(value).unwrap();
    assert_eq!(back.include, partial.include);
}

#[test]
fn import_extracts_shooting_params_into_assets() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // 单文件源：一张带完整拍摄参数的 JPEG fixture
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    let jpeg = common::build_exif_jpeg();
    fs::write(src.path().join("DCIM").join("IMG_0001.jpg"), &jpeg).unwrap();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 1);
    assert_eq!(stats.failed_files, 0);

    let db = open_db(db_dir.path());
    type Params7 = (
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let (width, height, iso, f_number, exposure_time, focal_length, lens): Params7 = db
        .0
        .query_row(
            "SELECT width, height, iso, f_number, exposure_time, focal_length, lens              FROM assets WHERE kind = 'photo'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?)),
        )
        .unwrap();
    assert_eq!((width, height), (Some(6048), Some(8064)), "宽高来自 SOF0");
    assert_eq!(iso, Some(1600));
    assert_eq!(f_number.as_deref(), Some("2.8"));
    assert_eq!(exposure_time.as_deref(), Some("1/250"));
    assert_eq!(focal_length.as_deref(), Some("85"));
    assert_eq!(lens.as_deref(), Some("FE 85mm F1.8"));
    // journal 仍 verified（提取失败不阻塞导入路径）
    let state: String =
        db.0.query_row(
            "SELECT state FROM job_files WHERE job_id = ?1",
            [job_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "verified");
}
