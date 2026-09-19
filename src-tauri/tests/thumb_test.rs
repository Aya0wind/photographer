//! 缩略图服务测试（thumb_get_by_path 的核心 `thumbs::thumb_file`）：生成/命中/尺寸分档/
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

// ---------------------------------------------------------------------------
// turbojpeg 缩放解码快路径（提速轮）
// ---------------------------------------------------------------------------

#[test]
fn pick_jpeg_scale_tier_matrix() {
    use thumbs::pick_jpeg_scale;
    // 大图（61MP 级 9504px）→ 1/8（1188px 仍 ≥256，取最小图）
    assert_eq!(pick_jpeg_scale(9504, 6336, 256), 1);
    // 大图 512 档 → 1/8（1188 ≥ 512）
    assert_eq!(pick_jpeg_scale(9504, 6336, 512), 1);
    // 中图 1024x768 → 256：2/8=256x192（高不足）→ 3/8=384x288 ✓
    assert_eq!(pick_jpeg_scale(1024, 768, 256), 3);
    // 中图 1000px → 256：1/8=125 ✗ 2/8=250 ✗ 3/8=375 ✓ → 3/8
    assert_eq!(pick_jpeg_scale(1000, 750, 256), 3);
    // 中图 2000px → 512：1/8=250 ✗ 2/8=500 ✗ 3/8=750 ✓ → 3/8
    assert_eq!(pick_jpeg_scale(2000, 1500, 512), 3);
    // 小图（200px < 256）→ 原尺寸 8/8（避免上采样糊）
    assert_eq!(pick_jpeg_scale(200, 150, 256), 8);
    assert_eq!(pick_jpeg_scale(300, 200, 512), 8);
    // 恰好边界：2048px → 256 档 1/8=256 ✓ → 1
    assert_eq!(pick_jpeg_scale(2048, 2048, 256), 1);
}

#[test]
fn permit_count_dynamic_matrix() {
    // 核心数直通（用户定案 2026-09-19：索引任务吃满硬件）：4 核→4、
    // 8 核→8、16 核→16；0 防御为 1
    assert_eq!(thumbs::permit_count_for_test(4), 4);
    assert_eq!(thumbs::permit_count_for_test(8), 8);
    assert_eq!(thumbs::permit_count_for_test(16), 16);
    assert_eq!(thumbs::permit_count_for_test(0), 1);
}

#[test]
fn jpeg_uses_turbo_fast_path_png_does_not() {
    let src_dir = tempfile::tempdir().unwrap();
    let db = db_dir();
    let before = thumbs::TURBO_DECODES.load(std::sync::atomic::Ordering::SeqCst);

    // JPG → 快路径命中（计数器进程级共享、测试并行执行，取 ≥ 断言防串扰）
    let jpg = src_dir.path().join("fast.jpg");
    write_jpg(&jpg, 1024, 768);
    let out = thumbs::thumb_file(&db, &jpg, 256).expect("JPG 应生成");
    let (w, h) = image::image_dimensions(Path::new(&out)).unwrap();
    assert!(w.max(h) <= 256);
    assert!(
        thumbs::TURBO_DECODES.load(std::sync::atomic::Ordering::SeqCst) > before,
        "JPG 必须走 turbojpeg 快路径"
    );

    // PNG → 不走快路径（image crate）但正常生成
    let png = src_dir.path().join("slow.png");
    let img = DynamicImage::new_rgb8(900, 300);
    img.save_with_format(&png, image::ImageFormat::Png).unwrap();
    assert!(thumbs::thumb_file(&db, &png, 256).is_some());
}

#[test]
fn truncated_jpeg_falls_back_without_panic() {
    // turbojpeg 拒收 + image crate 也解不了 → null（不 panic）；
    // 半合法 JPEG（turbo 失败但 image 能解）→ 仍出图（回退兜底）
    let src_dir = tempfile::tempdir().unwrap();
    let db = db_dir();
    let bad = src_dir.path().join("trunc.jpg");
    let mut data = fs::read(src_dir.path().join({
        // 先造一张合法 jpg 再截断
        let good = src_dir.path().join("good.jpg");
        write_jpg(&good, 640, 480);
        good
    }))
    .unwrap();
    data.truncate(data.len() / 3); // 砍掉 2/3
    fs::write(&bad, data).unwrap();

    // 截断 JPEG：turbo 与 image 两条路径要么失败（→None）要么侥幸解出
    //（→Some）都可接受——关键断言：不 panic、无 .tmp 残留、重复调用稳定
    let _first = thumbs::thumb_file(&db, &bad, 256);
    let _second = thumbs::thumb_file(&db, &bad, 256);
    assert!(
        thumbs::find_tmp_residue(&db).is_empty(),
        "失败路径不得留 .tmp 半成品"
    );
}

/// 基准：全量解码 vs 缩放解码（手动跑：cargo test --test thumb_test bench_ --ignored --nocapture）。
#[test]
#[ignore = "基准用例（报告数字）"]
fn bench_full_decode_vs_scaled() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("big.jpg");
    // 9504x6336（61MP 级），高频噪声内容（贴近真实照片的熵解码成本，
    // 平滑渐变对解码器过于友好测不出差异）
    let mut rng: u32 = 0x1234_5678;
    let mut img = image::RgbImage::new(9504, 6336);
    for (_x, _y, px) in img.enumerate_pixels_mut() {
        // xorshift 伪随机（无依赖）
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        *px = image::Rgb([(rng >> 24) as u8, (rng >> 16) as u8, (rng >> 8) as u8]);
    }
    let dynimg = DynamicImage::ImageRgb8(img);
    let mut buf = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 92);
    dynimg.write_with_encoder(enc).unwrap();
    fs::write(&src, buf).unwrap();
    println!(
        "源 JPEG 大小: {:.1} MB",
        fs::read(&src).unwrap().len() as f64 / 1e6
    );

    // 全量（image crate）计时
    let t0 = std::time::Instant::now();
    let data = fs::read(&src).unwrap();
    let full = image::ImageReader::new(std::io::Cursor::new(&data))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap();
    let full_thumb = full.thumbnail(256, 256).to_rgb8();
    let t_full = t0.elapsed();
    println!(
        "全量解码+缩放: {:?}（{}x{}）",
        t_full,
        full_thumb.width(),
        full_thumb.height()
    );

    // 缩放（turbojpeg）计时
    let t0 = std::time::Instant::now();
    let mut out = Vec::new();
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80);
    let fast = thumbs::bench_scaled_rgb(&src, 256).expect("快路径应成功");
    fast.write_with_encoder(enc).unwrap();
    let t_fast = t0.elapsed();
    println!(
        "缩放解码+编码: {:?}（{}x{}）",
        t_fast,
        fast.width(),
        fast.height()
    );
    println!(
        "提速倍数: {:.1}x",
        t_full.as_secs_f64() / t_fast.as_secs_f64()
    );
}

// ---------------------------------------------------------------------------
// RAW 内嵌预览提取（M3.5）：SOI 扫描 → turbojpeg 管线，同一缓存规则
// ---------------------------------------------------------------------------

#[test]
fn raw_preview_extracted_and_cached_across_tiers() {
    let db = db_dir();
    // 真实 NEF（已知样例单文件直连，不遍历目录）：内嵌 JPEG 预览
    let nef = Path::new(r"I:\SmartPhoto-test-e2e\收纳\2025\06-07\DSC_0176.NEF");
    if !nef.is_file() {
        eprintln!("skip: 样例不存在 {}", nef.display());
        return;
    }
    let first = thumbs::thumb_file(&db, nef, 256).expect("RAW 应提取出缩略图");
    let cache = PathBuf::from(&first);
    assert!(
        cache.starts_with(db.join("thumbs")),
        "缓存必须在 dbDir/thumbs: {first}"
    );
    let (w, h) = image::image_dimensions(&cache).unwrap();
    assert!(w.max(h) <= 256, "拟合 256 档: {w}x{h}");

    // 三档各自缓存
    for tier in [512u16, 2048] {
        let p = thumbs::thumb_file(&db, nef, tier).expect("高档应生成");
        let (w, h) = image::image_dimensions(Path::new(&p)).unwrap();
        assert!(w.max(h) <= u32::from(tier), "拟合 {tier}: {w}x{h}");
    }

    // 命中：二次调用同路径不重提取（mtime 不变）
    let meta_before = fs::metadata(&cache).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    let second = thumbs::thumb_file(&db, nef, 256).expect("二次应命中");
    assert_eq!(PathBuf::from(&second), cache);
    assert_eq!(
        meta_before.modified().unwrap(),
        fs::metadata(&cache).unwrap().modified().unwrap(),
        "命中不得重新生成"
    );
}

#[test]
fn corrupt_raw_returns_none_without_cache() {
    let src_dir = tempfile::tempdir().unwrap();
    let src = src_dir.path().join("bad.NEF");
    // 无 SOI 结构的垃圾
    fs::write(&src, b"not a raw file at all, no jpeg inside").unwrap();
    let db = db_dir();
    assert!(thumbs::thumb_file(&db, &src, 256).is_none());
    assert!(thumbs::raw_preview_jpeg(&src).is_none());
    assert!(!db.join("thumbs").join("256").join("x").exists());

    // 有 SOI 但无 EOI（截断）→ None
    fs::write(&src, [0xFF, 0xD8, 0xFF, 0xD8, 0x00, 0x00]).unwrap();
    assert!(thumbs::thumb_file(&db, &src, 256).is_none());
}
