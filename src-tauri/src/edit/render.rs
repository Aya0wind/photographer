//! 渲染管线（阶段 D）：解码 → 方向转正 → 旋转 → 裁剪 → 文字/笔迹 → 缩放。
//!
//! 解码路径与缩略图/预览管线同源（契约：方向归一化必须一致）：
//! - JPEG：image crate 全量解码（导出要全分辨率，不走 turbojpeg 缩放快道）；
//! - RAW：rawler 完整显影（[`crate::thumbs`] 2048 档同款 API），失败回退
//!   相机内嵌最大 JPEG（[`crate::thumbs::raw_preview_jpeg`]）再全量解码；
//! - 方向：源文件头 EXIF Orientation 1-8 → [`crate::thumbs::apply_orientation`]
//!   像素级转正（转正后的像素坐标才是配方坐标系的地基）。
//!
//! 旋转：rotateQuarter 0..3 顺时针 90° 步进，**先旋转后裁剪**（契约）。
//! 缩放：image crate Lanczos3，**只缩不放**（放大无意义）。

use std::io::Cursor;
use std::path::Path;

use image::{DynamicImage, RgbImage};

use super::recipe::EditRecipe;
use super::text::{draw_brush_stroke, draw_text_layer};
use crate::metadata::exif_lite;
use crate::thumbs;

/// 渲染产物：最终画布（已缩放）+ 解码阶段的诊断信息。
#[derive(Debug)]
pub struct Rendered {
    pub image: RgbImage,
}

/// 源文件头 1MB 的 EXIF Orientation（1-8；解析失败按 1）。
fn orientation_of(src: &Path) -> u32 {
    use std::io::Read;
    let Ok(mut file) = std::fs::File::open(src) else {
        return 1;
    };
    let mut buf = Vec::new();
    if (&mut file).take(1024 * 1024).read_to_end(&mut buf).is_err() {
        return 1;
    }
    exif_lite::parse_orientation(&buf).unwrap_or(1)
}

/// 全量解码 + 方向转正（画布的绝对坐标系自此确立）。
fn decode_upright(src: &Path) -> Result<RgbImage, String> {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let orientation = orientation_of(src);
    let decoded: Option<DynamicImage> = if thumbs::RAW_EXTS.contains(&ext.as_str()) {
        develop_raw(src).or_else(|| {
            thumbs::raw_preview_jpeg(src).and_then(|jpeg| {
                image::ImageReader::new(Cursor::new(&jpeg[..]))
                    .with_guessed_format()
                    .ok()
                    .and_then(|r| r.decode().ok())
            })
        })
    } else {
        image::ImageReader::open(src)
            .map_err(|e| format!("打开源文件失败: {e}"))
            .ok()
            .and_then(|r| r.decode().ok())
    };
    let img = decoded
        .map(|img| img.to_rgb8())
        .filter(|img| img.width() > 0 && img.height() > 0)
        .ok_or_else(|| format!("源文件无法解码: {}", src.display()))?;
    Ok(thumbs::apply_orientation(img, orientation))
}

/// RAW 完整显影（与 thumbs 2048 档同一条 rawler 路径，不缩放）。
fn develop_raw(src: &Path) -> Option<DynamicImage> {
    use rawler::imgop::develop::RawDevelop;
    let raw = rawler::decode_file(src).ok()?;
    let developed = RawDevelop::default().develop_intermediate(&raw).ok()?;
    developed.to_dynamic_image()
}

/// rotateQuarter 0..3 → 顺时针 90° 步进（0=不动）。
fn rotate_quarter(img: RgbImage, quarter: u32) -> RgbImage {
    use image::imageops;
    match quarter % 4 {
        1 => imageops::rotate90(&img),  // 顺时针 90°
        2 => imageops::rotate180(&img),
        3 => imageops::rotate270(&img), // 顺时针 270°
        _ => img,
    }
}

/// 归一化裁剪（x/w 对宽、y/h 对高），夹取到图像边界。
fn crop_normalized(img: &RgbImage, crop: super::recipe::CropRect) -> RgbImage {
    let (w, h) = (u64::from(img.width()), u64::from(img.height()));
    let x0 = (crop.x * w as f64).floor().clamp(0.0, w as f64) as u64;
    let y0 = (crop.y * h as f64).floor().clamp(0.0, h as f64) as u64;
    let x1 = ((crop.x + crop.w) * w as f64).ceil().clamp(x0 as f64, w as f64) as u64;
    let y1 = ((crop.y + crop.h) * h as f64).ceil().clamp(y0 as f64, h as f64) as u64;
    let (cw, ch) = ((x1 - x0) as u32, (y1 - y0) as u32);
    if cw == 0 || ch == 0 {
        return img.clone(); // 极端夹取（如 1px 高图）：不裁
    }
    image::imageops::crop_imm(img, x0 as u32, y0 as u32, cw, ch).to_image()
}

/// 完整渲染：解码 → 转正 → 旋转 → 裁剪 → 文字/笔迹 → 只缩不放。
/// `long_edge` 为**已合并的终值**（导出 options → 配方 output 的缺省序，
/// 见 [`crate::edit::export`]；渲染层不再看配方里的 output.longEdge）。
pub fn render_recipe(src: &Path, recipe: &EditRecipe, long_edge: Option<u32>) -> Result<Rendered, String> {
    let upright = decode_upright(src)?;
    let rotated = rotate_quarter(upright, recipe.rotate_quarter);
    let mut canvas = match recipe.crop {
        Some(crop) => crop_normalized(&rotated, crop),
        None => rotated,
    };
    for layer in &recipe.text_layers {
        draw_text_layer(&mut canvas, layer);
    }
    for stroke in &recipe.brush_strokes {
        draw_brush_stroke(&mut canvas, stroke);
    }
    let canvas = resize_long_edge(canvas, long_edge);
    Ok(Rendered { image: canvas })
}

/// 长边限制的缩放（Lanczos3，只缩不放；None/大于源 = 原尺寸）。
pub fn resize_long_edge(img: RgbImage, long_edge: Option<u32>) -> RgbImage {
    let Some(limit) = long_edge else {
        return img;
    };
    let (w, h) = (img.width(), img.height());
    let long_side = w.max(h);
    if long_side <= limit {
        return img; // 只缩不放
    }
    let scale = f64::from(limit) / f64::from(long_side);
    let nw = ((f64::from(w) * scale).round() as u32).max(1);
    let nh = ((f64::from(h) * scale).round() as u32).max(1);
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
}

/// JPEG 编码（sRGB，质量 1..=100）。
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality);
    img.write_with_encoder(encoder)
        .map_err(|e| format!("JPEG 编码失败: {e}"))?;
    Ok(out)
}
