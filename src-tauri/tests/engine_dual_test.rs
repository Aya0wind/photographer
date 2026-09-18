//! F2 双目的地导入（secondTarget）：单遍双写双校验、journal 双记录
//! （dst+dst2）、第二目的地失败判单文件失败且主路回滚、move 互斥拒绝、
//! 历史计划 JSON 兼容。

mod common;

pub use common::{db, devices, events, import, metadata, settings, tasks, thumbs};

use std::fs;
use std::path::{Path, PathBuf};

use common::{
    build_source, count_assets, find_part_files, journal_states, open_db, plan_for, run_engine,
};
use devices::volume::VolumeSource;
use events::{EventBus, FileState};
use import::engine::{Engine, ImportMode, ImportPlan, SecondTarget};

fn second_for(target: &Path) -> SecondTarget {
    SecondTarget {
        target_root: target.to_path_buf(),
        dir_template: "{YYYY}/backup".into(),
    }
}

/// 第二目的地的期望路径（模板 {YYYY}/backup；无 EXIF 回退 mtime）。
fn expected_second_dst(second_root: &Path, source_dir: &Path, rel: &str) -> PathBuf {
    let mtime = fs::metadata(source_dir.join(rel.replace('/', "\\")))
        .unwrap()
        .modified()
        .unwrap();
    let t: chrono::DateTime<chrono::Utc> = mtime.into();
    second_root
        .join(t.format("%Y").to_string())
        .join("backup")
        .join(rel.rsplit('/').next().unwrap())
}

#[test]
fn dual_target_writes_both_destinations_with_integrity() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.second_target = Some(second_for(second.path()));
    });

    assert_eq!(stats.done_files, 3, "双目的地全部成功: {stats:?}");
    assert_eq!(stats.failed_files, 0);

    let db = open_db(db_dir.path());
    for (rel, content) in &files {
        let name = rel.rsplit('/').next().unwrap();
        // 主目的地字节一致
        let primary = target
            .path()
            .join(common::expected_subdir(src.path(), rel))
            .join(name);
        assert_eq!(fs::read(&primary).unwrap(), *content, "主目的地: {rel}");
        // 第二目的地字节一致（独立目录模板）
        let secondary = expected_second_dst(second.path(), src.path(), rel);
        assert_eq!(fs::read(&secondary).unwrap(), *content, "第二目的地: {rel}");
        // journal 双记录：dst + dst2 均落位
        let row = db
            .all_job_files(job_id)
            .unwrap()
            .into_iter()
            .find(|r| &r.src == rel)
            .expect("journal row");
        assert_eq!(row.state, FileState::Verified);
        assert_eq!(
            row.dst.replace('\\', "/"),
            primary.to_string_lossy().replace('\\', "/")
        );
        assert_eq!(
            row.dst2.replace('\\', "/"),
            secondary.to_string_lossy().replace('\\', "/"),
            "journal 必须双记录第二目的地"
        );
    }

    // assets 只入册主目的地（库资产 = 主拷贝）
    assert_eq!(count_assets(&db), 3);
    let second_rows: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE path LIKE ?1",
            [format!("{}%", second.path().to_string_lossy()).as_str()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(second_rows, 0, "第二目的地不入 assets");

    // 两棵树均无 .part 残留
    assert!(find_part_files(target.path()).is_empty());
    assert!(find_part_files(second.path()).is_empty());

    // 二次导入：双目的地同样全跳过（不重复写）
    let (_, again) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.second_target = Some(second_for(second.path()));
    });
    assert_eq!(again.done_files, 0);
    assert_eq!(again.skipped_duplicates, 3);
    assert_eq!(count_assets(&open_db(db_dir.path())), 3);
}

#[test]
fn second_target_failure_fails_file_without_stray_primary() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    // 在第二目的地预置普通文件占据年份目录 → 主路落位后第二目的地
    // 建目录必然失败（走"任一失败判单文件失败 + 主路回滚"路径）
    for (rel, _) in &files {
        let year_dir = expected_second_dst(second.path(), src.path(), rel)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        assert!(
            fs::write(&year_dir, b"block").is_ok(),
            "占位失败: {}",
            year_dir.display()
        );
    }

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.second_target = Some(second_for(second.path()));
    });

    // 任一目的地失败 → 该文件整体判失败
    assert_eq!(stats.failed_files, 3, "{stats:?}");
    assert_eq!(stats.done_files, 0);
    assert_eq!(stats.skipped_duplicates, 0, "冲突策略不得把失败误判为跳过");
    let states = journal_states(&open_db(db_dir.path()), job_id);
    assert!(states.iter().all(|s| *s == FileState::Failed));

    // 主目的地不得残留半套拷贝（原子性：要么双落位要么全无）
    let primary_files: Vec<_> = walkdir::WalkDir::new(target.path())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect();
    assert!(
        primary_files.is_empty(),
        "第二目的地失败时主目的地不得留文件: {primary_files:?}"
    );
    // 两棵树均无 .part 残留
    assert!(find_part_files(target.path()).is_empty());
    assert!(find_part_files(second.path()).is_empty());
    // 源不动
    for (rel, content) in &files {
        assert_eq!(
            fs::read(src.path().join(rel.replace('/', "\\"))).unwrap(),
            *content
        );
    }
}

#[test]
fn move_plus_second_target_rejected_as_invalid_plan() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    build_source(src.path());

    let db = open_db(db_dir.path());
    let mut plan = plan_for(target.path());
    plan.mode = ImportMode::Move;
    plan.second_target = Some(second_for(second.path()));
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    let err = engine.begin().expect_err("move+secondTarget 必须拒绝");
    assert!(
        err.to_string().contains("双目的地"),
        "错误应说明双目的地限制: {err}"
    );

    // 拒绝时不建任务
    let db = open_db(db_dir.path());
    let jobs: i64 =
        db.0.query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
            .unwrap();
    assert_eq!(jobs, 0);
}

#[test]
fn second_target_serde_backward_compat_and_camel_case() {
    // 旧版 plan JSON（无 secondTarget）→ None
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
    assert_eq!(plan.second_target, None);
    assert_eq!(plan.mode, ImportMode::Copy);

    // Some 值 round-trip 且键为 camelCase secondTarget/targetRoot/dirTemplate
    let mut dual = plan.clone();
    dual.second_target = Some(SecondTarget {
        target_root: PathBuf::from(r"D:\backup"),
        dir_template: "{YYYY}/backup".into(),
    });
    let value = serde_json::to_value(&dual).unwrap();
    assert!(value.get("secondTarget").is_some(), "{value}");
    assert_eq!(value["secondTarget"]["targetRoot"], r"D:\backup");
    assert_eq!(value["secondTarget"]["dirTemplate"], "{YYYY}/backup");
    let back: ImportPlan = serde_json::from_value(value).unwrap();
    assert_eq!(back.second_target, dual.second_target);
}
