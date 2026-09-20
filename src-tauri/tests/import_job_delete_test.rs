//! import_job_delete IPC：终态任务三清（jobs/job_files/logs，级联验证）、
//! 进行中拒绝、不存在幂等。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::{open_db, run_engine};
use events::AppEvent;

#[test]
fn job_delete_clears_three_tables_for_terminal_job() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    common::build_source(src.path());
    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3);
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());

    // 造日志（历史的一部分）
    db.append_log("info", Some(job_id), "导入完成").unwrap();
    let counts = |db: &db::Db, job_id: i64| -> (i64, i64, i64) {
        let q = |sql: &str| -> i64 { db.0.query_row(sql, [job_id], |r| r.get(0)).unwrap() };
        (
            q("SELECT COUNT(*) FROM jobs WHERE id = ?1"),
            q("SELECT COUNT(*) FROM job_files WHERE job_id = ?1"),
            q("SELECT COUNT(*) FROM logs WHERE job_id = ?1"),
        )
    };
    assert_eq!(
        counts(&db, job_id),
        (1, 3, 3),
        "删除前：1 任务/3 journal/3 日志（引擎自记 2 条+补 1 条）"
    );

    // 删除（终态 done）
    ipc::import::fetch_import_job_delete(&state, job_id).unwrap();
    assert_eq!(
        counts(&db, job_id),
        (0, 0, 0),
        "三清：job_files 级联 + logs 显式删"
    );

    // 不存在：幂等 Ok
    ipc::import::fetch_import_job_delete(&state, job_id).unwrap();
    // 其他任务不受波及
    let (job_id2, _) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    let _ = job_id2;
}

#[test]
fn job_delete_rejects_running_and_paused() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    // 手造 running 任务（不走引擎后台，状态可控）
    let db = open_db(db_dir.path());
    let job_id = db
        .create_job_with_plan("copy", "test-src", "测试卡", 1, 100, "{}")
        .unwrap();
    db.upsert_job_file(&db::JobFileRow {
        job_id,
        src: "DCIM/IMG_0001.JPG".into(),
        dst: "X:/dst/IMG_0001.JPG".into(),
        size: 100,
        state: events::FileState::Pending,
        error: None,
        xxhash: None,
        dst2: String::new(),
    })
    .unwrap();
    db.append_log("info", Some(job_id), "开始").unwrap();

    // running → 拒绝
    let err = ipc::import::fetch_import_job_delete(&state, job_id).unwrap_err();
    assert_eq!(err, "任务进行中，无法删除");
    let n: i64 =
        db.0.query_row("SELECT COUNT(*) FROM jobs WHERE id = ?1", [job_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(n, 1, "running 行未动");

    // paused → 同样拒绝（非终态）
    db.finish_job(job_id, "paused", "{}").unwrap();
    let err = ipc::import::fetch_import_job_delete(&state, job_id).unwrap_err();
    assert_eq!(err, "任务进行中，无法删除");

    // 终态（cancelled）→ 可删
    db.finish_job(job_id, "cancelled", "{}").unwrap();
    ipc::import::fetch_import_job_delete(&state, job_id).unwrap();
    let logs: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM logs WHERE job_id = ?1",
            [job_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(logs, 0);

    let _ = AppEvent::Probe { ts: String::new() }; // 引用 events（模块树编译需要）
}
