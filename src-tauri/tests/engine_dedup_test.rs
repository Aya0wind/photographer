//! 引擎查重分层：宽松键预判、(size, xxhash) 精确复核、目标路径冲突策略
//! （Rename 生成 `_1` / Skip 判 skipped）、二次导入全跳过。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;

use common::{
    build_source, count_assets, expected_ungrouped_dir, journal_states, open_db, run_engine,
};
use events::FileState;
use settings::DuplicatePolicy;

#[test]
fn second_import_all_skipped() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());

    let (_, first) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(first.done_files, 3);
    let (_, second) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert_eq!(second.done_files, 0);
    assert_eq!(second.skipped_duplicates, 3);
    assert_eq!(second.failed_files, 0);
    let db = open_db(db_dir.path());
    assert_eq!(count_assets(&db), 3, "二次导入不得新增资产");
}

#[test]
fn rename_policy_generates_suffix() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    // 预占一个目标路径（内容不同，跳过内容查重干扰：skip_imported=false）
    let jpg = &files[0];
    let occupied = expected_ungrouped_dir(db_dir.path(), target.path()).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate_policy = DuplicatePolicy::Rename;
        plan.skip_imported = false;
    });

    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.skipped_duplicates, 0);
    assert_eq!(fs::read(&occupied).unwrap(), b"occupied");
    let renamed = occupied.with_file_name("IMG_0001_1.jpg");
    assert_eq!(
        fs::read(&renamed).unwrap(),
        jpg.1,
        "Rename 策略应生成 _1 副本"
    );
}

#[test]
fn skip_policy_path_conflict_marks_skipped() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path()); // 建源树（期望路径不再依赖逐文件 mtime）

    let occupied = expected_ungrouped_dir(db_dir.path(), target.path()).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate_policy = DuplicatePolicy::Skip;
        plan.skip_imported = false;
    });

    assert_eq!(stats.done_files, 2);
    assert_eq!(stats.skipped_duplicates, 1);
    let states = journal_states(&open_db(db_dir.path()), job_id);
    assert_eq!(
        states.iter().filter(|s| **s == FileState::Skipped).count(),
        1
    );
    // 原占用文件不被覆盖
    assert_eq!(fs::read(&occupied).unwrap(), b"occupied");
}
