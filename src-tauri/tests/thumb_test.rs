//! 缩略图服务测试（thumb_get 的核心 `thumbs::thumb_file`）：生成/命中/尺寸分档/
//! RAW 拒绝/坏文件不写缓存/mtime 重生成/并发合并（同 path+size 只解码一次）。
//! 缓存落库 dbDir/thumbs，不污染源目录。

#[path = "../src/thumbs/mod.rs"]
#[allow(dead_code)]
mod thumbs;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use image::DynamicImage;

/// 造一张指定尺寸的渐变 JPG。
fn write_jpg(path: &Path, w: u32, h: u32) {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    let img = DynamicImage::ImageRgb8(img);
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    img.write_with_encoder(encoder).unwrap();
    fs::write(path, buf).unwrap();
}

fn db_dir() -> PathBuf {
    tempfile::tempdir().unwrap().path().to_path_buf()
}

#[test]
fn generates_cache_returns_path_and_second_call_hits() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("IMG_0001.jpg");
    write_jpg(&src, 1024, 768);
    let db = db_dir();

    let first = thumbs::thumb_file(&db, &src, 256).expect("应生成缩略图");
    let cache = PathBuf::from(&first);
    assert!(cache.exists(), "缓存文件必须落盘: {first}");
    assert!(
        cache.starts_with(db.join("thumbs")),
        "缓存必须在 dbDir/thumbs 下: {first}"
    );
    assert!(
        cache.to_string_lossy().contains("256"),
        "按尺寸分档目录: {first}"
    );

    // 尺寸拟合：最长边 <= 256
    let (w, h) = image::image_dimensions(&cache).unwrap();
    assert!(w.max(h) <= 256, "拟合 256: {w}x{h}");
    assert!(
        w.max(h) >= 200,
        "不应过度缩小（1024 宽源应缩到 ~256）: {w}x{h}"
    );

    // 二次调用命中同一缓存文件，且不重写（mtime 不变 = 未重新生成）
    let meta_before = fs::metadata(&cache).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let second = thumbs::thumb_file(&db, &src, 256).expect("二次调用应命中");
    assert_eq!(PathBuf::from(&second), cache, "命中返回同一缓存路径");
    let meta_after = fs::metadata(&cache).unwrap();
    assert_eq!(
        meta_before.modified().unwrap(),
        meta_after.modified().unwrap(),
        "命中不得重新生成（mtime 不变）"
    );

    // 源文件不被改动（只读管线）
    assert!(src.exists());
    assert!(fs::read(&src).unwrap().len() > 10_000, "源 JPG 原样保留");
}

#[test]
fn size_snaps_to_supported_tiers() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("a.png");
    let img = DynamicImage::new_rgb8(900, 300);
    img.save_with_format(&src, image::ImageFormat::Png).unwrap();
    let db = db_dir();

    // 300 → 就近 256 档；512 显式档；两者不同缓存文件
    let a = thumbs::thumb_file(&db, &src, 300).expect("300 应就近 256 档");
    let b = thumbs::thumb_file(&db, &src, 512).expect("512 档");
    assert!(a.contains("256"), "300 就近 256: {a}");
    assert!(b.contains("512"), "{b}");
    assert_ne!(a, b);
    let (w, h) = image::image_dimensions(Path::new(&b)).unwrap();
    assert_eq!(w.max(h), 512, "PNG 缩放到 512 档");
}

#[test]
fn raw_and_video_return_none_without_cache() {
    let src_dir = tempfile::tempdir().unwrap();
    let db = db_dir();
    for name in ["DSC_0001.NEF", "IMG.ARW", "M_0001.MP4", "clip.MOV"] {
        let src = src_dir.path().join(name);
        fs::write(&src, b"not really media").unwrap();
        assert!(
            thumbs::thumb_file(&db, &src, 256).is_none(),
            "RAW/视频 v1 必须返回 null: {name}"
        );
    }
    // 不存在路径 / 目录也不崩
    assert!(thumbs::thumb_file(&db, &src_dir.path().join("missing.jpg"), 256).is_none());
    assert!(thumbs::thumb_file(&db, src_dir.path(), 256).is_none());
    // 未写任何缓存
    let thumbs_root = db.join("thumbs");
    assert!(
        !thumbs_root.exists() || fs::read_dir(&thumbs_root).unwrap().next().is_none(),
        "失败不得写缓存"
    );
}

#[test]
fn corrupt_file_returns_none_and_no_bad_cache() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("broken.jpg");
    fs::write(&src, b"\xFF\xD8\xFF garbage not a jpeg").unwrap();
    let db = db_dir();
    assert!(
        thumbs::thumb_file(&db, &src, 256).is_none(),
        "坏文件 → null"
    );
    assert!(
        !db.join("thumbs").join("256").exists()
            || fs::read_dir(db.join("thumbs").join("256")).unwrap().count() == 0,
        "失败不得留下坏缓存文件"
    );
    // 修复源后可成功（坏缓存不污染后续）
    write_jpg(&src, 640, 480);
    assert!(
        thumbs::thumb_file(&db, &src, 256).is_some(),
        "源修复后应可生成"
    );
}

#[test]
fn mtime_change_regenerates() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("r.jpg");
    write_jpg(&src, 500, 500);
    let db = db_dir();
    let v1 = thumbs::thumb_file(&db, &src, 256).expect("第一次生成");
    // 键的 mtime 粒度是秒：跨秒改写源才构成新键（生产语义：导入目标写一次不变）
    std::thread::sleep(Duration::from_millis(1100));
    write_jpg(&src, 700, 300);
    let v2 = thumbs::thumb_file(&db, &src, 256).expect("mtime 变化后重新生成");
    assert_ne!(v1, v2, "键含 mtime，缓存路径必须不同");
    assert!(Path::new(&v1).exists(), "旧缓存保留（不主动清理）");
    assert!(Path::new(&v2).exists());
}

#[test]
fn concurrent_same_request_decodes_once() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("c.jpg");
    write_jpg(&src, 2000, 1500); // 大图，拉长解码时间保证并发窗口
    let db = Arc::new(db_dir());
    let before = thumbs::DECODE_COUNT.load(std::sync::atomic::Ordering::SeqCst);

    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let db = Arc::clone(&db);
            let src = src.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                thumbs::thumb_file(&db, &src, 256)
            })
        })
        .collect();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();

    assert!(results.iter().all(|r| r.is_some()), "全部成功: {results:?}");
    let first = results[0].as_ref().unwrap().clone();
    assert!(
        results.iter().all(|r| r.as_deref() == Some(first.as_str())),
        "并发请求返回同一缓存路径"
    );
    let decoded = thumbs::DECODE_COUNT.load(std::sync::atomic::Ordering::SeqCst) - before;
    assert_eq!(
        decoded, 1,
        "同 path+size 并发必须合并为一次解码（in-flight dedup）"
    );
}
