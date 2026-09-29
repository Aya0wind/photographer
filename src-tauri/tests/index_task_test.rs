//! 索引任务系统（导入/索引任务分离）：导入入册→待办生成、worker 跑完→
//! 三档缓存+thumb_state=1、损坏图→attempts 重试后 failed、视频不导入。
//!（M8：不占索引待办）、
//! 中断恢复（running 复位→续跑）、worker 数=核心数、DTO 契约。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
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
    // build_source: 两张 jpg + CR3(raw) + MP4（应被忽略）
    common::build_source(src.path());
    std::fs::write(src.path().join("DCIM/100CANON/MVI_0003.MP4"), b"video").unwrap();

    let (_job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3);

    let database = open_db(db_dir.path());
    let tasks = states(&database);
    // 0021 起导入期为每张照片建 thumb+eyes+blur 三通道任务（eyes 无模型
    // 保持 pending 属设计：模型收录后 kick 自然续跑）
    assert_eq!(tasks.len(), 9, "三通道 × 3 张照片: {tasks:?}");
    assert!(tasks
        .iter()
        .all(|(_, _, state, attempts)| state == "pending" && *attempts == 0));
    assert_eq!(
        tasks.iter().filter(|(kind, _, _, _)| kind == "thumb").count(),
        3,
        "仅图片产生缩略图待办"
    );
    assert!(
        database.asset_id_by_path("MVI_0003.MP4").unwrap().is_none(),
        "视频不编目"
    );
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
    // thumb+blur 完成；eyes 无模型被跳过、保持 pending（0021 语义）
    assert_eq!(processed, 2, "thumb+blur 各一件完成");

    let database = open_db(db_dir.path());
    let tasks = states(&database);
    assert!(
        tasks
            .iter()
            .filter(|(kind, _, _, _)| kind != "eyes")
            .all(|(_, _, state, _)| state == "done"),
        "{tasks:?}"
    );
    assert!(
        tasks
            .iter()
            .any(|(kind, _, state, _)| kind == "eyes" && state == "pending"),
        "eyes 无模型保持 pending: {tasks:?}"
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
            database.claim_index_task("thumb").unwrap().is_some(),
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
    assert!(database.claim_index_task("thumb").unwrap().is_some());
    database.finish_index_task(1, false).unwrap();
    let tasks = states(&database);
    assert!(
        tasks
            .iter()
            .any(|(_, _, state, attempts)| state == "failed" && *attempts == 3),
        "attempts 封顶后落 failed: {tasks:?}"
    );
    assert_eq!(
        database.claim_index_task("thumb").unwrap(),
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
    // 单 worker 同轮内自动重试到封顶：attempts 1→2→3 落 failed；eyes 无模型
    // 保持 pending、blur 成功 done（0021 三通道）
    assert!(
        tasks.iter().any(|(kind, _, state, attempts)| kind == "thumb"
            && state == "failed"
            && *attempts == 3),
        "{tasks:?}"
    );
    assert!(
        tasks.iter().any(|(kind, _, state, _)| kind == "eyes" && state == "pending"),
        "{tasks:?}"
    );
    assert!(
        tasks.iter().any(|(kind, _, state, _)| kind == "blur" && state == "done"),
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

    // 启动恢复路径：复位 running → pending，返回待办数（三通道）
    let pending = index::resume_pending(db_dir.path());
    assert_eq!(pending, 3, "running 应复位为 pending（thumb/eyes/blur）");
    let tasks = states(&database);
    assert!(
        tasks.iter().all(|(_, _, state, _)| state == "pending"),
        "{tasks:?}"
    );

    // worker 续跑到完成（eyes 无模型跳过保持 pending）
    assert_eq!(index::run_pending(db_dir.path(), 2), 2);
    assert_eq!(thumb_state(&database, "IMG_0001.jpg"), 1);

    // eyes 无模型保持 pending → resume 计数恒为该残留（幂等空转）
    assert_eq!(index::resume_pending(db_dir.path()), 1);
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
    // eyes 无模型保持 pending 属设计（模型收录后 kick 续跑）——收敛判据
    // 排除 eyes 通道
    loop {
        let count: i64 = database
            .0
            .query_row(
                "SELECT COUNT(*) FROM index_tasks WHERE state IN ('pending','running') \
                 AND kind != 'eyes'",
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

#[test]
fn missing_source_keeps_thumb_state_zero_and_completes_on_restore() {
    // R2（2026-09-28 边界修复）：源文件被第三方移动/删除是**暂态**——
    // 索引任务失败走 attempts 封顶，但绝不置 thumb_state=2 毒化资产
    //（旧版真机实证：文件移回后 state=2 永久 unavailable 不自愈）。
    // 文件移回 + 重排任务 → 正常完成（真机实测 4s 内重新生成成功）。
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    write_real_jpg(&src.path().join("DCIM").join("IMG_0001.jpg"), 1400, 1000);
    let (_job_id, _stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    let database = open_db(db_dir.path());
    let path: String = database
        .0
        .query_row(
            "SELECT path FROM assets WHERE filename = 'IMG_0001.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let bytes = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();

    // 缺失期：thumb 任务 attempts 封顶落 failed，但 thumb_state 保持 0
    index::run_pending(db_dir.path(), 1);
    assert_eq!(
        thumb_state(&database, "IMG_0001.jpg"),
        0,
        "源缺失不得落 state=2（毒化）"
    );
    let tasks = states(&database);
    assert!(
        tasks
            .iter()
            .any(|(kind, _, state, _)| kind == "thumb" && state == "failed"),
        "缺失期任务按 attempts 封顶失败: {tasks:?}"
    );

    // 文件移回（同路径同内容）：重排任务即可完成
    fs::write(&path, &bytes).unwrap();
    database
        .0
        .execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0 \
             WHERE kind IN ('thumb', 'blur')",
            [],
        )
        .unwrap();
    index::run_pending(db_dir.path(), 1);
    assert_eq!(
        thumb_state(&database, "IMG_0001.jpg"),
        1,
        "文件移回后重排任务应完成并翻 state=1"
    );
    let tasks = states(&database);
    assert!(
        tasks
            .iter()
            .any(|(kind, _, state, _)| kind == "thumb" && state == "done"),
        "移回后 thumb 任务完成: {tasks:?}"
    );
}
