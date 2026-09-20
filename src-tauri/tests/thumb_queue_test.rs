//! 按需缩略图管线（M3）：asset_thumb_get(asset_id) 缓存命中直返 / 未命中入队
//! 后台生成 + ThumbnailReady 事件、同 (asset,size) 去抖、有界队列满丢弃、
//! RAW/缺资产不入队、队列 Drop 停工。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
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
            sha256: [0; 32],
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
fn raw_and_missing_assets_return_none_without_enqueue() {
    let src_dir = tempfile::tempdir().unwrap();
    let raw = src_dir.path().join("VID_0001.MP4");
    std::fs::write(&raw, b"video-bytes").unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let (state, id) = state_with_asset(db_dir.path(), &raw);

    // 视频不可解码：Unavailable 且不入队
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
