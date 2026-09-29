//! M8 性能包：① 同卷 rename 快道（直接尝试 rename + xxh=0 哨兵 + moved 计
//! 数 + 被锁回退流式）② 后台哈希通道（快道登记 → worker 补算 → requeue
//! 家族幂等 + 代际标记）③ 缩略图 LRU（touch 续命 + 淘汰边界 + 保护窗 +
//! 上限刷新）。copy 模式回归：内联哈希照旧、零 hash 任务。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;
use std::path::Path;
use std::time::{Duration, SystemTime};

use common::{build_source, count_assets, expected_ungrouped_dir, open_db, run_engine};
use events::EventBus;
use image::DynamicImage;
use import::engine::ImportMode;

/// 轮询等待（后台 supervisor 线程产物：marker / DB 终态）。
fn wait_until(secs: u64, predicate: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    predicate()
}

fn scalar(db: &db::Db, sql: &str) -> i64 {
    db.0.query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn same_volume_move_takes_fast_path_and_backfills_hash() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());
    // move 会搬走源：期望目标路径（依赖源 mtime）必须在运行前预计算
    let expected: Vec<(String, Vec<u8>, std::path::PathBuf)> = files
        .iter()
        .map(|(rel, content)| {
            (
                rel.clone(),
                content.clone(),
                expected_ungrouped_dir(db_dir.path(), target.path())
                    .join(rel.rsplit('/').next().unwrap()),
            )
        })
        .collect();

    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.mode = ImportMode::Move;
    });
    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.failed_files, 0, "{stats:?}");
    assert_eq!(stats.moved, 3, "快道 rename 也计移动成功: {stats:?}");
    assert_eq!(stats.source_delete_failed, 0);

    // 目标字节一致 + 源已随 rename 消失（tempdir 同卷 → 快道生效）
    let db = open_db(db_dir.path());
    assert_eq!(count_assets(&db), 3);
    for (rel, content, dst) in &expected {
        assert_eq!(fs::read(dst).unwrap(), *content, "目标字节一致: {rel}");
        assert!(
            !src.path().join(rel.replace('/', "\\")).exists(),
            "快道后源应消失: {rel}"
        );
    }

    // 快道指纹：全部 xxh=0 哨兵（无内联哈希）+ 每资产一条 pending hash 任务
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM assets WHERE xxhash = 0"),
        3
    );
    assert_eq!(
        scalar(
            &db,
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'hash' AND state = 'pending'"
        ),
        3
    );

    // worker 补算（step 认领 hash 通道）→ 哨兵全部就位，幂等不重建
    index::run_pending(db_dir.path(), 2);
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM assets WHERE xxhash = 0"),
        0,
        "补算后不应再有哨兵"
    );
    assert_eq!(db.create_hash_tasks_for_unhashed().unwrap(), 0);
}

/// Windows：源被无 FILE_SHARE_DELETE 的句柄锁住 → rename 失败 → 静默回退
/// 流式复制（内联哈希、删源走常规链路），导入绝不失败。
#[test]
#[cfg(windows)]
fn locked_source_falls_back_to_streaming() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    let locked = src
        .path()
        .join("DCIM")
        .join("100CANON")
        .join("IMG_0001.jpg");

    let _guard = {
        use std::os::windows::fs::OpenOptionsExt;
        fs::OpenOptions::new()
            .read(true)
            .share_mode(1) // 仅 FILE_SHARE_READ：禁 delete/rename
            .open(&locked)
            .unwrap()
    };

    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.mode = ImportMode::Move;
    });
    assert_eq!(stats.done_files, 3, "锁源回退不得判导入失败: {stats:?}");
    assert_eq!(stats.failed_files, 0);
    assert_eq!(stats.moved, 2, "另两文件快道成功: {stats:?}");
    // 锁定文件走流式：删源被句柄挡住 → 独立计数（warn 不失败）
    assert_eq!(stats.source_delete_failed, 1, "{stats:?}");

    let db = open_db(db_dir.path());
    // 锁定文件（流式）内联哈希非 0；另两文件（快道）哨兵 0
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM assets WHERE xxhash = 0"),
        2
    );
    let streamed: i64 =
        db.0.query_row(
            "SELECT xxhash FROM assets WHERE filename = 'IMG_0001.jpg'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(streamed, 0, "流式路径内联哈希正常");
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM index_tasks WHERE kind = 'hash'"),
        2,
        "只有快道资产登记 hash 任务"
    );
    // 锁定源文件仍在（句柄挡删）；另两个已消失
    assert!(locked.exists(), "锁定的源文件不得被吞");
    assert!(!src
        .path()
        .join("DCIM")
        .join("100CANON")
        .join("IMG_0002.CR3")
        .exists());
}

#[test]
fn hash_requeue_family_and_generation_marker() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3, "{stats:?}");

    // 模拟历史库：全部打回哨兵 → 批量建任务（pending 去重 → 二次 0）
    let db = open_db(db_dir.path());
    db.0.execute("UPDATE assets SET xxhash = 0", []).unwrap();
    assert_eq!(db.create_hash_tasks_for_unhashed().unwrap(), 3);
    assert_eq!(db.create_hash_tasks_for_unhashed().unwrap(), 0);

    // done → requeue 家族复位（对齐 exif/phash/thumb 的全量重排）
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'hash'",
        [],
    )
    .unwrap();
    assert_eq!(db.requeue_hash_tasks_for_all().unwrap(), 3);

    // 代际自愈：首次触发（后台复位 + 跑完 + 落 marker），二次 marker 短路
    let bus = EventBus::new();
    let supervisor = tasks::TaskSupervisor::new(bus.clone());
    index::refresh_hash_for_generation(db_dir.path().to_path_buf(), &bus, &supervisor);
    let marker = db_dir.path().join("hash-gen-1.marker");
    assert!(
        wait_until(10, || marker.is_file()),
        "代际标记应落盘: {}",
        marker.display()
    );
    index::refresh_hash_for_generation(db_dir.path().to_path_buf(), &bus, &supervisor);
    // 后台 worker 跑完 → 哨兵清零（run_pending 在 supervisor 线程内完成）
    assert!(
        wait_until(10, || scalar(
            &db,
            "SELECT COUNT(*) FROM assets WHERE xxhash = 0"
        ) == 0),
        "后台补算应清掉全部哨兵"
    );
}

#[test]
fn thumb_lru_evicts_oldest_beyond_cap_and_protects_recent() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let tier = db_dir.join("thumbs").join("256");
    fs::create_dir_all(&tier).unwrap();
    let now = SystemTime::now();
    // 4 个缓存：2 个 1 天前、1 个 10 分钟前（窗外）、1 个 1 秒前（窗内）
    for (name, size, age_secs) in [
        ("old1.jpg", 60u64, 86_400u64),
        ("old2.jpg", 40, 86_400),
        ("mid.jpg", 30, 600),
        ("fresh.jpg", 70, 1),
    ] {
        let path = tier.join(name);
        fs::write(&path, vec![0u8; size as usize]).unwrap();
        let file = fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(now - Duration::from_secs(age_secs))
            .unwrap();
    }

    // cap=40：需释放 160 → old1(60)+old2(40)+mid(30)=130 依次最旧先删，
    // fresh(70) 落在 60s 保护窗内跳过（即使仍未达标也不删）
    let (deleted, freed) = thumbs::evict_lru(&db_dir, 40, 60, now);
    assert_eq!((deleted, freed), (3, 130), "最旧先删、窗内保护");
    assert!(tier.join("fresh.jpg").is_file(), "保护窗内文件不删");

    // min_age=0（注入）：fresh 也可删
    let (deleted, freed) = thumbs::evict_lru(&db_dir, 40, 0, now);
    assert_eq!((deleted, freed), (1, 70));

    // 未超限：no-op
    let (deleted, freed) = thumbs::evict_lru(&db_dir, u64::MAX, 60, now);
    assert_eq!((deleted, freed), (0, 0));
}

/// 造一张真实可解码的 JPEG（image crate 编码，非魔数桩）。
fn write_real_jpg(path: &Path, w: u32, h: u32) {
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

#[test]
fn thumb_cache_hit_touches_mtime_and_cap_refreshes() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().to_path_buf();
    let src = dir.path().join("photo.jpg");
    write_real_jpg(&src, 400, 300);

    let first = thumbs::thumb_file(&db_dir, &src, 256).expect("应生成缓存");
    let cache = std::path::PathBuf::from(&first);
    // 人工老化 1 天 → 命中后应被 touch 续命（mtime 即热度账）
    let file = fs::File::options().write(true).open(&cache).unwrap();
    file.set_modified(SystemTime::now() - Duration::from_secs(86_400))
        .unwrap();
    drop(file);

    let hit = thumbs::thumb_file(&db_dir, &src, 256).expect("应命中");
    assert_eq!(hit, first);
    let mtime = fs::metadata(&cache).unwrap().modified().unwrap();
    assert!(
        SystemTime::now()
            .duration_since(mtime)
            .expect("mtime 不应在未来")
            < Duration::from_secs(5),
        "命中应 touch 续命"
    );

    // 上限刷新（0 = 不限；测试全局量，收尾还原默认 20GB）
    thumbs::set_thumb_cache_cap_bytes(0);
    assert_eq!(thumbs::thumb_cache_cap_bytes(), 0);
    thumbs::set_thumb_cache_cap_bytes(20 * 1024 * 1024 * 1024);
    assert_eq!(thumbs::thumb_cache_cap_bytes(), 20 * 1024 * 1024 * 1024);
}

/// copy 模式回归：不走快道（内联哈希照旧、零 hash 任务）。
#[test]
fn copy_mode_still_hashes_inline() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3, "{stats:?}");

    let db = open_db(db_dir.path());
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM assets WHERE xxhash = 0"),
        0
    );
    assert_eq!(
        scalar(&db, "SELECT COUNT(*) FROM index_tasks WHERE kind = 'hash'"),
        0
    );
}
