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
    #[serde(default="default_font")]
    pub font:String,
    #[serde(default="default_alignment")]
    pub align:String,
    #[serde(default="default_opacity")]
    pub opacity:f64,
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
    #[serde(default)]
    pub erase:bool,
    #[serde(default="default_opacity")]
    pub opacity:f64,
    pub id: String,
    pub color: String,
    pub width_rel: f64,
    pub points: Vec<NormPoint>,
}

fn default_font()->String{"Inter".into()}
fn default_alignment()->String{"left".into()}
fn default_opacity()->f64{1.0}

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
fn default_renderer()->Option<RenderEngine>{Some(RenderEngine::Photocraft)}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AdvancedAdjustments {
    #[serde(skip_serializing_if="Vec::is_empty")]
    pub point_colors:Vec<PointColor>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub primaries:Option<std::collections::BTreeMap<String,PrimaryShift>>,
    #[serde(skip_serializing_if="Vec::is_empty")]
    pub disabled_groups:Vec<String>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub parametric_curve:Option<ParametricCurve>,
    #[serde(skip_serializing_if="Option::is_none")]
    pub color_grading:Option<ColorGrading>,
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
    #[serde(skip_serializing_if="Option::is_none")]
    pub hsl_eight:Option<std::collections::BTreeMap<String,HslRange>>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct ParametricCurve {pub highlights:f64,pub lights:f64,pub darks:f64,pub shadows:f64,pub splits:[f64;3]}
impl Default for ParametricCurve{fn default()->Self{Self{highlights:0.0,lights:0.0,darks:0.0,shadows:0.0,splits:[25.0,50.0,75.0]}}}
impl ParametricCurve{pub fn active(&self)->bool{[self.highlights,self.lights,self.darks,self.shadows].iter().any(|v|*v!=0.0)}}

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",default)]
pub struct ColorGrading {
    pub shadows:photocraft_algo::camera_raw::Wheel,pub midtones:photocraft_algo::camera_raw::Wheel,
    pub highlights:photocraft_algo::camera_raw::Wheel,pub global:photocraft_algo::camera_raw::Wheel,
    pub blending:f64,pub balance:f64,
}
impl Default for ColorGrading {fn default()->Self{Self{shadows:Default::default(),midtones:Default::default(),highlights:Default::default(),global:Default::default(),blending:50.0,balance:0.0}}}
impl ColorGrading {pub fn active(&self)->bool{[self.shadows,self.midtones,self.highlights,self.global].iter().any(|w|w.sat!=0.0||w.lum!=0.0)}}

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
pub struct PointColor {pub id:String,pub enabled:bool,pub color:String,pub hue_range:f32,pub saturation_range:f32,pub luminance_range:f32,pub hue:f32,pub saturation:f32,pub luminance:f32}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct PrimaryShift {pub hue:f32,pub saturation:f32}

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct ColorBalance {
    pub shadows:[f64;3],pub midtones:[f64;3],pub highlights:[f64;3],pub preserve_luminosity:bool,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct BlackWhite {
    pub enabled:bool,pub weights:[f64;8],pub tint:Option<String>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct SelectiveColor {
    pub relative:bool,pub ranges:std::collections::BTreeMap<String,[f64;4]>,
}

/// Local coverage is saved in the lens-corrected source frame, before crop/rotation.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub enum MaskKind { Brush, Linear, Radial, Range }

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct MaskRange {pub kind:String,pub color:String,pub tolerance:f32,pub low:f32,pub high:f32,pub smoothness:f32}

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
pub struct MaskComponent {pub operation:photocraft_algo::selection::SelectionMode,pub mask:Box<LocalMask>}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all="camelCase")]
pub struct LocalMask {
    pub id: String,
    pub name: String,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub components:Vec<MaskComponent>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub overlay_color:Option<String>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub range:Option<MaskRange>,
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

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct RetouchStroke {pub id:String,pub kind:String,pub source:NormPoint,pub points:Vec<NormPoint>,pub width_rel:f64,pub hardness:f64,pub opacity:f64}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecipe {
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub brand_frame:Option<super::brand_frame::BrandFrame>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub lens:Option<super::lens::Settings>,
    #[serde(default,skip_serializing_if="Vec::is_empty")]
    pub retouch:Vec<RetouchStroke>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub color_pipeline:Option<ColorPipeline>,
    pub version: u32,
    /// 缺省使用旧算法，防止历史配方打开后改变效果。新配方显式选择 PhotoCraft。
    #[serde(default="default_renderer")]
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

#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub struct ColorPipeline {
    pub version:u32,
    pub backend:ColorBackend,
    pub core_revision:String,
    pub base_profile:BaseProfile,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub raw_white_balance:Option<RawWhiteBalance>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub lens_correction:Option<super::lens::ProfileSettings>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub relative_white_balance:Option<[f32;3]>,
    #[serde(default,skip_serializing_if="Option::is_none")]
    pub film:Option<LutSelection>,
}
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase",tag="mode")]
pub enum RawWhiteBalance {AsShot,Auto,Temperature{kelvin:f32,tint:f32}}
#[derive(Debug,Clone,Copy,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub enum BaseProfile { Standard, Soft, Neutral }
#[derive(Debug,Clone,PartialEq,Serialize,Deserialize)]
#[serde(rename_all="camelCase")]
pub enum ColorBackend {Rawlab}

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
    super::point_color::normalize(a)?;
    if a.disabled_groups.len()>14||a.disabled_groups.iter().any(|v|!super::groups::NAMES.contains(&v.as_str())){return Err("调整组开关无效".into());}
    fn numbers(values:&mut [f64],min:f64,max:f64)->Result<(),String>{
        for v in values {if !v.is_finite(){return Err("调色数值无效".into());}*v=v.clamp(min,max);}Ok(())
    }
    if let Some(c)=&mut a.parametric_curve {
        for v in [&mut c.highlights,&mut c.lights,&mut c.darks,&mut c.shadows]{numbers(std::slice::from_mut(v),-100.0,100.0)?;}
        if !c.splits.iter().all(|v|v.is_finite()){return Err("曲线分区边界无效".into());}
        c.splits[0]=c.splits[0].clamp(5.0,93.0);c.splits[1]=c.splits[1].clamp(c.splits[0]+1.0,94.0);c.splits[2]=c.splits[2].clamp(c.splits[1]+1.0,95.0);
    }
    if let Some(g)=&mut a.color_grading {
        for wheel in [&mut g.shadows,&mut g.midtones,&mut g.highlights,&mut g.global] {
            if ![wheel.hue,wheel.sat,wheel.lum].iter().all(|v|v.is_finite()){return Err("色彩分级数值无效".into());}
            wheel.hue=wheel.hue.clamp(0.0,360.0);wheel.sat=wheel.sat.clamp(0.0,100.0);wheel.lum=wheel.lum.clamp(-100.0,100.0);
        }
        numbers(std::slice::from_mut(&mut g.blending),0.0,100.0)?;numbers(std::slice::from_mut(&mut g.balance),-100.0,100.0)?;
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
    if let Some(ranges)=&mut a.hsl_eight{
        for (name,range) in ranges{
            if !super::color_mixer::BANDS.contains(&name.as_str()){return Err("八色 HSL 范围无效".into());}
            numbers(std::slice::from_mut(&mut range.hue),-100.0,100.0)?;
            numbers(std::slice::from_mut(&mut range.saturation),-100.0,100.0)?;
            numbers(std::slice::from_mut(&mut range.lightness),-100.0,100.0)?;
        }
    }
    Ok(())
}

fn normalize_mask(mask:&mut LocalMask,point_count:&mut usize,count:&mut usize,depth:usize)->Result<(),String>{
    *count+=1;
    if depth>4||*count>128||mask.name.len()>512||mask.id.is_empty()||mask.id.len()>96{return Err("蒙版组件过多或嵌套过深".into());}
    super::masks::normalize_range(mask)?;
    if mask.overlay_color.as_ref().is_some_and(|v|parse_color(v).is_none()){return Err("蒙版叠加颜色无效".into());}
        for point in [&mut mask.from,&mut mask.to] {
            if !point.x.is_finite() || !point.y.is_finite() {return Err("蒙版坐标无效".into());}
            point.x=clamp01(point.x);point.y=clamp01(point.y);
        }
        for v in [&mut mask.density,&mut mask.feather] {
            if !v.is_finite() {return Err("蒙版数值无效".into());}
            *v=v.clamp(0.0,100.0);
        }
        for stroke in &mut mask.strokes {
            *point_count=point_count.saturating_add(stroke.points.len());
            if *point_count>100_000 || stroke.points.is_empty() {return Err("蒙版笔迹无效或过长".into());}
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
    for component in &mut mask.components{normalize_mask(&mut component.mask,point_count,count,depth+1)?;}
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
    if let Some(frame)=&recipe.brand_frame{super::brand_frame::validate(frame)?;}
    if let Some(settings)=&mut recipe.lens{super::lens::normalize(settings)?;}
    if let Some(p)=&mut recipe.color_pipeline {
        if p.version!=2||p.core_revision!=super::rawlab::core_revision()||recipe.renderer!=Some(RenderEngine::Photocraft){return Err("此显影配方的 core 版本或编辑引擎不受支持".into());}
        if let Some(lens)=&p.lens_correction{super::lens::validate_profile(lens)?;}
        if let Some(RawWhiteBalance::Temperature{kelvin,tint})=&p.raw_white_balance{
            if !kelvin.is_finite()||!(2000.0..=50000.0).contains(kelvin)||!tint.is_finite()||!(-150.0..=150.0).contains(tint){return Err("RAW 色温或色调无效".into());}
        }
        if let Some(gains)=&p.relative_white_balance{if !gains.iter().all(|v|v.is_finite()&&(0.05..=20.0).contains(v)){return Err("中性校正增益无效".into());}}
        if let Some(f)=p.film.as_mut(){super::film::validate_id(&f.id)?;if !f.amount.is_finite(){return Err("胶片强度无效".into());}f.amount=f.amount.clamp(0.0,100.0);}
    }
    super::retouch::normalize(&mut recipe.retouch,recipe.renderer)?;
    if recipe.masks.len()>32 {return Err("蒙版数量超过上限".into());}
    if !recipe.masks.is_empty() && recipe.renderer!=Some(RenderEngine::Photocraft) {
        return Err("局部蒙版需要使用高级编辑引擎".into());
    }
    let mut ids=std::collections::HashSet::new();
    let mut point_count=0usize;
    let mut component_count=0usize;
    for mask in &mut recipe.masks {
        if mask.id.is_empty() || mask.id.len()>96 || !ids.insert(mask.id.clone()) || mask.name.len()>512 {
            return Err("蒙版名称或标识无效".into());
        }
        normalize_mask(mask,&mut point_count,&mut component_count,0)?;
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
    if recipe.text_layers.len()>128||recipe.brush_strokes.len()>2048||recipe.brush_strokes.iter().map(|s|s.points.len()).sum::<usize>()>200_000{return Err("标注图层或点数超过上限".into());}
    for layer in &mut recipe.text_layers {
        if layer.font.len()>200||layer.text.len()>32000||!layer.opacity.is_finite()||!(0.0..=1.0).contains(&layer.opacity)||!["left","center","right"].contains(&layer.align.as_str()){return Err("文字字体、对齐或不透明度无效".into());}
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
        if !stroke.opacity.is_finite()||!(0.0..=1.0).contains(&stroke.opacity){return Err("画笔不透明度无效".into());}
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
    fn parametric_curve_boundaries_are_ordered_and_use_upstream_limits() {
        let r=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"parametricCurve":{"shadows":200,"darks":-200,"splits":[99,2,-10]}}})).unwrap();
        let c=r.advanced.as_ref().unwrap().parametric_curve.as_ref().unwrap();assert_eq!(c.splits,[93.0,94.0,95.0]);assert_eq!(c.shadows,100.0);assert_eq!(c.darks,-100.0);
        assert_eq!(parse_recipe(&serde_json::to_value(&r).unwrap()).unwrap(),r);
    }
    #[test]
    fn grading_defaults_and_bounds_are_normalized() {
        let recipe=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"colorGrading":{"midtones":{"hue":999,"sat":200,"lum":-200},"balance":-999,"blending":999}}})).unwrap();
        let g=recipe.advanced.as_ref().unwrap().color_grading.as_ref().unwrap();assert_eq!(g.midtones.hue,360.0);assert_eq!(g.midtones.sat,100.0);assert_eq!(g.midtones.lum,-100.0);assert_eq!(g.balance,-100.0);assert_eq!(g.blending,100.0);
        assert_eq!(parse_recipe(&serde_json::to_value(&recipe).unwrap()).unwrap(),recipe);
        assert!(!ColorGrading::default().active());
    }
    #[test]
    fn development_defaults_ranges_and_style_are_validated() {
        let defaults=serde_json::to_value(Development::default()).unwrap();
        let upstream=serde_json::to_value(photocraft_algo::camera_raw::CameraRaw::default()).unwrap();
        for (key,value) in defaults.as_object().unwrap(){assert_eq!(upstream[key],*value,"default must match upstream: {key}");}
        let mut body=serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"development":{"shadows":200,"sharpenRadius":99,"noiseColor":999,"grainAmount":-10}}});
        let recipe=parse_recipe(&body).unwrap();let d=recipe.advanced.unwrap().development.unwrap();
        assert_eq!(d.shadows,100.0);assert_eq!(d.sharpen_radius,3.0);assert_eq!(d.noise_color,100.0);assert_eq!(d.grain_amount,0.0);
        body["advanced"]["development"]["vignetteStyle"]=serde_json::json!("invalid");assert!(parse_recipe(&body).is_err());
    }
    #[test]
    fn extended_color_validates_ranges_colors_and_roundtrips_defaults() {
        let recipe=parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"colorBalance":{"shadows":[-200,0,150],"midtones":[0,0,0],"highlights":[0,0,0],"preserveLuminosity":true},"blackWhite":{"enabled":false,"weights":[-999,50,60,40,60,20,50,999],"tint":null},"selectiveColor":{"relative":false,"ranges":{"reds":[150,0,0,-150]}}}})).unwrap();
        let a=recipe.advanced.as_ref().unwrap();assert_eq!(a.color_balance.as_ref().unwrap().shadows,[-100.0,0.0,100.0]);
        assert_eq!(a.black_white.as_ref().unwrap().weights[0],-200.0);assert_eq!(a.black_white.as_ref().unwrap().weights[7],300.0);
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
