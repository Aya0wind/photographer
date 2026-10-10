//! 编辑配方契约（version 1）与服务端校验。
//!
//! 前端把配方按 camelCase JSON 传入（[`crate::ipc::edit`]）；本模块做三件事：
//! ① serde 反序列化（结构非法 → 错误）；② 语义校验（version==1、
//! rotateQuarter 0..3、颜色 `#RRGGBB`）；③ 数值夹取 0..1（夹取后的归一化
//! 配方才是存库/渲染用的唯一真值——`edit_recipe_save` 存归一化结果）。

use serde::{Deserialize, Serialize};

/// 配方版本（当前 1；不认识的版本一律报错，不做猜测式兼容）。
pub const RECIPE_VERSION: u32 = 1;

/// 归一化坐标点。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormPoint {
    pub x: f64,
    pub y: f64,
}

/// 裁剪矩形：相对**旋转后**图像归一化（x/w 对宽、y/h 对高），∈0..1。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CropRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// 文字图层：锚点=文本框左上角、左对齐；`text` 支持多行（`\n` 分行）。
/// sizeRel = 字高（em）/画布宽。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextLayer {
    pub id: String,
    pub x: f64,
    pub y: f64,
    pub text: String,
    pub size_rel: f64,
    /// `#RRGGBB`（十六进制；大小写不敏感）。
    pub color: String,
}

/// 笔迹图层：折线、圆头端点/连接、等宽。widthRel = 笔宽/画布宽。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrushStroke {
    pub id: String,
    pub color: String,
    pub width_rel: f64,
    pub points: Vec<NormPoint>,
}

/// 输出偏好（导出时 options 可覆盖）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputPrefs {
    /// null = 按源尺寸（只缩不放）。
    #[serde(default)]
    pub long_edge: Option<u32>,
    #[serde(default = "default_quality")]
    pub quality: u8,
}

fn default_quality() -> u8 {
    90
}

impl Default for OutputPrefs {
    fn default() -> Self {
        Self {
            long_edge: None,
            quality: default_quality(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Adjustments {
    pub brightness: f64,
    pub contrast: f64,
    pub saturation: f64,
}

/// 编辑配方（version 1）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RenderEngine {
    Photocraft,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AdvancedAdjustments {
    #[serde(skip_serializing_if="Option::is_none")]
    pub development:Option<Development>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub color_balance:Option<ColorBalance>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub black_white:Option<BlackWhite>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub selective_color:Option<SelectiveColor>,
    pub exposure: f64,
    pub temperature: f64,
    pub tint: f64,
    pub vibrance: f64,
    #[serde(skip_serializing_if="Option::is_none")]
    pub lookup: Option<LutSelection>,
    pub curves: Vec<[f64; 2]>,
    pub channel_curves: ChannelCurves,
    pub levels: Option<Levels>,
    pub hsl: std::collections::BTreeMap<String, HslRange>,
}

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct LutSelection {
    pub id:String,
    pub amount:f64,
    pub enabled:bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelCurves {
    pub red: Vec<[f64; 2]>,
    pub green: Vec<[f64; 2]>,
    pub blue: Vec<[f64; 2]>,
    pub luminance: Vec<[f64; 2]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    pub black: f64,
    pub white: f64,
    pub gamma: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HslRange {
    pub hue: f64,
    pub saturation: f64,
    pub lightness: f64,
}

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct Development {
    pub highlights:f64,pub shadows:f64,pub whites:f64,pub blacks:f64,
    pub texture:f64,pub clarity:f64,pub dehaze:f64,
    pub sharpen_amount:f64,pub sharpen_radius:f64,pub sharpen_detail:f64,pub sharpen_masking:f64,
    pub noise_luminance:f64,pub noise_luminance_detail:f64,pub noise_color:f64,pub noise_color_detail:f64,
    pub grain_amount:f64,pub grain_size:f64,pub grain_roughness:f64,
    pub vignette_amount:f64,pub vignette_midpoint:f64,pub vignette_roundness:f64,pub vignette_feather:f64,pub vignette_highlights:f64,pub vignette_style:String,
}
impl Default for Development {
    fn default()->Self {
        let defaults=serde_json::to_value(photocraft_algo::camera_raw::CameraRaw::default()).expect("Camera Raw defaults are finite");
        let n=|key:&str|defaults[key].as_f64().unwrap();
        Self{highlights:0.0,shadows:0.0,whites:0.0,blacks:0.0,texture:0.0,clarity:0.0,dehaze:0.0,sharpen_amount:0.0,sharpen_radius:n("sharpenRadius"),sharpen_detail:n("sharpenDetail"),sharpen_masking:0.0,noise_luminance:0.0,noise_luminance_detail:n("noiseLuminanceDetail"),noise_color:0.0,noise_color_detail:n("noiseColorDetail"),grain_amount:0.0,grain_size:n("grainSize"),grain_roughness:n("grainRoughness"),vignette_amount:0.0,vignette_midpoint:n("vignetteMidpoint"),vignette_roundness:0.0,vignette_feather:n("vignetteFeather"),vignette_highlights:0.0,vignette_style:"highlightPriority".into()}
    }
}
impl Development {
    pub fn active(&self)->bool {
        [self.highlights,self.shadows,self.whites,self.blacks,self.texture,self.clarity,self.dehaze,self.sharpen_amount,self.noise_luminance,self.noise_color,self.grain_amount,self.vignette_amount].iter().any(|v|*v!=0.0)
    }
}

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct ColorBalance {
    pub shadows:[f64;3],pub midtones:[f64;3],pub highlights:[f64;3],pub preserve_luminosity:bool,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct BlackWhite {
    pub enabled:bool,pub weights:[f64;6],pub tint:Option<String>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct SelectiveColor {
    pub relative:bool,pub ranges:std::collections::BTreeMap<String,[f64;4]>,
}

/// Local coverage is saved in original source coordinates, before geometry.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum MaskKind { Brush, Linear, Radial }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct MaskStroke {
    pub points: Vec<NormPoint>,
    pub width_rel: f64,
    pub hardness: f64,
    pub opacity: f64,
    pub flow: f64,
    pub erase: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct LocalMask {
    pub id: String,
    pub name: String,
    pub kind: MaskKind,
    pub enabled: bool,
    pub inverted: bool,
    pub density: f64,
    pub feather: f64,
    pub from: NormPoint,
    pub to: NormPoint,
    #[serde(default)]
    pub strokes: Vec<MaskStroke>,
    #[serde(default)]
    pub adjustments: Option<Adjustments>,
    #[serde(default)]
    pub advanced: Option<AdvancedAdjustments>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecipe {
    /// Retains historical sRGB filter semantics while rendering via native layers.
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub legacy_adjustments: Option<Adjustments>,
    pub version: u32,
    /// 缺省使用旧算法，防止历史配方打开后改变效果。新配方显式选择 PhotoCraft。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer: Option<RenderEngine>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advanced: Option<AdvancedAdjustments>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub masks: Vec<LocalMask>,
    #[serde(default)]
    pub rotate_quarter: u32,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub geometry: Option<Geometry>,
    #[serde(default)]
    pub adjustments: Option<Adjustments>,
    #[serde(default)]
    pub crop: Option<CropRect>,
    #[serde(default)]
    pub text_layers: Vec<TextLayer>,
    #[serde(default)]
    pub brush_strokes: Vec<BrushStroke>,
    #[serde(default)]
    pub output: OutputPrefs,
}

#[derive(Debug,Clone,Default,PartialEq,Serialize,Deserialize)]
#[serde(default,rename_all="camelCase")]
pub struct Geometry {pub angle:f64,pub flip_horizontal:bool,pub flip_vertical:bool}

/// 解析 `#RRGGBB`（大小写不敏感）为 RGB；非法返回 None。
pub fn parse_color(color: &str) -> Option<[u8; 3]> {
    let hex = color.strip_prefix('#')?;
    if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; 3];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// 单值夹取到 0..=1。
fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

fn validate_curve(curves: &[[f64; 2]]) -> Result<(), String> {
    if !curves.is_empty()
        && (curves.len() < 2
            || curves.len() > 19
            || curves
                .iter()
                .flatten()
                .any(|v| !v.is_finite() || !(0.0..=255.0).contains(v))
            || curves.windows(2).any(|p| p[0][0] >= p[1][0]))
    {
        return Err("曲线控制点无效".into());
    }
    Ok(())
}

fn normalize_advanced(a: &mut AdvancedAdjustments) -> Result<(), String> {
    fn numbers(values:&mut [f64],min:f64,max:f64)->Result<(),String>{
        for v in values {if !v.is_finite(){return Err("调色数值无效".into());}*v=v.clamp(min,max);}Ok(())
    }
    if let Some(d)=&mut a.development {
        for v in [&mut d.highlights,&mut d.shadows,&mut d.whites,&mut d.blacks,&mut d.texture,&mut d.clarity,&mut d.dehaze,&mut d.vignette_amount,&mut d.vignette_roundness] {numbers(std::slice::from_mut(v),-100.0,100.0)?;}
        for v in [&mut d.sharpen_detail,&mut d.sharpen_masking,&mut d.noise_luminance,&mut d.noise_luminance_detail,&mut d.noise_color,&mut d.noise_color_detail,&mut d.grain_amount,&mut d.grain_size,&mut d.grain_roughness,&mut d.vignette_midpoint,&mut d.vignette_feather,&mut d.vignette_highlights] {numbers(std::slice::from_mut(v),0.0,100.0)?;}
        numbers(std::slice::from_mut(&mut d.sharpen_amount),0.0,150.0)?;numbers(std::slice::from_mut(&mut d.sharpen_radius),0.5,3.0)?;
        if !["highlightPriority","colorPriority","paintOverlay"].contains(&d.vignette_style.as_str()){return Err("暗角样式无效".into());}
    }
    if let Some(b)=&mut a.color_balance {
        numbers(&mut b.shadows,-100.0,100.0)?;numbers(&mut b.midtones,-100.0,100.0)?;numbers(&mut b.highlights,-100.0,100.0)?;
    }
    if let Some(b)=&mut a.black_white {
        numbers(&mut b.weights,-200.0,300.0)?;
        if b.tint.as_ref().is_some_and(|v|parse_color(v).is_none()){return Err("黑白着色色彩无效".into());}
    }
    if let Some(s)=&mut a.selective_color {
        for (name,values) in &mut s.ranges {
            if !["reds","yellows","greens","cyans","blues","magentas","whites","neutrals","blacks"].contains(&name.as_str()){return Err("可选颜色范围无效".into());}
            numbers(values,-100.0,100.0)?;
        }
    }
    for (v, min, max) in [
        (&mut a.exposure, -5.0, 5.0),
        (&mut a.temperature, -100.0, 100.0),
        (&mut a.tint, -100.0, 100.0),
        (&mut a.vibrance, -100.0, 100.0),
    ] {
        if !v.is_finite() {
            return Err("高级调整数值无效".into());
        }
        *v = v.clamp(min, max);
    }
    for curves in [
        &a.curves,
        &a.channel_curves.red,
        &a.channel_curves.green,
        &a.channel_curves.blue,
        &a.channel_curves.luminance,
    ] {
        validate_curve(curves)?;
    }
    if let Some(levels) = &mut a.levels {
        if ![levels.black, levels.white, levels.gamma]
            .iter()
            .all(|v| v.is_finite())
        {
            return Err("色阶数值无效".into());
        }
        levels.black = levels.black.clamp(0.0, 253.0);
        levels.white = levels.white.clamp(levels.black + 2.0, 255.0);
        levels.gamma = levels.gamma.clamp(0.01, 9.99);
    }
    if let Some(lookup)=&mut a.lookup {
        super::lut::validate_id(&lookup.id)?;
        if lookup.id.len()>96 || !lookup.amount.is_finite() {return Err("调色方案参数无效".into());}
        lookup.amount=lookup.amount.clamp(0.0,100.0);
    }
    for (key, range) in &mut a.hsl {
        if !["reds", "yellows", "greens", "cyans", "blues", "magentas"].contains(&key.as_str())
            || ![range.hue, range.saturation, range.lightness]
                .iter()
                .all(|v| v.is_finite())
        {
            return Err("HSL 数值无效".into());
        }
        range.hue = range.hue.clamp(-180.0, 180.0);
        range.saturation = range.saturation.clamp(-100.0, 100.0);
        range.lightness = range.lightness.clamp(-100.0, 100.0);
    }
    Ok(())
}

/// 反序列化 + 校验 + 夹取，产出归一化配方。
///
/// 错误（用户可见文案）：
/// - 版本缺失/非 1 → 「编辑配方版本不支持」；
/// - rotateQuarter 越界（>3）、裁剪宽高夹取后为 0、文字尺寸/笔宽夹取后为 0
///   → 各自的具体错误；
/// - 颜色非 `#RRGGBB` → 「颜色格式非法」；
/// - 结构不匹配（字段类型错/缺必填）→ serde 错误透传。
pub fn parse_recipe(value: &serde_json::Value) -> Result<EditRecipe, String> {
    let mut recipe: EditRecipe =
        serde_json::from_value(value.clone()).map_err(|e| format!("编辑配方结构非法: {e}"))?;
    if recipe.version != RECIPE_VERSION {
        return Err(format!(
            "编辑配方版本不支持（当前支持 {RECIPE_VERSION}，收到 {}）",
            recipe.version
        ));
    }
    if recipe.masks.len()>32 {return Err("蒙版数量超过上限".into());}
    if !recipe.masks.is_empty() && recipe.renderer!=Some(RenderEngine::Photocraft) {
        return Err("局部蒙版需要使用高级编辑引擎".into());
    }
    let mut ids=std::collections::HashSet::new();
    let mut point_count=0usize;
    for mask in &mut recipe.masks {
        if mask.id.is_empty() || mask.id.len()>96 || !ids.insert(mask.id.clone()) || mask.name.len()>512 {
            return Err("蒙版名称或标识无效".into());
        }
        for point in [&mut mask.from,&mut mask.to] {
            if !point.x.is_finite() || !point.y.is_finite() {return Err("蒙版坐标无效".into());}
            point.x=clamp01(point.x);point.y=clamp01(point.y);
        }
        for v in [&mut mask.density,&mut mask.feather] {
            if !v.is_finite() {return Err("蒙版数值无效".into());}
            *v=v.clamp(0.0,100.0);
        }
        for stroke in &mut mask.strokes {
            point_count=point_count.saturating_add(stroke.points.len());
            if point_count>100_000 || stroke.points.is_empty() {return Err("蒙版笔迹无效或过长".into());}
            for (v,min,max) in [(&mut stroke.width_rel,0.0001,1.0),(&mut stroke.hardness,0.0,1.0),(&mut stroke.opacity,0.0,1.0),(&mut stroke.flow,0.0,1.0)] {
                if !v.is_finite() {return Err("蒙版画笔数值无效".into());}*v=v.clamp(min,max);
            }
            for point in &mut stroke.points {
                if !point.x.is_finite() || !point.y.is_finite() {return Err("蒙版笔迹坐标无效".into());}
                point.x=clamp01(point.x);point.y=clamp01(point.y);
            }
        }
        if let Some(a)=&mut mask.advanced {normalize_advanced(a)?;}
        if let Some(a)=&mut mask.adjustments {
            for v in [&mut a.brightness,&mut a.contrast,&mut a.saturation] {
                if !v.is_finite() {return Err("局部调整数值无效".into());}*v=v.clamp(-100.0,100.0);
            }
        }
    }
    if let Some(a) = recipe.advanced.as_mut() {
        normalize_advanced(a)?;
    }
    if let Some(adjustments) = recipe.adjustments.as_mut() {
        for v in [
            &mut adjustments.brightness,
            &mut adjustments.contrast,
            &mut adjustments.saturation,
        ] {
            if !v.is_finite() {
                return Err("调整数值无效".into());
            }
            *v = v.clamp(-100.0, 100.0);
        }
    }
    if let Some(a)=&mut recipe.legacy_adjustments {
        if recipe.renderer!=Some(RenderEngine::Photocraft) {return Err("旧调色迁移需要高级编辑引擎".into());}
        for v in [&mut a.brightness,&mut a.contrast,&mut a.saturation] {
            if !v.is_finite(){return Err("旧调色数值无效".into());}*v=v.clamp(-100.0,100.0);
        }
    }
    if recipe.rotate_quarter > 3 {
        return Err(format!(
            "旋转步数非法（0..3，收到 {}）",
            recipe.rotate_quarter
        ));
    }
    if let Some(geometry)=&mut recipe.geometry {
        if !geometry.angle.is_finite() {return Err("旋转角度无效".into());}
        geometry.angle=geometry.angle.clamp(-180.0,180.0);
    }
    if let Some(crop) = recipe.crop.as_mut() {
        crop.x = clamp01(crop.x);
        crop.y = clamp01(crop.y);
        crop.w = clamp01(crop.w);
        crop.h = clamp01(crop.h);
        if crop.w <= 0.0 || crop.h <= 0.0 {
            return Err("裁剪区域为空（w/h 夹取后为 0）".into());
        }
    }
    for layer in &mut recipe.text_layers {
        layer.x = clamp01(layer.x);
        layer.y = clamp01(layer.y);
        layer.size_rel = clamp01(layer.size_rel);
        if layer.size_rel <= 0.0 {
            return Err(format!("文字图层 {} 尺寸为 0", layer.id));
        }
        if parse_color(&layer.color).is_none() {
            return Err(format!("文字图层 {} 颜色格式非法（需 #RRGGBB）", layer.id));
        }
    }
    for stroke in &mut recipe.brush_strokes {
        stroke.width_rel = clamp01(stroke.width_rel);
        if stroke.width_rel <= 0.0 {
            return Err(format!("笔迹 {} 宽度为 0", stroke.id));
        }
        if parse_color(&stroke.color).is_none() {
            return Err(format!("笔迹 {} 颜色格式非法（需 #RRGGBB）", stroke.id));
        }
        for point in &mut stroke.points {
            point.x = clamp01(point.x);
            point.y = clamp01(point.y);
        }
    }
    // quality 夹取 1..=100；longEdge=0 视同未设置（源尺寸）。
    recipe.output.quality = recipe.output.quality.clamp(1, 100);
    if recipe.output.long_edge == Some(0) {
        recipe.output.long_edge = None;
    }
    Ok(recipe)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v1(body: serde_json::Value) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("version".into(), serde_json::json!(1));
        if let serde_json::Value::Object(fields) = body {
            for (key, value) in fields {
                map.insert(key, value);
            }
        }
        serde_json::Value::Object(map)
    }

    #[test]
    fn extended_color_validates_ranges_colors_and_roundtrips_defaults() {
        let recipe=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"colorBalance":{"shadows":[-200,0,150],"midtones":[0,0,0],"highlights":[0,0,0],"preserveLuminosity":true},"blackWhite":{"enabled":false,"weights":[-999,60,40,60,20,999],"tint":null},"selectiveColor":{"relative":false,"ranges":{"reds":[150,0,0,-150]}}}})).unwrap();
        let a=recipe.advanced.as_ref().unwrap();assert_eq!(a.color_balance.as_ref().unwrap().shadows,[-100.0,0.0,100.0]);
        assert_eq!(a.black_white.as_ref().unwrap().weights[0],-200.0);assert_eq!(a.black_white.as_ref().unwrap().weights[5],300.0);
        assert_eq!(a.selective_color.as_ref().unwrap().ranges["reds"],[100.0,0.0,0.0,-100.0]);
        let mut value=serde_json::to_value(&recipe).unwrap();assert_eq!(parse_recipe(&value).unwrap(),recipe);
        value["advanced"]["blackWhite"]["tint"]=serde_json::json!("invalid");assert!(parse_recipe(&value).is_err());
        value["advanced"]["blackWhite"]["tint"]=serde_json::Value::Null;
        value["advanced"]["selectiveColor"]["ranges"]["unknown"]=serde_json::json!([0,0,0,0]);assert!(parse_recipe(&value).is_err());
    }
    #[test]
    fn accepts_minimal_and_clamps() {
        let recipe = parse_recipe(&serde_json::json!({ "version": 1 })).unwrap();
        assert_eq!(recipe.rotate_quarter, 0);
        assert!(recipe.crop.is_none());
        assert_eq!(recipe.output.quality, 90);

        let recipe = parse_recipe(&v1(serde_json::json!({
            "rotateQuarter": 2,
            "crop": { "x": -0.5, "y": 0.25, "w": 2.0, "h": 0.5 },
            "textLayers": [{ "id": "t1", "x": 1.7, "y": 0.2, "text": "hi",
                             "sizeRel": 0.05, "color": "#ffFF33" }],
            "brushStrokes": [{ "id": "b1", "color": "#FF3333", "widthRel": 9.0,
                               "points": [{ "x": -1.0, "y": 0.5 }] }],
            "output": { "longEdge": 0, "quality": 250 }
        })))
        .unwrap();
        assert_eq!(recipe.rotate_quarter, 2);
        let crop = recipe.crop.unwrap();
        assert!((crop.x - 0.0).abs() < 1e-9 && (crop.w - 1.0).abs() < 1e-9);
        assert!((recipe.text_layers[0].x - 1.0).abs() < 1e-9);
        assert!((recipe.brush_strokes[0].width_rel - 1.0).abs() < 1e-9);
        assert_eq!(recipe.output.long_edge, None);
        assert_eq!(recipe.output.quality, 100);
        assert_eq!(
            parse_color(&recipe.text_layers[0].color),
            Some([255, 255, 51])
        );
    }

    #[test]
    fn rejects_bad_version_rotation_colors_and_empty_crop() {
        assert!(parse_recipe(&serde_json::json!({ "version": 2 })).is_err());
        assert!(parse_recipe(&v1(serde_json::json!({ "rotateQuarter": 4 }))).is_err());
        assert!(parse_recipe(&v1(serde_json::json!({
            "crop": { "x": 0.1, "y": 0.1, "w": 0.0, "h": 0.5 }
        })))
        .is_err());
        assert!(parse_recipe(&v1(serde_json::json!({
            "textLayers": [{ "id": "t", "x": 0, "y": 0, "text": "a",
                             "sizeRel": 0.1, "color": "red" }]
        })))
        .is_err());
        assert!(parse_recipe(&serde_json::json!({ "version": "1" })).is_err());
    }
}
