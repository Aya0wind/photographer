//! 按需缩略图管线（M3）：asset_thumb_get(asset_id) 缓存命中直返 / 未命中入队
//! 后台生成 + ThumbnailReady 事件、同 (asset,size) 去抖、有界队列满丢弃、
//! RAW/缺资产不入队、队列 Drop 停工。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::state_with_library;
use events::AppEvent;
use image::DynamicImage;
use ipc::thumb::ThumbJob;

/// 造一张渐变 JPG（沿用 thumb_test 的生成方式）。
fn write_jpg(path: &Path, w: u32, h: u32) {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    let img = DynamicImage::ImageRgb8(img);
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    img.write_with_encoder(encoder).unwrap();
    std::fs::write(path, buf).unwrap();
}

/// 建库 + 资产行（path 指向真实文件），返回 (AppState, asset_id)。
fn state_with_asset(db_dir: &Path, src: &Path) -> (ipc::AppState, i64) {
    let database = common::open_db(db_dir);
    database
        .insert_asset(&db::AssetRow {
            path: src.to_string_lossy().into_owned(),
            filename: src.file_name().unwrap().to_string_lossy().into_owned(),
            size: 123,
            mtime: "2026-09-01T00:00:00.000Z".into(),
            xxhash: 42,
            kind: events::AssetKind::Photo,
            captured_at: None,
            camera: None,
            source: "imported".into(),
            created_at: "2026-09-01T00:00:00.000Z".into(),
            origin: "imported".into(),
            width: None,
            height: None,
            iso: None,
            f_number: None,
            exposure_time: None,
            focal_length: None,
            lens: None,
            pair_asset_id: None,
            thumb_state: 0,
            rating: 0,
            flagged: 0,
            color_label: None,
            rejected: 0,
            orientation: None,
            flash: None,
            metering_mode: None,
            white_balance: None,
            exposure_program: None,
            software: None,
            artist: None,
            gps_lat: None,
            gps_lon: None,
        })
        .unwrap();
    let id = database
        .asset_id_by_path(&src.to_string_lossy())
        .unwrap()
        .unwrap();
    let holder = tempfile::tempdir().unwrap(); // source_dir 只需存在
    let state = state_with_library(db_dir, holder.path(), Duration::from_millis(1));
    (state, id)
}

/// 轮询总线直到出现 ThumbnailReady（跳过其他事件）；超时 panic。
fn wait_thumb_ready(state: &ipc::AppState, asset_id: i64) -> Option<String> {
    let mut rx = state.bus.subscribe();
    let deadline = Instant::now() + Duration::from_secs(20);
    // 订阅晚于触发时的已发事件会丢——先查缓存文件是否已生成
    loop {
        while let Ok(event) = rx.try_recv() {
            if let AppEvent::ThumbnailReady {
                asset_id: id, path, ..
            } = event
            {
                assert_eq!(id, asset_id, "事件应携带请求的 assetId");
                return path;
            }
        }
        if Instant::now() > deadline {
            panic!("20s 内未收到 ThumbnailReady");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn cache_hit_returns_path_immediately_without_event() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("hit.jpg");
    write_jpg(&src, 800, 600);
    let db_dir = tempfile::tempdir().unwrap();
    // 预生成缓存（同一条生成管线）
    let pre = thumbs::thumb_file(db_dir.path(), &src, 256).expect("预生成");
    let (state, id) = state_with_asset(db_dir.path(), &src);

    let got = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    match got {
        ipc::thumb::ThumbOutcome::Ready { path } => assert_eq!(
            PathBuf::from(&path),
            PathBuf::from(&pre),
            "命中返回同一缓存路径"
        ),
        other => panic!("命中应 Ready，实际 {other:?}"),
    }
    // 命中不发事件
    let mut rx = state.bus.subscribe();
    std::thread::sleep(Duration::from_millis(100));
    assert!(rx.try_recv().is_err(), "命中不得产生事件");
}

#[test]
fn miss_enqueues_generates_and_publishes_ready() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("miss.jpg");
    write_jpg(&src, 1024, 768);
    let db_dir = tempfile::tempdir().unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &src);

    let first = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    assert!(
        first == ipc::thumb::ThumbOutcome::Pending,
        "未命中入队后本轮返回 Pending（前端等事件重试），实际 {first:?}"
    );

    let path = wait_thumb_ready(&state, id).expect("生成成功事件应携带路径");
    let cache = PathBuf::from(&path);
    assert!(cache.exists(), "事件路径必须已落盘: {path}");
    assert!(cache.starts_with(db_dir.path().join("thumbs")));

    // 生成完成后：缓存命中直返
    let second = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    match second {
        ipc::thumb::ThumbOutcome::Ready { path: p } => {
            assert_eq!(p, path, "完成后转为命中")
        }
        other => panic!("完成后应 Ready，实际 {other:?}"),
    }
}

#[test]
fn same_asset_size_debounced_while_pending() {
    let queue = ipc::thumb::ThumbQueue::new();
    let job = |id: i64| ThumbJob {
        asset_id: id,
        size: 256,
        src: PathBuf::from("X:\\a.jpg"),
        db_dir: PathBuf::from("X:\\db"),
    };
    assert!(queue.push(job(1)), "首次入队成功");
    assert!(
        queue.push(job(1)),
        "同 (asset,size) 去抖返回成功（不重复入队）"
    );
    assert!(queue.push(job(2)), "不同 asset 正常入队");
    assert_eq!(
        queue.pending_len(),
        2,
        "job1+job2 在队（job1 的重复被去抖）"
    );
    queue.push(job(1));
    assert_eq!(queue.pending_len(), 2, "重复的 job1 仍被去抖");
}

#[test]
fn bounded_queue_full_drops_request() {
    let queue = ipc::thumb::ThumbQueue::new();
    for i in 0..ipc::thumb::QUEUE_CAPACITY as i64 {
        assert!(
            queue.push(ThumbJob {
                asset_id: i,
                size: 256,
                src: PathBuf::from("X:\\a.jpg"),
                db_dir: PathBuf::from("X:\\db"),
            }),
            "第 {i} 个应可入队"
        );
    }
    let overflow = ThumbJob {
        asset_id: ipc::thumb::QUEUE_CAPACITY as i64,
        size: 256,
        src: PathBuf::from("X:\\a.jpg"),
        db_dir: PathBuf::from("X:\\db"),
    };
    assert!(
        !queue.push(overflow),
        "队列满必须丢弃并返回 false（前端重试）"
    );
    assert_eq!(queue.pending_len(), ipc::thumb::QUEUE_CAPACITY);
}

#[test]
fn undecodable_and_missing_assets_return_none_without_enqueue() {
    let src_dir = tempfile::tempdir().unwrap();
    let txt = src_dir.path().join("notes.txt");
    std::fs::write(&txt, b"not media").unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &txt);

    // 非媒体扩展：Unavailable 且不入队
    assert_eq!(
        ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap(),
        ipc::thumb::ThumbOutcome::Unavailable,
        "不可解码：Unavailable 不入队"
    );
    assert_eq!(state.thumb_queue.pending_len(), 0, "不可解码不得入队");
    // 资产不存在：Unavailable
    assert_eq!(
        ipc::thumb::fetch_asset_thumb(&state, 9999, 256).unwrap(),
        ipc::thumb::ThumbOutcome::Unavailable
    );
    assert_eq!(state.thumb_queue.pending_len(), 0);

    // 视频扩展名不进入缩略图队列。
    let mp4 = src_dir.path().join("VID_0001.MP4");
    std::fs::write(&mp4, b"video-bytes").unwrap();
    let db_dir2 = tempfile::tempdir().unwrap();
    let (state2, id2) = state_with_asset(db_dir2.path(), &mp4);
    assert_eq!(
        ipc::thumb::fetch_asset_thumb(&state2, id2, 256).unwrap(),
        ipc::thumb::ThumbOutcome::Unavailable,
        "视频不生成缩略图"
    );
}

#[test]
fn thumbnail_ready_serializes_camel_case() {
    let ev = AppEvent::ThumbnailReady {
        asset_id: 7,
        size: 256,
        path: Some("X:\\thumbs\\256\\a.jpg".into()),
    };
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["type"], "thumbnailReady");
    assert_eq!(json["assetId"], 7);
    assert_eq!(json["size"], 256);
    assert!(json["path"].is_string());

    let failed = AppEvent::ThumbnailReady {
        asset_id: 7,
        size: 256,
        path: None,
    };
    assert!(serde_json::to_value(&failed).unwrap()["path"].is_null());
}

// ---------------------------------------------------------------------------
// 源缺失终态 Missing + state=2 自愈（2026-09-28 边界修复 R1/R3）
// ---------------------------------------------------------------------------

/// 删掉源文件（模拟第三方移动/删除）。返回原字节数据（供恢复用）。
fn delete_source(src: &Path) -> Vec<u8> {
    let bytes = std::fs::read(src).unwrap();
    std::fs::remove_file(src).unwrap();
    bytes
}

#[test]
fn missing_source_returns_missing_with_null_cached_path() {
    // 源被删 + 从未生成过缓存：Missing 且 cachedPath=null，绝不入队
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("gone.jpg");
    write_jpg(&src, 800, 600);
    let db_dir = tempfile::tempdir().unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &src);
    delete_source(&src);

    let got = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    assert_eq!(
        got,
        ipc::thumb::ThumbOutcome::Missing { cached_path: None },
        "源缺失必须给 Missing 终态（cachedPath=null），实际 {got:?}"
    );
    assert_eq!(state.thumb_queue.pending_len(), 0, "Missing 不得入队");
    // 幂等：重复请求仍是 Missing（不会退化为无限 Pending 重拉）
    assert_eq!(
        ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap(),
        ipc::thumb::ThumbOutcome::Missing { cached_path: None }
    );
}

#[test]
fn missing_source_recovers_cached_thumb_by_prefix_scan() {
    // 源被删但缓存缩略图在盘：Missing 且 cachedPath 指向既有缓存
    //（查看器/编辑器拿它出图而非空白舞台）
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("moved.jpg");
    write_jpg(&src, 800, 600);
    let db_dir = tempfile::tempdir().unwrap();
    let cached = thumbs::thumb_file(db_dir.path(), &src, 256).expect("预生成缓存");
    let (state, id) = state_with_asset(db_dir.path(), &src);
    delete_source(&src);

    let got = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    match got {
        ipc::thumb::ThumbOutcome::Missing {
            cached_path: Some(p),
        } => {
            assert_eq!(
                PathBuf::from(&p),
                PathBuf::from(&cached),
                "cachedPath 应按 xxh64 前缀恢复出既有缓存"
            );
            assert!(Path::new(&p).exists(), "恢复路径必须真实在盘: {p}");
        }
        other => panic!("应 Missing 带缓存路径，实际 {other:?}"),
    }
    assert_eq!(state.thumb_queue.pending_len(), 0, "Missing 不得入队");
}

#[test]
fn state2_self_heals_when_source_present_and_cache_cleared() {
    // R3：文件移回 + 缓存被清 + thumb_state=2（缺失期毒化的脏占位）
    // → 请求应重置 0 并入队重生成，而不是永久 Unavailable
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("back.jpg");
    write_jpg(&src, 900, 600);
    let db_dir = tempfile::tempdir().unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &src);

    // 毒化：直接置 state=2（模拟旧版缺失期落下的永久占位）
    let database = common::open_db(db_dir.path());
    database.set_thumb_state(id, 2).unwrap();
    drop(database);

    let got = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    assert!(
        got == ipc::thumb::ThumbOutcome::Pending,
        "源在盘 + 可解码：state=2 必须自愈入队（Pending），实际 {got:?}"
    );
    let path = wait_thumb_ready(&state, id).expect("自愈重生成应完成");
    assert!(Path::new(&path).exists(), "重生成缓存落盘: {path}");

    // 生成后命中直返 + DB 状态翻回 1
    let after = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    assert!(matches!(after, ipc::thumb::ThumbOutcome::Ready { .. }));
    let database = common::open_db(db_dir.path());
    let (.., state_val) = database.thumb_info_by_id(id).unwrap().unwrap();
    assert_eq!(state_val, 1, "自愈后 thumb_state 应回 1");
}

#[test]
fn cache_hit_over_state2_flips_state_to_one() {
    // R3 命中侧：state=2 但源在盘且缓存命中 → Ready 且 DB 翻 1（不再保留脏占位）
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("hit2.jpg");
    write_jpg(&src, 800, 600);
    let db_dir = tempfile::tempdir().unwrap();
    let _ = thumbs::thumb_file(db_dir.path(), &src, 256).unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &src);
    let database = common::open_db(db_dir.path());
    database.set_thumb_state(id, 2).unwrap();
    drop(database);

    let got = ipc::thumb::fetch_asset_thumb(&state, id, 256).unwrap();
    assert!(matches!(got, ipc::thumb::ThumbOutcome::Ready { .. }));
    let database = common::open_db(db_dir.path());
    let (.., state_val) = database.thumb_info_by_id(id).unwrap().unwrap();
    assert_eq!(state_val, 1, "命中即治愈：state 2 → 1");
}

#[test]
fn missing_outcome_serializes_camel_case_shape() {
    // 前端契约：Missing 变体序列化形状（前端 lane 按此对接）
    let null_shape = serde_json::to_value(ipc::thumb::ThumbOutcome::Missing {
        cached_path: None,
    })
    .unwrap();
    assert_eq!(null_shape["status"], "missing");
    assert!(null_shape["cachedPath"].is_null());
    assert_eq!(null_shape.as_object().unwrap().len(), 2, "仅 status+cachedPath 两键");

    let path_shape = serde_json::to_value(ipc::thumb::ThumbOutcome::Missing {
        cached_path: Some(r"I:\db\thumbs\256\abc-123.jpg".into()),
    })
    .unwrap();
    assert_eq!(path_shape["status"], "missing");
    assert_eq!(path_shape["cachedPath"], r"I:\db\thumbs\256\abc-123.jpg");
}

#[test]
fn cached_without_source_picks_newest_generation_by_prefix() {
    // 源缺失后 cached() 必 None（拿不到 mtime）；cached_without_source 按
    // <xxh64 前缀> 扫档位目录恢复既有缓存，多代（历史 mtime）取最新。
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("gen.jpg");
    write_jpg(&src, 800, 600);
    let db_dir = tempfile::tempdir().unwrap();
    let v1 = thumbs::thumb_file(db_dir.path(), &src, 256).unwrap();
    std::thread::sleep(Duration::from_millis(1100)); // 缓存键 mtime 粒度是秒
    write_jpg(&src, 640, 480);
    let v2 = thumbs::thumb_file(db_dir.path(), &src, 256).unwrap();
    assert_ne!(v1, v2, "mtime 变化产生新一代缓存（同前缀不同后缀）");

    // 源在盘：cached() 精确命中最新代
    assert_eq!(
        thumbs::cached(db_dir.path(), &src, 256).as_deref(),
        Some(v2.as_str())
    );
    // 删源：cached() None；前缀扫描恢复**最新**一代
    delete_source(&src);
    assert!(
        thumbs::cached(db_dir.path(), &src, 256).is_none(),
        "源缺失时精确键算不出（无 mtime）"
    );
    assert_eq!(
        thumbs::cached_without_source(db_dir.path(), &src, 256).as_deref(),
        Some(v2.as_str()),
        "前缀扫描应取最新代而非旧代"
    );
    // 无任何缓存的源 / 不可解码扩展 → None
    let other = src_dir.path().join("never.jpg");
    assert!(thumbs::cached_without_source(db_dir.path(), &other, 256).is_none());
    let txt = src_dir.path().join("a.txt");
    assert!(thumbs::cached_without_source(db_dir.path(), &txt, 256).is_none());
}
