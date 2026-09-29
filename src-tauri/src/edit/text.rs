//! 文字与笔迹栅格化（阶段 D）。
//!
//! 字体策略（用户定案：**不得捆绑字体文件**，避免再分发许可问题）：运行时
//! 从系统字体目录加载——Windows 依次尝试 `C:\Windows\Fonts` 的
//! msyh.ttc（微软雅黑，中英全覆盖）→ simhei.ttf（黑体）→ simsun.ttc；
//! 非 Windows 尝试 Noto CJK / DejaVu 常见路径。TTC 集合用 ab_glyph 原生
//! `try_from_vec_and_index(…, 0)` 解析（**不能手工切字节**：TTC 内表偏移是
//! 文件绝对偏移，切片后全部错位——msyh.ttc 实测 InvalidFont 回退 simhei，
//! 宽度/基线与前端预览完全对不上）。全部失败 → **静态回退**：每行画一个
//! 等尺寸的空心矩形占位（导出不因缺字体失败，标注位置仍可见）。
//! 字体进程级缓存（OnceLock），首次渲染后零 IO。
//!
//! **基线公式与前端 Konva Text 逐像素对齐**（konva/lib/shapes/Text.js 非
//! legacy 分支）：首行字母基线 = `y + (ascent - descent)/2 + lineHeightPx/2`，
//! 行进 = `lineHeight × em`；其中 lineHeight=1.25（前端 Konva.Text lineHeight
//! 同值），ascent/descent 取 hhea（浏览器 canvas fontBoundingBox 同源，
//! msyh 100px 实测 fba/fbd=106/26 ≈ hhea 2167/541）。见 konvaMapping/EditorCanvas。
//!
//! 笔迹 = 折线 + 圆头端点/连接 + 等宽：沿路径按半径步长密集盖印实心圆
//! （单点 = 圆点），天然满足圆头/圆连接语义。

use std::sync::OnceLock;

use ab_glyph::{Font, FontArc, FontVec, PxScale, ScaleFont};

use super::recipe::{BrushStroke, TextLayer};

/// 与前端 Konva.Text lineHeight={1.25} 对齐的行高倍数。
const LINE_HEIGHT: f32 = 1.25;

/// 进程级字体缓存（None = 已尝试且全部失败，走静态回退）。
fn font() -> Option<&'static FontArc> {
    static FONT: OnceLock<Option<FontArc>> = OnceLock::new();
    FONT.get_or_init(|| {
        for path in crate::platform::font_candidates() {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if let Ok(font) = FontVec::try_from_vec_and_index(bytes, 0) {
                return Some(FontArc::new(font));
            }
        }
        None
    })
    .as_ref()
}

/// 坐标换算基准：文字/笔迹的 x,y,w 全部归一化到**画布宽**（契约 v1）。
#[inline]
fn to_px(norm: f64, canvas_w: u32) -> f32 {
    (norm * f64::from(canvas_w)) as f32
}

/// 画一个文字图层（含多行）。坐标越界的部分自然被画布裁掉。
pub fn draw_text_layer(canvas: &mut image::RgbImage, layer: &TextLayer) {
    let w = canvas.width();
    let h = canvas.height();
    let color = super::recipe::parse_color(&layer.color).unwrap_or([255, 255, 255]);
    let x_px = to_px(layer.x, w);
    let y_px = to_px(layer.y, w);
    let em = to_px(layer.size_rel, w).max(1.0);
    let Some(font) = font().cloned() else {
        draw_text_fallback(canvas, &layer.text, x_px, y_px, em, color);
        return;
    };
    // ab_glyph 的 PxScale.y 语义 = hhea (ascent+descent) 盒的像素高，**不是** CSS
    // fontSize 的 em（msyh 实测：scale=100 时 ascent+descent 恰为 100、CJK 字宽
    // 75.77px 而浏览器同字号为 100px）。FontArc 不暴露原始 hhea 单位，用全角
    // 字符（advance=upem，如 U+3000）在 scale=em 下的实测字宽反推修正系数
    // factor = span/upem（msyh = 2708/2048 ≈ 1.322），使 h_advance 与浏览器
    // fontSize=em 逐像素对齐。夹取防御异常字体度量。
    let probe_scale = PxScale { x: em, y: em };
    let probe = font.clone().into_scaled(probe_scale);
    let mut factor = 1.0f32;
    for ch in ['\u{3000}', '\u{9a8c}'] {
        let adv = probe.h_advance(probe.glyph_id(ch));
        if adv > 1.0 {
            factor = em / adv;
            break;
        }
    }
    factor = factor.clamp(0.5, 2.5);
    let scale = PxScale { x: em * factor, y: em * factor };
    let scaled = font.into_scaled(scale);
    let ascent = scaled.ascent();
    // ab_glyph 的 descent 为负（height = ascent - descent）；Konva 公式里的
    // descent 是正的 fontBoundingBoxDescent，故此处用 (asc + desc)/2 等价之
    let descent = -scaled.descent();
    // 首行基线与 Konva Text 对齐：(asc - desc)/2 + lineHeight·em/2；行进 = 1.25·em
    let mut baseline = y_px + (ascent - descent) / 2.0 + LINE_HEIGHT * em / 2.0;
    for line in layer.text.split('\n') {
        let mut cursor_x = x_px;
        let mut previous: Option<ab_glyph::GlyphId> = None;
        for ch in line.chars() {
            let glyph_id = scaled.glyph_id(ch);
            if let Some(prev) = previous {
                cursor_x += scaled.kern(prev, glyph_id);
            }
            previous = Some(glyph_id);
            let glyph = ab_glyph::Glyph {
                id: glyph_id,
                scale,
                position: ab_glyph::Point {
                    x: cursor_x,
                    y: baseline,
                },
            };
            if let Some(outlined) = scaled.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|x, y, coverage| {
                    let px = bounds.min.x as i64 + i64::from(x);
                    let py = bounds.min.y as i64 + i64::from(y);
                    if px < 0 || py < 0 || px >= i64::from(w) || py >= i64::from(h) {
                        return;
                    }
                    blend_pixel(canvas, px as u32, py as u32, color, coverage);
                });
            }
            cursor_x += scaled.h_advance(glyph_id);
        }
        baseline += LINE_HEIGHT * em;
    }
}

/// 静态回退（系统无可用字体）：每行一个空心矩形占位（高=em，宽=0.6em×
/// 字符数，至少 0.6em）——位置与占位可见，导出不失败。
fn draw_text_fallback(
    canvas: &mut image::RgbImage,
    text: &str,
    x: f32,
    y: f32,
    em: f32,
    color: [u8; 3],
) {
    let (w, h) = (canvas.width(), canvas.height());
    let line_h = (em * 1.2) as i32;
    for (index, line) in text.split('\n').enumerate() {
        let x0 = x as i32;
        let y0 = y as i32 + index as i32 * line_h;
        let width = (((em * 0.6) as i32).max(1)) * line.chars().count().max(1) as i32;
        for dx in 0..=width {
            for dy in 0..=line_h {
                if dx == 0 || dy == 0 || dx == width || dy == line_h {
                    let (px, py) = (x0 + dx, y0 + dy);
                    if px >= 0 && py >= 0 && (px as u32) < w && (py as u32) < h {
                        canvas.put_pixel(px as u32, py as u32, image::Rgb(color));
                    }
                }
            }
        }
    }
}

/// 画一条笔迹（折线，圆头/圆连接，等宽）。
pub fn draw_brush_stroke(canvas: &mut image::RgbImage, stroke: &BrushStroke) {
    let w = canvas.width();
    let h = canvas.height();
    let color = super::recipe::parse_color(&stroke.color).unwrap_or([255, 255, 255]);
    let radius = (to_px(stroke.width_rel, w) / 2.0).max(0.5);
    let points: Vec<(f32, f32)> = stroke
        .points
        .iter()
        .map(|p| (to_px(p.x, w), to_px(p.y, w)))
        .collect();
    if points.is_empty() {
        return;
    }
    // 沿段密集盖印：步长 = 半径（相邻圆覆盖 ≥50%，视觉连续无断裂）
    let step = radius.max(0.5);
    stamp_circle(canvas, w, h, points[0], radius, color);
    for pair in points.windows(2) {
        let (x0, y0) = pair[0];
        let (x1, y1) = pair[1];
        let length = (x1 - x0).hypot(y1 - y0);
        let steps = ((length / step).ceil() as u32).max(1);
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            stamp_circle(
                canvas,
                w,
                h,
                (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t),
                radius,
                color,
            );
        }
    }
}

/// 实心圆盖印（整像素判定，无抗锯齿——标注线宽通常 ≥ 数像素，锯齿不可见）。
fn stamp_circle(
    canvas: &mut image::RgbImage,
    w: u32,
    h: u32,
    center: (f32, f32),
    radius: f32,
    color: [u8; 3],
) {
    let (cx, cy) = (center.0 as f64, center.1 as f64);
    let r = f64::from(radius);
    let x_min = ((cx - r).floor() as i32).max(0);
    let x_max = ((cx + r).ceil() as i32).min(w as i32 - 1);
    let y_min = ((cy - r).floor() as i32).max(0);
    let y_max = ((cy + r).ceil() as i32).min(h as i32 - 1);
    for px in x_min..=x_max {
        for py in y_min..=y_max {
            let dx = f64::from(px) - cx;
            let dy = f64::from(py) - cy;
            if dx * dx + dy * dy <= r * r {
                canvas.put_pixel(px as u32, py as u32, image::Rgb(color));
            }
        }
    }
}

/// 反锯齿混色（coverage 0..1，coverage=1 直接覆盖）。
fn blend_pixel(canvas: &mut image::RgbImage, x: u32, y: u32, color: [u8; 3], coverage: f32) {
    if coverage <= 0.0 {
        return;
    }
    if coverage >= 1.0 {
        canvas.put_pixel(x, y, image::Rgb(color));
        return;
    }
    let pixel = canvas.get_pixel_mut(x, y);
    for (channel, value) in pixel.0.iter_mut().zip(color) {
        *channel =
            (*channel as f32 * (1.0 - coverage) + f32::from(value) * coverage).round() as u8;
    }
}

