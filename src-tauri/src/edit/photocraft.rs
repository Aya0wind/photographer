//! 上游 PhotoCraft 的唯一接入层；不依赖图库、数据库或存储目录。
//! v1 配方是持久化真值；导出从基础文档重建，增量预览共享原生参数并缓存各阶段。

use photocraft_engine::{
    doc::{Document, Layer, LayerContent},
    Session,
};
use serde_json::json;

use super::recipe::EditRecipe;

pub(super) fn execute(
    session: &mut Session,
    command: &str,
    params: serde_json::Value,
) -> Result<(), String> {
    session
        .execute(command, params)
        .map(|_| ())
        .map_err(|e| format!("PhotoCraft {command}: {e}"))
}

pub(super) struct AdjustmentSpec {
    pub key: &'static str,
    pub kind: &'static str,
    pub params: serde_json::Value,
    pub luminosity: bool,
}

/// 导出与增量预览共享原生参数，避免两条路径的操作顺序/默认值分叉。
pub(super) fn adjustment_specs(recipe: &EditRecipe) -> Vec<AdjustmentSpec> {
    let mut specs = Vec::new();
    let mut add = |key, kind, params, luminosity| {
        specs.push(AdjustmentSpec {
            key,
            kind,
            params,
            luminosity,
        })
    };
    if let Some(a) = recipe.adjustments {
        if a.brightness != 0.0 || a.contrast != 0.0 {
            add(
                "basic",
                "brightnessContrast",
                json!({"brightness": a.brightness, "contrast": a.contrast.clamp(-50.0, 100.0)}),
                false,
            );
        }
        if a.saturation != 0.0 {
            add(
                "saturation",
                "hueSaturation",
                json!({"saturation": a.saturation}),
                false,
            );
        }
    }
    let Some(a) = &recipe.advanced else {
        return specs;
    };
    if a.vibrance != 0.0 {
        add(
            "vibrance",
            "vibrance",
            json!({"vibrance": a.vibrance}),
            false,
        );
    }
    if let Some(levels) = &a.levels {
        add(
            "levels",
            "levels",
            json!({"inBlack": levels.black, "inWhite": levels.white, "gamma": levels.gamma}),
            false,
        );
    }
    if !a.hsl.is_empty() {
        add("hsl", "hueSaturation", json!(a.hsl), false);
    }
    let mut curves = serde_json::Map::new();
    for (key, points) in [
        ("points", &a.curves),
        ("red", &a.channel_curves.red),
        ("green", &a.channel_curves.green),
        ("blue", &a.channel_curves.blue),
    ] {
        if !points.is_empty() {
            curves.insert(key.into(), json!(points));
        }
    }
    if !curves.is_empty() {
        add("curves", "curves", curves.into(), false);
    }
    if !a.channel_curves.luminance.is_empty() {
        add(
            "luminance",
            "curves",
            json!({"points": a.channel_curves.luminance}),
            true,
        );
    }
    specs
}

pub(super) fn camera_tuning(recipe: &EditRecipe) -> [f64; 3] {
    recipe
        .advanced
        .as_ref()
        .map_or([0.0; 3], |a| [a.exposure, a.temperature, a.tint])
}

pub(super) fn camera_adjustment(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    let [exposure, temperature, tint] = camera_tuning(recipe);
    if [exposure, temperature, tint] != [0.0; 3] {
        execute(
            session,
            "filter.cameraRaw",
            json!({"exposure": exposure, "temperature": temperature, "tint": tint}),
        )?;
    }
    Ok(())
}

fn adjustments(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    camera_adjustment(session, recipe)?;
    for spec in adjustment_specs(recipe) {
        execute(
            session,
            &format!("layer.newAdjustmentLayer.{}", spec.kind),
            spec.params,
        )?;
        if spec.luminosity {
            execute(session, "layer.setProps", json!({"blend": "Luminosity"}))?;
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

/// 原生缩放保留位深、色彩空间、图层和 ICC。
pub fn resize(doc: &Document, long_edge: Option<u32>) -> Result<Document, String> {
    let Some(edge) = long_edge.filter(|n| *n > 0) else {
        return Ok(doc.clone());
    };
    let current = doc.size.width.max(doc.size.height);
    if current <= edge {
        return Ok(doc.clone());
    }
    let width = (u64::from(doc.size.width) * u64::from(edge) / u64::from(current)).max(1) as u32;
    let height = (u64::from(doc.size.height) * u64::from(edge) / u64::from(current)).max(1) as u32;
    let mut session = Session::new();
    session.add_document(doc.clone(), None);
    execute(
        &mut session,
        "image.imageSize",
        json!({"width": width, "height": height}),
    )?;
    let mut out = active_document(&session)?;
    out.id = photocraft_engine::doc::DocId::fresh();
    Ok(out)
}

pub(super) fn active_document(session: &Session) -> Result<Document, String> {
    session
        .active()
        .map(|s| (*s.doc).clone())
        .ok_or("编辑文档不存在".into())
}

/// GPU 只负责原生合成；格式转换、ICC、透明背景和编码均由上游处理。
#[derive(Default)]
pub(super) struct DisplayTimings {
    pub composite_ms: f64,
    pub profile_ms: f64,
    pub encode_ms: f64,
    pub gpu: bool,
}

pub fn jpeg(doc: &Document, quality: u8) -> Result<Vec<u8>, String> {
    jpeg_with_timings(doc, quality).map(|(bytes, _)| bytes)
}

pub(super) fn jpeg_with_timings(
    doc: &Document,
    quality: u8,
) -> Result<(Vec<u8>, DisplayTimings), String> {
    let started = std::time::Instant::now();
    let mut timing = DisplayTimings::default();
    let gpu = super::gpu::composite(doc);
    timing.gpu = gpu.is_some();
    let mut flat = if let Some(surface) = gpu {
        let mut out = doc.clone();
        out.layers = vec![Layer::new("Composite", LayerContent::Raster(surface))];
        out
    } else {
        let image =
            photocraft_io::document_to_image(doc, &mut Vec::new()).map_err(|e| e.to_string())?;
        super::source::from_image("Composite", &image)?
    };
    timing.composite_ms = started.elapsed().as_secs_f64() * 1000.0;
    let phase = std::time::Instant::now();
    let srgb = photocraft_cms::Builtin::Srgb.profile();
    // 只在原生解析的源/目标 ICC 完全一致时省略像素转换。
    if flat.mode == photocraft_engine::doc::ColorMode::Rgb
        && photocraft_engine::color_cmds::document_profile(&flat).to_bytes() == srgb.to_bytes()
    {
        flat.icc_profile = Some(srgb.to_bytes());
    } else {
        photocraft_engine::color_cmds::convert_document(
            &mut flat,
            srgb,
            photocraft_cms::Intent::RelativeColorimetric,
            true,
        )
        .map_err(|e| e.to_string())?;
    }
    timing.profile_ms = phase.elapsed().as_secs_f64() * 1000.0;
    let phase = std::time::Instant::now();
    let mut opts = photocraft_io::ExportOptions::default();
    opts.encode.jpeg_quality = quality;
    opts.xmp = photocraft_io::XmpEmbed::None;
    let bytes = photocraft_io::export(&flat, "jpg", &opts)
        .map_err(|e| e.to_string())?
        .bytes;
    timing.encode_ms = phase.elapsed().as_secs_f64() * 1000.0;
    Ok((bytes, timing))
}

pub fn geometry(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    let rotation = match recipe.rotate_quarter {
        1 => Some("image.imageRotation.90cw"),
        2 => Some("image.imageRotation.180"),
        3 => Some("image.imageRotation.90ccw"),
        _ => None,
    };
    if let Some(command) = rotation {
        execute(session, command, json!({}))?;
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
                session,
                "image.crop",
                json!({"x": x, "y": y, "width": right - x, "height": bottom - y}),
            )?;
        }
    }
    Ok(())
}

/// 标注也是上游原生图层/笔刷；此处仅将 UI 归一化坐标适配为像素/点。
pub(super) fn annotations(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    let doc = session.active().ok_or("编辑文档不存在")?;
    let width = f64::from(doc.doc.size.width);
    let height = f64::from(doc.doc.size.height);
    let dpi = f64::from(doc.doc.resolution_dpi);
    for layer in &recipe.text_layers {
        if layer.text.trim().is_empty() {
            continue;
        }
        execute(
            session,
            "type.create",
            json!({"text": layer.text, "color": layer.color,
            "box": [layer.x * width, layer.y * height, ((1.0 - layer.x) * width).max(1.0), ((1.0 - layer.y) * height).max(1.0)],
            "size": (layer.size_rel * width * 72.0 / dpi).clamp(0.1, 1296.0), "name": layer.id}),
        )?;
    }
    for stroke in &recipe.brush_strokes {
        execute(session, "layer.new.layer", json!({"name": stroke.id}))?;
        let points: Vec<_> = stroke
            .points
            .iter()
            .map(|p| [p.x * width, p.y * height])
            .collect();
        execute(
            session,
            "paint.stroke",
            json!({"points": points, "color": stroke.color, "size": stroke.width_rel * width,
            "brush": {"hardness": 1.0, "spacing": 0.1}, "seed": 0}),
        )?;
    }
    Ok(())
}

pub fn render_document(base: &Document, recipe: &EditRecipe) -> Result<Document, String> {
    let mut session = Session::new();
    session.add_document(base.clone(), None);
    adjustments(&mut session, recipe)?;
    geometry(&mut session, recipe)?;
    annotations(&mut session, recipe)?;
    active_document(&session)
}

pub fn export_native(
    path: &std::path::Path,
    recipe: &EditRecipe,
    long_edge: Option<u32>,
    quality: u8,
) -> Result<(Vec<u8>, u32, u32), String> {
    let source = super::source::open(path)?;
    let mut values = recipe.clone();
    let base = if let Some(raw) = source.raw {
        let a = recipe.advanced.clone().unwrap_or_default();
        let base = if [a.exposure, a.temperature, a.tint] == [0.0; 3] {
            source.document
        } else {
            raw.develop(recipe)?
        };
        if let Some(a) = &mut values.advanced {
            a.exposure = 0.0;
            a.temperature = 0.0;
            a.tint = 0.0;
        }
        base
    } else {
        source.document
    };
    let doc = resize(&render_document(&base, &values)?, long_edge)?;
    Ok((jpeg(&doc, quality)?, doc.size.width, doc.size.height))
}
