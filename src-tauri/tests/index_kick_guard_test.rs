//! index::kick 让路守卫（2026-10-09 审计 P2：导入在场时 kick 为 no-op，
//! 任务留 pending，导入收尾兜底唤醒；修复联拍 kick 穿透让路闸）。
//!
//! 独立测试文件：守卫读进程级全局旗（IMPORT_IN_FLIGHT），与其他依赖
//! kick 实际派发的测试（import_then_index_chain 等）并行会互相干扰——
//! 单独成 binary 串行隔离。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, settings, tasks, thumbs,
};

use std::path::Path;
use std::time::{Duration, Instant};

use common::open_db;
use events::EventBus;

/// 造一条 pending thumb 任务 + 对应 photo 资产（路径指向真实可解码 JPEG，
/// worker 放行后能跑完到 done）。
fn seed_pending_thumb_task(db_dir: &Path, photo: &Path) {
    let db = open_db(db_dir);
    db.0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
             created_at, origin, missing) \
             VALUES (?1, ?2, 1, '2026-01-01T00:00:00Z', 0, 'photo', 'imported', \
             '2026-01-01T00:00:00Z', 'imported', 0)",
            rusqlite::params![
                photo.to_string_lossy().to_string(),
                photo.file_name().unwrap().to_string_lossy().to_string(),
            ],
        )
        .unwrap();
    let id: i64 = db
        .0
        .query_row(
            "SELECT id FROM assets WHERE path = ?1",
            [photo.to_string_lossy().to_string()],
            |r| r.get(0),
        )
        .unwrap();
    db.0
        .execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('thumb', ?1, 'pending', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
            [id],
        )
        .unwrap();
}

fn task_state(db_dir: &Path) -> (String, i64) {
    let db = open_db(db_dir);
    db.0
        .query_row(
            "SELECT state, COUNT(*) FROM index_tasks WHERE kind = 'thumb' \
             GROUP BY state HAVING COUNT(*) > 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
}

#[test]
fn kick_is_noop_while_import_in_flight_and_wakes_after_release() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let photo = dir.path().join("IMG_0001.jpg");
    image::RgbImage::from_pixel(1024, 768, image::Rgb([120, 140, 160]))
        .save(&photo)
        .unwrap();
    seed_pending_thumb_task(&db_dir, &photo);
    assert_eq!(task_state(&db_dir).0, "pending");

    let supervisor = tasks::TaskSupervisor::new(EventBus::new());

    // 导入在场：kick 为 no-op——任务保持 pending（worker 池不派发）
    index::set_import_in_flight(true);
    index::kick(db_dir.clone(), &supervisor);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        task_state(&db_dir).0,
        "pending",
        "导入在场时 kick 不得派发 worker"
    );

    // 放行后的 kick（导入收尾路径同构）：worker 池跑完到 done
    index::set_import_in_flight(false);
    index::kick(db_dir.clone(), &supervisor);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let (state, count) = task_state(&db_dir);
        if state == "done" && count == 1 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "放行后 kick 应唤醒 worker 跑完任务（当前 {state}×{count}）"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // 收尾复位，避免影响本 binary 内后续断言（当前仅本测试）
    index::set_import_in_flight(false);
}
