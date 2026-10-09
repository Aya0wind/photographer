//! 上游 PhotoCraft 的唯一接入层；不依赖图库、数据库或存储目录。
//! v1 配方是持久化真值，每次从未修改的文档重建，避免滑块累计调整。

use image::RgbImage;
use photocraft_engine::{
    doc::{ColorMode, Document, Layer, LayerContent, PixelFormat, Rect, SampleType, Size, Surface},
    Session,
};
use serde_json::json;

use super::recipe::{Adjustments, EditRecipe};

pub fn document(image: &RgbImage) -> Result<Document, String> {
    let (w, h) = image.dimensions();
    if w == 0 || h == 0 || w > i32::MAX as u32 || h > i32::MAX as u32 {
        return Err("编辑图像尺寸无效".into());
    }
    // 直接填充上游分块像素，无损复用现有 RAW/EXIF 解码；无需 PNG 中转或整图 RGBA 副本。
    let mut surface = Surface::new(PixelFormat::new(ColorMode::Rgb, SampleType::U8, true));
    let mut row = vec![255u8; w as usize * 4];
    for (y, source) in image.as_raw().chunks_exact(w as usize * 3).enumerate() {
        for (rgb, rgba) in source.chunks_exact(3).zip(row.chunks_exact_mut(4)) {
            rgba[..3].copy_from_slice(rgb);
        }
        surface.write_interleaved(Rect::from_xywh(0, y as i32, w, 1), &row);
    }
    let mut document = Document::new("Photo", Size::new(w, h), ColorMode::Rgb, SampleType::U8);
    document
        .layers
        .push(Layer::new("Photo", LayerContent::Raster(surface)));
    Ok(document)
}

fn execute(session: &mut Session, command: &str, params: serde_json::Value) -> Result<(), String> {
    session
        .execute(command, params)
        .map(|_| ())
        .map_err(|e| format!("PhotoCraft {command}: {e}"))
}

fn basic_adjustments(session: &mut Session, values: Option<Adjustments>) -> Result<(), String> {
    let Some(a) = values else { return Ok(()) };
    if a.brightness != 0.0 || a.contrast != 0.0 {
        // UI 保持 -100..100；上游现代对比度下限为 -50。
        execute(
            session,
            "layer.newAdjustmentLayer.brightnessContrast",
            json!({
                "brightness": a.brightness, "contrast": if a.contrast < 0.0 { a.contrast / 2.0 } else { a.contrast }
            }),
        )?;
    }
    if a.saturation != 0.0 {
        execute(
            session,
            "layer.newAdjustmentLayer.hueSaturation",
            json!({"saturation": a.saturation}),
        )?;
    }
    Ok(())
}

fn adjustments(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    if let Some(a) = &recipe.advanced {
        if a.exposure != 0.0 {
            execute(
                session,
                "layer.newAdjustmentLayer.exposure",
                json!({"exposure": a.exposure}),
            )?;
        }
    }
    basic_adjustments(session, recipe.adjustments)?;
    if let Some(a) = &recipe.advanced {
        if a.vibrance != 0.0 {
            execute(
                session,
                "layer.newAdjustmentLayer.vibrance",
                json!({"vibrance": a.vibrance}),
            )?;
        }
        if a.temperature != 0.0 || a.tint != 0.0 {
            // RGB 中间调色彩平衡，不冒充 RAW 白平衡或绝对色温。
            execute(
                session,
                "layer.newAdjustmentLayer.colorBalance",
                json!({
                    "midtones": [a.temperature / 2.0, -a.tint / 2.0, -a.temperature / 2.0],
                    "preserveLuminosity": true
                }),
            )?;
        }
        if !a.curves.is_empty() {
            execute(
                session,
                "layer.newAdjustmentLayer.curves",
                json!({"points": a.curves}),
            )?;
        }
    }
    Ok(())
}

pub fn adjusted_document(base: &Document, recipe: &EditRecipe) -> Result<Document, String> {
    let mut session = Session::new();
    session.add_document(base.clone(), None);
    adjustments(&mut session, recipe)?;
    session
        .active()
        .map(|s| (*s.doc).clone())
        .ok_or("编辑文档不存在".into())
}

pub fn composite(doc: &Document) -> Result<RgbImage, String> {
    // 按行带合成，避免全尺寸 float RGBA 缓冲与成品 RGB 同时占用大量内存。
    let mut output = RgbImage::new(doc.size.width, doc.size.height);
    photocraft_compose::render_bands(doc, doc.bounds(), 64, |band| {
        for (i, p) in band.px.iter().enumerate() {
            let x = band.rect.x0 as u32 + (i % band.rect.width() as usize) as u32;
            let y = band.rect.y0 as u32 + (i / band.rect.width() as usize) as u32;
            let a = p[3].clamp(0.0, 1.0);
            let rgb = [p[0], p[1], p[2]]
                .map(|v| ((v * a + 1.0 - a).clamp(0.0, 1.0) * 255.0).round() as u8);
            output.put_pixel(x, y, image::Rgb(rgb));
        }
        Ok::<(), String>(())
    })?;
    Ok(output)
}

pub fn render(image: &RgbImage, recipe: &EditRecipe) -> Result<RgbImage, String> {
    let mut session = Session::new();
    session.add_document(document(image)?, None);
    let rotation = match recipe.rotate_quarter {
        1 => Some("image.imageRotation.90cw"),
        2 => Some("image.imageRotation.180"),
        3 => Some("image.imageRotation.90ccw"),
        _ => None,
    };
    if let Some(command) = rotation {
        execute(&mut session, command, json!({}))?;
    }
    if let Some(crop) = recipe.crop {
        let doc = session.active().ok_or("编辑文档不存在")?;
        let w = f64::from(doc.doc.size.width);
        let h = f64::from(doc.doc.size.height);
        let x = (crop.x * w).floor().clamp(0.0, w) as u32;
        let y = (crop.y * h).floor().clamp(0.0, h) as u32;
        let right = ((crop.x + crop.w) * w).ceil().clamp(f64::from(x), w) as u32;
        let bottom = ((crop.y + crop.h) * h).ceil().clamp(f64::from(y), h) as u32;
        if right > x && bottom > y {
            execute(
                &mut session,
                "image.crop",
                json!({"x": x, "y": y, "width": right - x, "height": bottom - y}),
            )?;
        }
    }
    adjustments(&mut session, recipe)?;
    composite(&session.active().ok_or("编辑文档不存在")?.doc)
}
