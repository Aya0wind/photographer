//! 断点恢复语义：软取消返回部分统计、暂停恢复无损续跑、中断后 resume
//! 只重做 pending（`.part` 残留清扫）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;
use std::time::Duration;

use common::{
    build_many, count_assets, find_part_files, job_status, journal_states, open_db, pending_count,
    plan_for, wait_completed, SlowSource,
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
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(15),
        }),
        plan_for(target.path()),
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
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(10),
        }),
        plan_for(target.path()),
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
fn resume_after_interruption_redoes_pending_only() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 12);

    // 会话一：取消中断
    let db = open_db(db_dir.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus.clone(),
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(12),
        }),
        plan_for(target.path()),
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());
    wait_completed(&mut rx);
    controls.cancel();
    let partial = handle.join().unwrap();
    assert!(partial.done_files < 12);

    // 模拟崩溃残留：暂存目录里塞一个孤儿 .part
    let part_dir = target.path().join(".smartphoto-part");
    fs::create_dir_all(&part_dir).unwrap();
    fs::write(part_dir.join("999.part"), b"half-written").unwrap();

    // 会话二：resume 重建（verified 跳过，pending 重做）
    let db2 = open_db(db_dir.path());
    let engine = Engine::resume(
        db2,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
        job_id,
    )
    .unwrap();
    let stats = engine.run();

    assert_eq!(stats.done_files, 12, "续传后无遗漏");
    assert_eq!(stats.failed_files, 0);
    assert_eq!(count_assets(&open_db(db_dir.path())), 12, "无重复入库");

    // .part 无残留：暂存目录已清除
    assert!(!part_dir.exists(), "resume 后暂存目录应清除");
    assert!(find_part_files(target.path()).is_empty());

    // journal 全 verified，job 终态 done
    assert!(journal_states(&open_db(db_dir.path()), job_id)
        .into_iter()
        .all(|s| s == FileState::Verified));
    assert_eq!(job_status(&open_db(db_dir.path()), job_id), "done");
}
