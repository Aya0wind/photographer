//! 引擎流水线（单遍复制）：字节级完整性、哈希/journal/资产一致、
//! 里程碑序列、进度节流、模板降级、`.part` 无残留、ImportPlan 契约。

mod common;

pub use common::{db, devices, events, import, metadata, settings};

use std::fs;

use common::{
    build_many, build_source, count_assets, expected_subdir, find_part_files, open_db, plan_for,
    run_engine, sha256_of, shrink,
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
        let dst = target
            .path()
            .join(expected_subdir(src.path(), rel))
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
        let expected_sha = sha256_of(content);
        assert_eq!(
            row.sha256.as_ref().map(|s| s.as_slice()),
            Some(expected_sha.as_slice())
        );
        assert_eq!(
            row.dst.replace('\\', "/"),
            dst.to_string_lossy().replace('\\', "/")
        );
    }
    assert_eq!(count_assets(&db), 3);
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
    // 追加第 4 个等大文件 → 每文件 25%
    let extra = shrink(b"\0\0\0\x18ftypisom\x00\x00".to_vec(), 4096);
    fs::write(src.path().join("DCIM/100CANON/MVI_0004.MOV"), &extra).unwrap();

    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let db = open_db(db_dir.path());
    let engine = Engine::new(
        db,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
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
    let engine = Engine::new(
        db,
        bus,
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
    );
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
fn camera_template_falls_back_for_exifless_files() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // 无 EXIF 相机字段的 NEF（TIFF 头但空 IFD）
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    let nef = shrink(b"II*\0\x00\x00\x00\x08\x00\x00".to_vec(), 1024);
    fs::write(src.path().join("DCIM/DSC_0001.NEF"), &nef).unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.dir_template = "{YYYY}/{相机}/{MM-DD}".into();
    });

    assert_eq!(stats.done_files, 1);
    // 期望落在 .../<年>/未知相机/<月-日>/DSC_0001.NEF
    let mtime: chrono::DateTime<chrono::Utc> = fs::metadata(src.path().join("DCIM/DSC_0001.NEF"))
        .unwrap()
        .modified()
        .unwrap()
        .into();
    let expected = target
        .path()
        .join(mtime.format("%Y").to_string())
        .join("未知相机")
        .join(mtime.format("%m-%d").to_string())
        .join("DSC_0001.NEF");
    assert!(
        expected.exists(),
        "应降级到 未知相机 目录: {}",
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
