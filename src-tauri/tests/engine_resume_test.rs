//! 断点恢复语义：软取消返回部分统计、暂停恢复无损续跑、中断后 resume
//! 只重做 pending（`.part` 残留清扫）、启动时 running 转 paused，保留恢复入口。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, plan_with_album,
    settings, tasks, thumbs,
};

use std::fs;
use std::time::Duration;

use common::{
    build_many, count_assets, find_part_files, job_status, journal_states, open_db, pending_count,
    wait_completed, SlowSource,
};
use devices::volume::VolumeSource;
use events::{EventBus, FileState};
use import::engine::Engine;

#[test]
fn soft_cancel_returns_partial_and_job_cancelled() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 12);

    let db = open_db(db_dir.path());
    let plan = plan_with_album(&db, db_dir.path(), target.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(15),
        }),
        plan,
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());

    // 首个文件完成后软取消
    wait_completed(&mut rx);
    controls.cancel();
    let stats = handle.join().unwrap();

    assert!(stats.done_files < 12, "软取消应停在部分完成: {stats:?}");
    assert_eq!(
        stats.done_files + stats.failed_files,
        stats.total_files - pending_count(&open_db(db_dir.path()), job_id)
    );
    let status = job_status(&open_db(db_dir.path()), job_id);
    assert_eq!(status, "cancelled");
}

#[test]
fn pause_resume_completes_without_loss() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 10);

    let db = open_db(db_dir.path());
    let plan = plan_with_album(&db, db_dir.path(), target.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(10),
        }),
        plan,
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());

    // 首个文件完成后暂停 → 验证停工 → 恢复 → 完成
    wait_completed(&mut rx);
    controls.pause();
    std::thread::sleep(Duration::from_millis(120));
    assert!(!controls.is_done(), "暂停期间 run 不得结束");
    controls.resume();
    let stats = handle.join().unwrap();

    assert_eq!(stats.done_files, 10);
    assert_eq!(job_status(&open_db(db_dir.path()), job_id), "done");
}

#[test]
fn resume_discards_copying_and_completes_pending_only() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 12);

    // 会话一：取消中断
    let db = open_db(db_dir.path());
    let plan = plan_with_album(&db, db_dir.path(), target.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus.clone(),
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(12),
        }),
        plan,
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());
    wait_completed(&mut rx);
    controls.cancel();
    let partial = handle.join().unwrap();
    assert!(partial.done_files < 12);

    // 模拟崩溃残留：暂存目录里塞一个孤儿 .part
    let part_dir = target.path().join(".photographer-part");
    fs::create_dir_all(&part_dir).unwrap();
    fs::write(part_dir.join("999.part"), b"half-written").unwrap();

    // 会话二：resume 重建（verified 跳过，pending 重做；plan 从 journal
    // 重建——与 ipc::resume_import 同构，布局公式由 plan.album_id 派生）
    let db2 = open_db(db_dir.path());
    let interrupted = db2.all_job_files(job_id).unwrap().into_iter()
        .find(|row| row.state == FileState::Pending).unwrap();
    db2.0.execute("UPDATE job_files SET state = 'copying' WHERE job_id = ?1 AND src = ?2",
        rusqlite::params![job_id, interrupted.src]).unwrap();
    let plan2: import::engine::ImportPlan =
        serde_json::from_str(&db2.job_plan_json(job_id).unwrap().unwrap()).unwrap();
    let engine = Engine::resume(
        db2,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan2,
        job_id,
    )
    .unwrap();
    let stats = engine.run();

    assert_eq!(stats.done_files, 11, "仅丢弃上次未完成的一个文件");
    assert_eq!(stats.failed_files, 1);
    assert_eq!(count_assets(&open_db(db_dir.path())), 11, "未完成文件不入库");

    // .part 无残留：暂存目录已清除
    assert!(!part_dir.exists(), "resume 后暂存目录应清除");
    assert!(find_part_files(target.path()).is_empty());

    let db = open_db(db_dir.path());
    let lost = db.all_job_files(job_id).unwrap().into_iter().find(|row| row.src == interrupted.src).unwrap();
    assert_eq!(lost.state, FileState::Failed);
    assert!(lost.error.unwrap().starts_with(events::LOST_IMPORT_FILE_PREFIX));
    assert!(db.retry_failed_into_new_job(job_id).unwrap().is_none(), "丢失文件不参加失败重试");
    assert_eq!(job_status(&open_db(db_dir.path()), job_id), "done");
}

/// 重启后 running 自动暂停，paused/终态保持，恢复入口仍可用。
#[test]
fn startup_pauses_running_and_keeps_paused_jobs() {
    let db_dir = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());

    // 模拟进程死亡：job 建好后 run 永未收尾——status 停在 running；
    // paused 是设备失联自动暂停后进程退出的遗留；done 是正常终态
    let running = db
        .create_job_with_plan("import", "vol:X", "SD Card", 3, 300, "{}")
        .unwrap();
    let paused = db
        .create_job_with_plan("import", "vol:Y", "SD Card 2", 2, 200, "{}")
        .unwrap();
    db.finish_job(paused, "paused", "{}").unwrap();
    let done = db
        .create_job_with_plan("import", "vol:Z", "SD Card 3", 1, 100, "{}")
        .unwrap();
    db.finish_job(done, "done", "{}").unwrap();
    // running 任务的 journal（进程死时已结算 1 文件、余 2 pending）
    db.0.execute(
        "INSERT INTO job_files (job_id, src, dst, size, state) VALUES \
             (?1, 'a.jpg', 'X:/a.jpg', 100, 'verified'), \
             (?1, 'b.jpg', '', 100, 'pending')",
        [running],
    )
    .unwrap();

    // 启动核对：仅 running 转 paused。
    let reaped = db.reap_orphan_import_jobs().unwrap();
    assert_eq!(reaped, vec![running]);
    assert_eq!(job_status(&db, running), "paused");
    assert_eq!(job_status(&db, paused), "paused");
    assert_eq!(job_status(&db, done), "done", "终态任务不得被改动");

    // 自动暂停记录一行日志。
    let logged: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM logs WHERE job_id = ?1 AND level = 'warn' \
             AND message LIKE '%进程重启%'",
            [running],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(logged, 1);

    // journal 保留：恢复语义不毁（设备回连后 resume 仍可续传 pending）
    let states = journal_states(&db, running);
    assert!(
        states.contains(&FileState::Pending),
        "journal 不动: {states:?}"
    );

    // 幂等：二次启动无孤儿
    assert!(db.reap_orphan_import_jobs().unwrap().is_empty());

    // 暂停任务仍保留恢复数据，必须显式取消才能删除历史。
    assert!(db.delete_job_history(running).unwrap().is_err());
    db.finish_job(running, "cancelled", "{}").unwrap();
    assert!(matches!(db.delete_job_history(running).unwrap(), Ok(true)));
}

// ---------------------------------------------------------------------------
// 删库软停（2026-10-09 审计 P0）：导入在跑时目标照片库被移除登记 →
// 会话以明确错误中止，journal 落 failed，不写悬挂 library_id 的幽灵行。
// ---------------------------------------------------------------------------

#[test]
fn import_aborts_when_target_library_removed_mid_run() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 6);

    let engine_db = open_db(db_dir.path());
    let plan = common::plan_for(&engine_db, db_dir.path(), target.path());
    let mut engine = Engine::new(
        engine_db,
        EventBus::new(),
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(20),
        }),
        plan.clone(),
    );
    let job_id = engine.begin().unwrap();
    // begin 落任务后、run 起跑前移除登记（另一连接；删除即触发点查判死）。
    // 新守卫要求库内至少保留一个照片库——先垫一个占位库再删目标库。
    let admin = open_db(db_dir.path());
    let keeper = tempfile::tempdir().unwrap();
    admin
        .photos_library_register("占位库", &keeper.path().to_string_lossy(), db_dir.path())
        .unwrap();
    admin.photos_library_remove(&plan.target_library_id).unwrap();
    drop(admin);

    let stats = engine.run();

    assert_eq!(stats.done_files, 0, "不再登记任何文件");
    assert_eq!(stats.failed_files, 6, "全部按失败收尾");
    let check = open_db(db_dir.path());
    assert_eq!(count_assets(&check), 0, "不写悬挂 library_id 的幽灵行");
    assert_eq!(job_status(&check, job_id), "failed", "会话终态 failed");
    let errors: Vec<String> = check
        .0
        .prepare("SELECT COALESCE(error, '') FROM job_files WHERE job_id = ?1")
        .unwrap()
        .query_map([job_id], |r| r.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(errors.len(), 6);
    assert!(
        errors.iter().all(|e| e.contains("目标照片库已被删除")),
        "journal 失败行带明确错误文案: {errors:?}"
    );
    assert_eq!(journal_states(&check, job_id), vec![FileState::Failed; 6]);
}
