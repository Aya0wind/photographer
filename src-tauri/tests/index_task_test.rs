//! 索引任务系统（导入/索引任务分离）：导入入册→待办生成、worker 跑完→
//! 三档缓存+thumb_state=1、损坏图→attempts 重试后 failed、video→永久占位、
//! 中断恢复（running 复位→续跑）、worker 数=核心数、DTO 契约。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;
use std::time::Duration;

use common::{open_db, run_engine};
use events::AppEvent;
use image::DynamicImage;

/// 造一张真实可解码的 JPEG（image crate 编码，非魔数桩）。
fn write_real_jpg(path: &std::path::Path, w: u32, h: u32) {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    DynamicImage::ImageRgb8(img)
        .write_with_encoder(encoder)
        .unwrap();
    fs::write(path, buf).unwrap();
}

fn states(db: &db::Db) -> Vec<(String, i64, String, i64)> {
    let stmt =
        db.0.prepare("SELECT kind, asset_id, state, attempts FROM index_tasks ORDER BY id")
            .unwrap();
    rows_to_vec(stmt)
}

fn rows_to_vec(mut stmt: rusqlite::Statement<'_>) -> Vec<(String, i64, String, i64)> {
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn thumb_state(db: &db::Db, filename: &str) -> i32 {
    db.0.query_row(
        "SELECT thumb_state FROM assets WHERE filename = ?1",
        [filename],
        |r| r.get::<_, i64>(0),
    )
    .unwrap() as i32
}

#[test]
fn import_creates_thumb_tasks_only_for_photo_and_raw() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // build_source: jpg + CR3(raw) + MP4(video)
    common::build_source(src.path());

    let (_job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3);

    let database = open_db(db_dir.path());
    let tasks = states(&database);
    assert_eq!(tasks.len(), 2, "video 不产生缩略图待办: {tasks:?}");
    assert!(tasks
        .iter()
        .all(|(kind, _, state, attempts)| kind == "thumb" && state == "pending" && *attempts == 0));
    // video 资产直接永久占位
    let video_state = thumb_state(&database, "MVI_0003.MP4");
    assert_eq!(video_state, 2, "video 永久占位");
    // photo/raw 仍 pending
    assert_eq!(thumb_state(&database, "IMG_0001.jpg"), 0);
}

#[test]
fn worker_completes_tasks_and_generates_three_tiers() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    write_real_jpg(&src.path().join("DCIM").join("IMG_0001.jpg"), 1600, 1200);

    let (_job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 1);

    let processed = index::run_pending(db_dir.path(), index::worker_count());
    assert_eq!(processed, 1, "一个待办任务");

    let database = open_db(db_dir.path());
    let tasks = states(&database);
    assert!(
        tasks.iter().all(|(_, _, state, _)| state == "done"),
        "{tasks:?}"
    );
    assert_eq!(thumb_state(&database, "IMG_0001.jpg"), 1);

    // 三档缓存文件都在 dbDir/thumbs/<tier>/ 下
    for tier in thumbs::SIZE_TIERS {
        let dir = db_dir.path().join("thumbs").join(tier.to_string());
        let count = fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
        assert!(count >= 1, "{tier} 档缓存缺失: {}", dir.display());
    }

    // 二次跑队列空（幂等，不重复处理）
    assert_eq!(index::run_pending(db_dir.path(), 2), 0);
}

#[test]
fn corrupt_image_fails_with_attempts_then_failed() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // 魔数合法但内容损坏的"jpg"（classify 过、image 解码必败）
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    fs::write(
        src.path().join("DCIM").join("broken.jpg"),
        common::shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 2048),
    )
    .unwrap();

    let (_job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 1, "导入本身不受缩略图失败影响");
    assert_eq!(stats.failed_files, 0);

    // attempts 递增语义（单连接确定性断言）：1..2 次失败回 pending，第 3 次落 failed
    let database = open_db(db_dir.path());
    for expected in [1i64, 2] {
        assert!(
            database.claim_index_task().unwrap().is_some(),
            "第 {expected} 次认领"
        );
        database.finish_index_task(1, false).unwrap();
        let tasks = states(&database);
        assert!(
            tasks
                .iter()
                .any(|(_, _, state, attempts)| state == "pending" && *attempts == expected),
            "失败 {expected} 次后应回 pending: {tasks:?}"
        );
    }
    assert!(database.claim_index_task().unwrap().is_some());
    database.finish_index_task(1, false).unwrap();
    let tasks = states(&database);
    assert!(
        tasks
            .iter()
            .any(|(_, _, state, attempts)| state == "failed" && *attempts == 3),
        "attempts 封顶后落 failed: {tasks:?}"
    );
    assert_eq!(
        database.claim_index_task().unwrap(),
        None,
        "failed 不再被认领"
    );

    // 重置为 pending（模拟运维重试）→ worker 实际处理：解码失败 →
    // thumb_state=2（永久占位）+ attempts 再 +1（仍 failed）
    database
        .0
        .execute("UPDATE index_tasks SET state = 'pending', attempts = 0", [])
        .unwrap();
    index::run_pending(db_dir.path(), 1);
    let tasks = states(&database);
    assert_eq!(
        thumb_state(&database, "broken.jpg"),
        2,
        "失败资产永久占位（按需兜底不再排队）"
    );
    // 单 worker 同轮内自动重试到封顶：attempts 1→2→3 落 failed
    assert!(
        tasks
            .iter()
            .all(|(_, _, state, attempts)| state == "failed" && *attempts == 3),
        "{tasks:?}"
    );
}

#[test]
fn interrupted_running_tasks_resume_on_startup_path() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    write_real_jpg(&src.path().join("DCIM").join("IMG_0001.jpg"), 1200, 900);
    let (_job_id, _stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    let database = open_db(db_dir.path());

    // 模拟崩溃窗口：任务被置 running 后进程死掉
    database
        .0
        .execute("UPDATE index_tasks SET state = 'running'", [])
        .unwrap();

    // 启动恢复路径：复位 running → pending，返回待办数
    let pending = index::resume_pending(db_dir.path());
    assert_eq!(pending, 1, "running 应复位为 pending");
    let tasks = states(&database);
    assert!(
        tasks.iter().all(|(_, _, state, _)| state == "pending"),
        "{tasks:?}"
    );

    // worker 续跑到完成
    assert_eq!(index::run_pending(db_dir.path(), 2), 1);
    assert_eq!(thumb_state(&database, "IMG_0001.jpg"), 1);

    // 无待办时 resume 返回 0（不发事件）
    assert_eq!(index::resume_pending(db_dir.path()), 0);
}

#[test]
fn index_task_resumed_event_serializes() {
    let ev = AppEvent::IndexTaskResumed { pending: 7 };
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["type"], "indexTaskResumed");
    assert_eq!(json["pending"], 7);
}

#[test]
fn worker_count_equals_physical_cores() {
    assert_eq!(
        index::worker_count(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
        "索引 worker 数 = 物理核心数全量"
    );
    assert_eq!(
        thumbs::permit_count_for_test(8),
        8,
        "permit 池 = 核心数直通（不再 clamp 1..6）"
    );
}

#[test]
fn import_then_index_chain_via_ipc() {
    // IPC 编排：导入完成钩子派发索引 worker（异步）→ 轮询到全 done
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    write_real_jpg(&src.path().join("DCIM").join("IMG_0001.jpg"), 1024, 768);
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let job_id = ipc::start_import(&state, common::ipc_plan(&state, target.path())).unwrap();
    assert!(
        common::wait_done(&state, Duration::from_secs(30)),
        "导入应收尾"
    );
    let _ = job_id;

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let database = open_db(db_dir.path());
    loop {
        let count: i64 = database
            .0
            .query_row(
                "SELECT COUNT(*) FROM index_tasks WHERE state IN ('pending','running')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        if count == 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "索引 worker 未在期限内跑完"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        thumb_state(&database, "IMG_0001.jpg"),
        1,
        "导入链自动完成索引"
    );
}
