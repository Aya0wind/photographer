//! M8 视频支持：ffmpeg 侧车海报提取 + thumbs 管线视频路由 + 视频资产
//! thumb_state 解锁。素材用侧车自产（lavfi testsrc），不依赖真库。
//! 侧车缺失的环境（新克隆未下载）软跳过集成件——单测面（路由/降级/参数
//! 校验）仍全量执行。open_with_system 的参数校验在 ipc/system.rs 单测。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{open_db, run_engine};

/// 侧车可用性门（缺失 = 软跳过集成件，打印原因）。
fn sidecar() -> Option<PathBuf> {
    let exe = videos::ffmpeg_path();
    if exe.is_none() {
        eprintln!(
            "[跳过] ffmpeg 侧车未安装（src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe）"
        );
    }
    exe
}

/// lavfi testsrc 自产测试视频（2s@10fps 160x120 yuv420p H.264 mp4）。
/// duration=2 保证 `-ss 1` 落在有效帧区间。
fn make_test_video(exe: &Path, out: &Path) {
    let status = Command::new(exe)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=2:size=160x120:rate=10",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(out)
        .status()
        .expect("spawn ffmpeg lavfi");
    assert!(status.success(), "lavfi 素材生成失败: {status}");
    assert!(out.is_file());
}

#[test]
fn poster_extraction_produces_tiered_jpeg_cache() {
    let Some(exe) = sidecar() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let mp4 = dir.path().join("C0121.MP4");
    make_test_video(&exe, &mp4);

    let poster = videos::video_poster(&db_dir, &mp4, 256).expect("海报应生成");
    assert!(
        poster.starts_with(db_dir.join("thumbs").join("video-256-v1")),
        "缓存落 thumbs 根的代际档位: {}",
        poster.display()
    );
    assert!(poster.is_file());

    // JPEG 魔数（侧车产物必须是可解码 JPEG）
    let head = std::fs::read(&poster).unwrap()[..3].to_vec();
    assert_eq!(head, vec![0xFF, 0xD8, 0xFF], "JPEG 头");

    // 尺寸档：160x120 源 → scale=256:-2（宽度对齐档位、高度偶数对齐）
    let (w, h) = image::image_dimensions(&poster).unwrap();
    assert_eq!(w, 256, "宽度 = 档宽");
    assert_eq!(h, 192, "4:3 高度（-2 偶数对齐）");

    // 命中探测（不 spawn）：同键返回同一路径
    let cached = videos::cached_poster(&db_dir, &mp4, 256).expect("缓存探测应命中");
    assert_eq!(PathBuf::from(cached), poster);
    assert!(
        videos::cached_poster(&db_dir, &mp4, 512).is_none(),
        "512 档未生成"
    );

    // 二次调用命中缓存：改 db_dir 无关变量不可行（键含 mtime），以
    // cached_poster 一致性 + video_poster 幂等返回佐证
    let again = videos::video_poster(&db_dir, &mp4, 256).expect("二次应命中");
    assert_eq!(again, poster);
}

#[test]
fn thumbs_pipeline_routes_video_extensions_to_poster() {
    let Some(exe) = sidecar() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().to_path_buf();
    let mp4 = dir.path().join("clip.mp4");
    make_test_video(&exe, &mp4);

    // 路由 + 就近归档（300 → 256 档；2048 档独立）
    let p256 = thumbs::thumb_file(&db_dir, &mp4, 300).expect("视频应出海报");
    assert!(p256.contains("video-256-v1"), "300 就近 256 档: {p256}");
    let p2048 = thumbs::thumb_file(&db_dir, &mp4, 2048).expect("2048 档海报");
    assert!(p2048.contains("video-2048-v1"), "{p2048}");
    assert_ne!(p256, p2048);

    // 快路径缓存探测同构
    let hit = thumbs::cached(&db_dir, &mp4, 256).expect("thumbs::cached 应命中视频海报");
    assert_eq!(hit, p256);

    // 廉价否决放行（入队前置检查）
    assert!(thumbs::is_decodable(&mp4));
}

#[test]
fn corrupt_or_short_video_fails_gracefully() {
    let Some(_exe) = sidecar() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    // 垃圾字节 .mp4：ffmpeg 解不了 → None（不 panic、不留半缓存）
    let bad = dir.path().join("bad.mp4");
    std::fs::write(&bad, b"this is not a video at all").unwrap();
    assert!(videos::video_poster(&db_dir, &bad, 256).is_none());
    // thumbs 管线同语义（路由到海报 → 失败 → None）
    assert!(thumbs::thumb_file(&db_dir, &bad, 256).is_none());
    // 缓存目录不留 .part.jpg 半产物
    let thumbs_root = db_dir.join("thumbs");
    if thumbs_root.is_dir() {
        let leftovers: Vec<_> = walkdir::WalkDir::new(&thumbs_root)
            .into_iter()
            .flatten()
            .filter(|e| {
                e.file_type().is_file() && e.file_name().to_string_lossy().contains(".part.")
            })
            .collect();
        assert!(leftovers.is_empty(), "不留半产物: {leftovers:?}");
    }
}

#[test]
fn import_keeps_video_pending_and_legacy_placeholder_unlocks() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    common::build_source(src.path()); // 含 MVI_0003.MP4

    let (_job, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(stats.done_files, 3);

    let db = open_db(db_dir.path());
    let state: i64 =
        db.0.query_row(
            "SELECT thumb_state FROM assets WHERE filename = 'MVI_0003.MP4'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, 0, "M8 起 video 不再永久占位（按需海报队列）");

    // 历史库自愈：手工打回 2（旧版本语义）→ 启动解锁复位
    db.0.execute(
        "UPDATE assets SET thumb_state = 2 WHERE filename = 'MVI_0003.MP4'",
        [],
    )
    .unwrap();
    assert_eq!(db.reset_video_thumb_placeholders().unwrap(), 1);
    let state: i64 =
        db.0.query_row(
            "SELECT thumb_state FROM assets WHERE filename = 'MVI_0003.MP4'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, 0, "历史 video 占位应被解锁");
    // 幂等：无 2 可复位
    assert_eq!(db.reset_video_thumb_placeholders().unwrap(), 0);
    // photo 的永久占位（不可解码）不受影响
    db.0.execute(
        "UPDATE assets SET thumb_state = 2 WHERE filename = 'IMG_0001.jpg'",
        [],
    )
    .unwrap();
    assert_eq!(
        db.reset_video_thumb_placeholders().unwrap(),
        0,
        "只动 video"
    );
}

/// open_with_system 参数校验（真打开会弹播放器窗口，只测错误路径）。
#[test]
fn open_with_system_rejects_missing_path() {
    let err = ipc::system::fetch_open_with_system(r"Z:\definitely\not\a\clip.mp4").unwrap_err();
    assert!(err.contains("不存在"), "明确报不存在: {err}");
    let dir = tempfile::tempdir().unwrap();
    let err = ipc::system::fetch_open_with_system(dir.path().to_str().unwrap()).unwrap_err();
    assert!(err.contains("不存在"), "目录不算可打开文件: {err}");
}
