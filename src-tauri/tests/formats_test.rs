//! 扩展图片格式接入测试（2026-09-29 批次）：
//! - devices：新格式扩展名白名单与魔数（avif/avis brand、JXL 裸码流/容器、
//!   mif1、ICO/CUR 头）——细节断言在 devices/mod.rs 模块内单测，此处走管线。
//! - thumbs：heic/heif（heif-oxide）、jxl（jxl-oxide）、avif（avif-decode/rav1d）
//!   真实样本 + image crate 现场生成的 bmp/ico/webp/tiff/gif 全部过
//!   thumb_file 缩略管线，产出合法 JPEG 缓存（≤ 目标边长）。
//! 夹具：tests/assets/formats/（nokiatech heic / libjxl testdata jxl /
//! libavif paris avif，均为公开测试样本）。

#[path = "../src/thumbs/mod.rs"]
#[allow(dead_code)]
mod thumbs;

#[path = "../src/metadata/mod.rs"]
#[allow(dead_code)]
mod metadata;

use std::path::{Path, PathBuf};

use image::DynamicImage;

/// 造一张渐变 RGB 图（512×512：缩到 256 档后最长边应恰为 256）。
fn gradient(w: u32, h: u32) -> DynamicImage {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    DynamicImage::ImageRgb8(img)
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/assets/formats")
        .join(name)
}

fn db_dir() -> PathBuf {
    tempfile::tempdir().unwrap().path().to_path_buf()
}

/// 断言一个源文件过 thumb_file 出合法 JPEG 缩略缓存。
/// `min_edge`：期望最长边下界（源足够大时应缩到 ~256；未知小图传 1）。
fn assert_thumb_ok(src: &Path, min_edge: u32) {
    let db = db_dir();
    let out = thumbs::thumb_file(&db, src, 256)
        .unwrap_or_else(|| panic!("{} 应生成缩略图", src.display()));
    let cache = PathBuf::from(&out);
    assert!(cache.exists(), "缓存文件必须落盘: {out}");
    assert!(
        cache.starts_with(db.join("thumbs")),
        "缓存必须在 dbDir/thumbs 下: {out}"
    );
    let (w, h) = image::image_dimensions(&cache).expect("缓存应为可解析图片（JPEG）");
    let max = w.max(h);
    assert!(max <= 256, "拟合 256: {w}x{h}");
    assert!(max >= min_edge, "不应过度缩小: {w}x{h}（下界 {min_edge}）");
    // 源文件只读不动
    assert!(src.exists());
}

// ---------------------------------------------------------------------------
// 真实样本（下载夹具）：三种外部解码器各一
// ---------------------------------------------------------------------------

#[test]
fn thumb_heic_sample_via_heif_oxide() {
    let src = fixture("sample.heic"); // 1440×960 网格拼贴（nokiatech autumn）
    assert!(src.exists(), "夹具缺失: {}", src.display());
    assert_thumb_ok(&src, 200);
}

#[test]
fn thumb_jxl_sample_via_jxl_oxide() {
    let src = fixture("sample.jxl"); // 裸码流 FF 0A（libjxl testdata）
    assert!(src.exists(), "夹具缺失: {}", src.display());
    assert_thumb_ok(&src, 1);
}

#[test]
fn thumb_avif_sample_via_avif_decode() {
    let src = fixture("sample.avif"); // ftyp avif（libavif paris，8bit RGB）
    assert!(src.exists(), "夹具缺失: {}", src.display());
    assert_thumb_ok(&src, 200);
}

// ---------------------------------------------------------------------------
// image crate 原生格式（现场生成）：位图全家桶
// ---------------------------------------------------------------------------

#[test]
fn thumb_image_crate_formats() {
    let big = gradient(512, 512); // 缩到 256 档后最长边应恰为 256
                                  // ICO 目录宽度字段 u8（0=256）>256 无法编码，且 image 的 ICO 解码器
                                  // 要求内嵌 PNG 为 RGBA → 用 128×128 RGBA 源
    let small = DynamicImage::ImageRgba8(gradient(128, 128).to_rgba8());
    let cases: &[(&str, image::ImageFormat, &DynamicImage)] = &[
        ("bmp", image::ImageFormat::Bmp, &big),
        ("ico", image::ImageFormat::Ico, &small),
        ("webp", image::ImageFormat::WebP, &big),
        ("gif", image::ImageFormat::Gif, &big),
        ("tif", image::ImageFormat::Tiff, &big),
    ];
    let dir = tempfile::tempdir().unwrap();
    for (ext, format, img) in cases {
        let src = dir.path().join(format!("gen.{ext}"));
        img.save_with_format(&src, *format).expect("编码应成功");
        // ico 源 128×128：thumbnail 不放大，最长边仍为 128
        let min_edge = if *ext == "ico" { 100 } else { 200 };
        assert_thumb_ok(&src, min_edge);
    }
}

/// CUR 走 ICO 解码器（魔数嗅探）：`00 00 02 00` 头 + .cur 扩展名。
#[test]
fn thumb_cur_via_ico_decoder_guess() {
    let dir = tempfile::tempdir().unwrap();
    let ico = dir.path().join("tmp.ico");
    DynamicImage::ImageRgba8(gradient(64, 64).to_rgba8())
        .save_with_format(&ico, image::ImageFormat::Ico)
        .expect("编码应成功");
    // 复用 ICO 编码产物，仅把头部 icon_type（偏移 2 的 u16）改为 2(CUR)：
    // 结构与 ICO 完全同构（image 的 ICO 解码器同时认领两种头）。
    let mut bytes = std::fs::read(&ico).unwrap();
    bytes[2] = 2;
    let src = dir.path().join("cursor.cur");
    std::fs::write(&src, bytes).unwrap();
    assert_thumb_ok(&src, 1);
}

// ---------------------------------------------------------------------------
// 门禁：白名单与缓存命中
// ---------------------------------------------------------------------------

#[test]
fn decodable_exts_cover_new_formats() {
    for ext in [
        "heic", "heif", "jxl", "avif", "bmp", "ico", "cur", "webp", "gif", "tif", "tiff",
    ] {
        assert!(
            thumbs::DECODABLE_EXTS.contains(&ext),
            "DECODABLE_EXTS 应包含 {ext}"
        );
    }
}

/// 新格式缓存命中路径与 JPG 一致（第二次调用不重解码、路径不变）。
#[test]
fn extended_format_cache_hit() {
    let src = fixture("sample.avif");
    let db = db_dir();
    let first = thumbs::thumb_file(&db, &src, 256).expect("首次生成");
    let second = thumbs::thumb_file(&db, &src, 256).expect("二次命中");
    assert_eq!(first, second, "命中返回同一缓存路径");
}
