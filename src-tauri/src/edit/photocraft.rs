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
    pub native:Option<photocraft_engine::doc::Adjustment>,
    pub key: &'static str,
    pub kind: &'static str,
    pub params: serde_json::Value,
    pub luminosity: bool,
    pub opacity:f32,
}

/// 导出与增量预览共享原生参数，避免两条路径的操作顺序/默认值分叉。
pub(super) fn adjustment_specs(recipe: &EditRecipe) -> Result<Vec<AdjustmentSpec>,String> {
    let mut specs = Vec::new();
    if let Some(a)=recipe.legacy_adjustments {
        use photocraft_engine::doc::{Adjustment,adjust::{CurvePoint,ToneSpace}};
        let slope=(1.0+a.brightness/100.0)*(1.0+a.contrast/100.0);
        let intercept=-127.5*a.contrast/100.0;
        let points=(0..256).map(|x|CurvePoint{input:x as f32/255.0,output:((slope*f64::from(x)+intercept).clamp(0.0,255.0)/255.0) as f32}).collect();
        specs.push(AdjustmentSpec{native:Some(Adjustment::Curves{master:points,per_channel:Default::default(),space:ToneSpace::Rgb,black:Vec::new()}),key:"legacy-light",kind:"curves",params:json!({"legacy":a}),luminosity:false,opacity:1.0});
    }
    let mut add = |key, kind, params, luminosity| {
        specs.push(AdjustmentSpec {
            native:None,
            key,
            kind,
            params,
            luminosity,
            opacity:1.0,
        })
    };
    if let Some(a)=recipe.legacy_adjustments {
        if a.saturation!=0.0 {
            let saturation=1.0+a.saturation/100.0;
            let matrix:[[f64;4];3]=std::array::from_fn(|row|std::array::from_fn(|column|if column==3 {0.0} else {100.0*((1.0-saturation)*[0.2126,0.7152,0.0722][column]+if row==column {saturation} else {0.0})}));
            add("legacy-color","channelMixer",json!({"red":matrix[0],"green":matrix[1],"blue":matrix[2]}),false);
        }
    }
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
        return Ok(specs);
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
    if let Some(b)=&a.color_balance {
        if b.shadows.iter().chain(&b.midtones).chain(&b.highlights).any(|v|*v!=0.0) {
            add("color-balance","colorBalance",json!({"shadows":b.shadows,"midtones":b.midtones,"highlights":b.highlights,"preserveLuminosity":b.preserve_luminosity}),false);
        }
    }
    if let Some(b)=a.black_white.as_ref().filter(|b|b.enabled) {
        let mut params=json!({"reds":b.weights[0],"yellows":b.weights[1],"greens":b.weights[2],"cyans":b.weights[3],"blues":b.weights[4],"magentas":b.weights[5],"tint":b.tint.is_some()});
        if let Some(color)=&b.tint {params["tintColor"]=json!(color);}
        add("black-white","blackWhite",params,false);
    }
    if let Some(s)=a.selective_color.as_ref().filter(|s|s.ranges.values().flatten().any(|v|*v!=0.0)) {
        let mut params=serde_json::to_value(&s.ranges).map_err(|e|e.to_string())?;params["relative"]=json!(s.relative);
        add("selective-color","selectiveColor",params,false);
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
    if let Some(lookup)=a.lookup.as_ref().filter(|lookup|lookup.enabled && lookup.amount>0.0) {
        specs.push(AdjustmentSpec{native:None,key:"lut",kind:"colorLookup",params:super::lut::params(&lookup.id)?,luminosity:false,opacity:(lookup.amount/100.0) as f32});
    }
    Ok(specs)
}

pub(super) fn camera_tuning(recipe: &EditRecipe) -> [f64; 3] {
    recipe
        .advanced
        .as_ref()
        .map_or([0.0; 3], |a| [a.exposure, a.temperature, a.tint])
}

pub(super) fn camera_adjustment(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    camera_adjustment_scaled(session,recipe,1.0)
}
pub(super) fn camera_params(recipe:&EditRecipe,scale:f32)->photocraft_algo::camera_raw::CameraRaw {
    let mut params=recipe.advanced.as_ref().and_then(|a|a.development.as_ref()).map(|d|serde_json::from_value(serde_json::to_value(d).unwrap()).unwrap()).unwrap_or_else(photocraft_algo::camera_raw::CameraRaw::default);
    let [exposure,temperature,tint]=camera_tuning(recipe);
    params.exposure=exposure as f32;params.temperature=temperature as f32;params.tint=tint as f32;params.pixel_scale=scale.clamp(0.01,1.0);
    let defaults=photocraft_algo::camera_raw::CameraRaw::default();
    if params.sharpen_amount==0.0 {params.sharpen_radius=defaults.sharpen_radius;params.sharpen_detail=defaults.sharpen_detail;params.sharpen_masking=defaults.sharpen_masking;}
    if params.noise_luminance==0.0 {params.noise_luminance_detail=defaults.noise_luminance_detail;}
    if params.noise_color==0.0 {params.noise_color_detail=defaults.noise_color_detail;}
    if params.grain_amount==0.0 {params.grain_size=defaults.grain_size;params.grain_roughness=defaults.grain_roughness;}
    if params.vignette_amount==0.0 {params.vignette_midpoint=defaults.vignette_midpoint;params.vignette_roundness=defaults.vignette_roundness;params.vignette_feather=defaults.vignette_feather;params.vignette_highlights=defaults.vignette_highlights;params.vignette_style=defaults.vignette_style;}
    params
}
pub(super) fn camera_adjustment_scaled(session:&mut Session,recipe:&EditRecipe,scale:f32)->Result<(),String> {
    let params=camera_params(recipe,scale);
    if !params.is_identity() {
        execute(
            session,
            "filter.cameraRaw",
            serde_json::to_value(params).map_err(|e|e.to_string())?,
        )?;
    }
    Ok(())
}

fn adjustments(session: &mut Session, recipe: &EditRecipe) -> Result<(), String> {
    camera_adjustment(session, recipe)?;
    for spec in adjustment_specs(recipe)? {
        if let Some(native)=spec.native {
            let state=session.active_mut().ok_or("编辑文档不存在")?;
            let layer=Layer::new(spec.key,LayerContent::Adjustment(native));
            state.active_layer=Some(layer.id);state.selected_layers=vec![layer.id];state.layer_anchor=Some(layer.id);
            std::sync::Arc::make_mut(&mut state.doc).layers.push(layer);
            continue;
        }
        execute(
            session,
            &format!("layer.newAdjustmentLayer.{}", spec.kind),
            spec.params,
        )?;
        if spec.opacity!=1.0 {execute(session,"layer.setProps",json!({"opacity":spec.opacity}))?;}
        if spec.luminosity {
            execute(session, "layer.setProps", json!({"blend": "Luminosity"}))?;
        }
    }
    if !recipe.masks.is_empty() {
        let base=active_document(session)?;
        let layers=super::masks::MaskCache::default().layers(&base,recipe)?;
        let state=session.active_mut().ok_or("编辑文档不存在")?;
        std::sync::Arc::make_mut(&mut state.doc).layers.extend(layers);
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
    if let Some(geometry)=&recipe.geometry {
        if geometry.flip_horizontal {execute(session,"image.imageRotation.flipCanvasHorizontal",json!({}))?;}
        if geometry.flip_vertical {execute(session,"image.imageRotation.flipCanvasVertical",json!({}))?;}
        if geometry.angle!=0.0 {execute(session,"image.rotation.arbitrary",json!({"angle":geometry.angle,"direction":"cw"}))?;}
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

#[cfg(test)]
mod migration_tests {
    use super::*;
    #[test]
    fn historical_filters_migrate_without_changing_saved_pixel_values() {
        let bytes:Vec<u8>=(0..256*3).map(|i|((i*73+19)%256) as u8).collect();
        let image=photocraft_codecs::Image::from_u8(256,1,photocraft_codecs::ChannelLayout::Rgb,bytes.clone()).unwrap();
        let base=super::super::source::from_image("Legacy",&image).unwrap();
        for (brightness,contrast,saturation) in [(30.0,25.0,-60.0),(-35.0,-100.0,100.0),(100.0,100.0,80.0),(0.0,0.0,-100.0)] {
            let adjustment=super::super::recipe::Adjustments{brightness,contrast,saturation};
            let mut old=image::RgbImage::from_raw(256,1,bytes.clone()).unwrap();
            super::super::render::apply_adjustments(&mut old,adjustment);
            let recipe=super::super::recipe::parse_recipe(&json!({"version":1,"renderer":"photocraft","legacyAdjustments":adjustment})).unwrap();
            let doc=render_document(&base,&recipe).unwrap();
            let native=super::super::preview_renderer::Renderer::new(&base).canvas_document(&base,&recipe).unwrap();
            assert_eq!(photocraft_compose::flatten(&doc).px,photocraft_compose::flatten(&native).px);
            let actual=photocraft_compose::flatten(&doc);
            let max=actual.px.iter().zip(old.pixels()).flat_map(|(p,old)|(0..3).map(move |ch|((p[ch]*255.0).round()-f32::from(old[ch])).abs())).fold(0.0,f32::max);
            assert!(max<=1.0,"legacy pixel error {max}, params {adjustment:?}");
        }
    }
    #[test]
    fn geometry_uses_native_flip_and_arbitrary_rotation_commands() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("Geometry",Size::new(40,30),ColorMode::Rgb,SampleType::U8,Color::rgba(0.2,0.3,0.4,1.0));
        let recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","geometry":{"angle":30,"flipHorizontal":true,"flipVertical":true}})).unwrap();
        let persisted=serde_json::to_value(&recipe).unwrap();
        let restored=super::super::recipe::parse_recipe(&persisted).unwrap();assert_eq!(restored,recipe);
        let actual=render_document(&base,&restored).unwrap();
        let mut expected=Session::new();expected.add_document(base,None);
        execute(&mut expected,"image.imageRotation.flipCanvasHorizontal",json!({})).unwrap();
        execute(&mut expected,"image.imageRotation.flipCanvasVertical",json!({})).unwrap();
        execute(&mut expected,"image.rotation.arbitrary",json!({"angle":30,"direction":"cw"})).unwrap();
        let expected=active_document(&expected).unwrap();
        assert_eq!(actual.size,Size::new(50,46));assert_eq!(actual.size,expected.size);
        assert_eq!(photocraft_compose::flatten(&actual).px,photocraft_compose::flatten(&expected).px);
    }
    #[test]
    fn photocraft_export_applies_lut_and_leaves_source_bytes_unchanged() {
        let directory=tempfile::tempdir().unwrap();let path=directory.path().join("source.png");
        image::RgbImage::from_pixel(32,24,image::Rgb([90,130,170])).save(&path).unwrap();
        let original=std::fs::read(&path).unwrap();
        let neutral=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft"})).unwrap();
        let lookup=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"lookup":{"id":"builtin:warm","amount":50,"enabled":true}}})).unwrap();
        let (before,w,h)=export_native(&path,&neutral,None,95).unwrap();
        let (after,aw,ah)=export_native(&path,&lookup,None,95).unwrap();
        assert_eq!((w,h),(32,24));assert_eq!((w,h),(aw,ah));
        assert_ne!(image::load_from_memory(&before).unwrap().to_rgb8(),image::load_from_memory(&after).unwrap().to_rgb8());
        assert_eq!(std::fs::read(path).unwrap(),original);
    }
}
