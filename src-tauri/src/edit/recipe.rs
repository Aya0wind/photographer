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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecipe {
    pub version: u32,
    #[serde(default)]
    pub rotate_quarter: u32,
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
