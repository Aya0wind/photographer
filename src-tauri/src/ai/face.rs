//! 人脸全链路（M4）：SCRFD 343G 检测 + 5 关键点对齐 + ArcFace 512 维特征 +
//! 在线聚类（index_tasks kind="face" 通道）。
//!
//! ## 预处理 / 解码约定（immich-app 官方 ONNX 用法，2026-09 核对
//! immich_ml/models/facial_recognition/{detection,_ops}.py）
//! - 检测输入：letterbox 640×640（保比缩放贴左上角，余下补黑）→
//!   `(px - 127.5) / 128`，NCHW f32 RGB。
//! - 输出 9 头（stride 8/16/32）：score `[1,N,1]`、box 距离 `[1,N,4]`、
//!   kps 偏移 `[1,N,10]`；每格 anchor-major 布局（中心重复 anchors 次），
//!   `N` 降序即 stride 升序。解码：`distance × stride` 与 anchor 中心加减
//!   得框；kps = 中心 + `offset × stride`；NMS（IoU 0.4）后按 letterbox
//!   缩放还原到原图坐标。
//! - 对齐：5 关键点 → ArcFace 112×112 canonical 模板（Umeyama 2D 相似
//!   变换闭式解 + 双线性逆向采样）。
//! - 识别（garavv/arcface-onnx 模型卡 pin 死）：112×112 NHWC RGB
//!   `[1,112,112,3]`，`(px - 127.5) / 128` → 512 维 → L2 归一化。
//!
//! ## 在线聚类（v1，用户定案 2026-09-19）
//! 新脸与现有簇心（成员归一化向量的均值）最大 cos ≥ [`CLUSTER_COS_THRESHOLD`]
//! → 归簇（簇心滑动平均更新）；否则新建簇。簇人数 > 500 的近簇合并 v1 **不做**
//! （误合并不可逆，等 M8 真库观察误聚率再定，见 [`OnlineClusterer`] 注释）。
//! 聚类为顺序敏感的单遍状态 → face 通道单 worker 串行；簇心缓存在进程内
//! （重启后从 faces 表重建）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use image::RgbImage;
use ort::session::Session;
use ort::value::Tensor;

use super::ModelManager;
use crate::db::Db;
use crate::events::{AppEvent, EventBus};

/// ArcFace 输出维度。
pub const FACE_EMBED_DIM: usize = 512;
/// 在线聚类归簇阈值（与簇心的最大 cos 相似度；可配常量）。
#[doc(hidden)]
#[allow(dead_code)] // face_test 引用（lib 内已由 settings 参数取代）
pub const CLUSTER_COS_THRESHOLD: f32 = 0.4;
/// 检测置信度下限（immich 默认 0.5 档）。
#[doc(hidden)]
#[allow(dead_code)] // face_test 引用（lib 内已由 settings 参数取代）
pub const DETECT_MIN_SCORE: f32 = 0.5;
/// NMS IoU 阈值（immich _ops.py 同值）。
const NMS_IOU: f32 = 0.4;
/// 检测输入边长（DET_SIZE）。
pub const DET_SIZE: u32 = 640;
/// 对齐裁剪边长（ALIGNED_SIZE）。
pub const ALIGNED_SIZE: u32 = 112;
/// FPN 步长（DET_STRIDES）。
const DET_STRIDES: [u32; 3] = [8, 16, 32];
/// 单图最多入库人脸数（防极端群像刷爆 faces 表）。
pub const MAX_FACES_PER_IMAGE: usize = 16;
/// ArcFace 5 点模板（insightface canonical，112×112）：
/// 左眼 / 右眼 / 鼻尖 / 左嘴角 / 右嘴角。
const ARCFACE_DST: [[f32; 2]; 5] = [
    [38.2946, 51.6963],
    [73.5318, 51.5014],
    [56.0252, 71.7366],
    [41.5493, 92.3655],
    [70.7299, 92.2041],
];

/// 检出人脸（原图坐标；box 为 x/y/w/h，kps 为 5 关键点）。
#[derive(Debug, Clone, PartialEq)]
pub struct DetectedFace {
    pub box_x: f32,
    pub box_y: f32,
    pub box_w: f32,
    pub box_h: f32,
    pub score: f32,
    pub kps: [[f32; 2]; 5],
}

// ---------------------------------------------------------------------------
// 会话槽（惰性加载；与 embed.rs 同款互斥串行约定）
// ---------------------------------------------------------------------------

#[derive(Default)]
struct FaceSlots {
    det: Option<Session>,
    rec: Option<Session>,
}

fn face_slots() -> &'static Mutex<FaceSlots> {
    static SLOTS: std::sync::OnceLock<Mutex<FaceSlots>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(FaceSlots::default()))
}

impl ModelManager {
    /// 人脸两件套（scrfd + arcface）是否齐备。
    pub fn face_models_ready(&self) -> bool {
        ["scrfd", "arcface"]
            .iter()
            .all(|id| self.model_path(id).is_file())
    }

    /// 闭眼检测模型是否就绪（feature="selection" 清单条目全部落盘）。
    /// 今日清单无 selection 条目 → 恒 false：eyes 通道跳过并计数，
    /// 任务保持 pending（模型收录后 kick 自然续跑）。
    pub fn selection_eyes_ready(&self) -> bool {
        let ids: Vec<String> = super::catalog()
            .iter()
            .filter(|e| e.feature == "selection")
            .map(|e| e.id.clone())
            .collect();
        !ids.is_empty() && ids.iter().all(|id| self.model_path(id).is_file())
    }

    fn ensure_det(&self, slots: &mut FaceSlots) -> Result<(), String> {
        if slots.det.is_some() {
            return Ok(());
        }
        let path = self.model_path("scrfd");
        if !path.is_file() {
            return Err("模型 scrfd 未下载（设置页下载后再试）".into());
        }
        // EP 序列与 embed 共用（use_gpu → [DML, CPU]；DML 失败自动落 CPU）
        let session = super::build_session(&path, None, self.ai_params().use_gpu, "scrfd")?;
        slots.det = Some(session);
        Ok(())
    }

    fn ensure_rec(&self, slots: &mut FaceSlots) -> Result<(), String> {
        if slots.rec.is_some() {
            return Ok(());
        }
        let path = self.model_path("arcface");
        if !path.is_file() {
            return Err("模型 arcface 未下载（设置页下载后再试）".into());
        }
        let session = super::build_session(&path, None, self.ai_params().use_gpu, "arcface")?;
        slots.rec = Some(session);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// letterbox + 检测
// ---------------------------------------------------------------------------

/// letterbox：保比缩放到 size 内、贴左上角、余下补黑；返回 (画布, 缩放比)。
/// 缩放比 = size / max(w, h)（immich transforms.letterbox 同口径）。
pub fn letterbox(img: &RgbImage, size: u32) -> (RgbImage, f32) {
    let (w, h) = img.dimensions();
    let scale = size as f32 / w.max(h) as f32;
    let (nw, nh) = ((w as f32 * scale) as u32, (h as f32 * scale) as u32);
    let resized = image::imageops::resize(
        img,
        nw.max(1),
        nh.max(1),
        image::imageops::FilterType::Triangle,
    );
    let mut canvas = RgbImage::new(size, size);
    image::imageops::overlay(&mut canvas, &resized, 0, 0);
    (canvas, scale)
}

/// SCRFD 检测（RGB8 输入 → 原图坐标人脸列表，score 降序）。
pub fn detect_faces(manager: &ModelManager, img: &RgbImage) -> Result<Vec<DetectedFace>, String> {
    // 置信门槛（settings.ai.face_detect_threshold，默认 DETECT_MIN_SCORE）
    let threshold = manager.ai_params().face_detect_threshold;
    let (canvas, scale) = letterbox(img, DET_SIZE);
    let mut data = Vec::with_capacity(3 * DET_SIZE as usize * DET_SIZE as usize);
    for ch in 0..3 {
        for px in canvas.pixels() {
            data.push(px.0[ch] as f32 / 128.0 - 127.5 / 128.0);
        }
    }
    // DML 运行时故障同 embed 语义：毒化 + 纯 CPU 重建重跑一次
    let use_gpu = manager.ai_params().use_gpu;
    let heads = super::run_with_dml_fallback(
        use_gpu,
        || {
            let tensor = Tensor::from_array((
                vec![1i64, 3, DET_SIZE as i64, DET_SIZE as i64],
                data.clone(),
            ))
            .map_err(|e| format!("构造检测张量失败: {e}"))?;
            let mut slots = face_slots().lock().expect("face slots mutex poisoned");
            manager.ensure_det(&mut slots)?;
            let session = slots.det.as_mut().expect("ensure_det 已保证");
            let input_name = session
                .inputs()
                .first()
                .map(|i| i.name().to_string())
                .unwrap_or_else(|| "input".into());
            let output_names: Vec<String> = session
                .outputs()
                .iter()
                .map(|o| o.name().to_string())
                .collect();
            let outputs = session
                .run(ort::inputs![input_name => tensor])
                .map_err(|e| format!("SCRFD 推理失败: {e}"))?;
            collect_heads(&outputs, &output_names)
        },
        || {
            let mut slots = face_slots().lock().expect("face slots mutex poisoned");
            slots.det = None;
        },
    )?;

    let (mut boxes, mut kps_all, mut scores) = (Vec::new(), Vec::new(), Vec::new());
    decode_heads(&heads, &mut boxes, &mut kps_all, &mut scores);

    // 阈值过滤 + 还原到原图坐标
    let mut faces: Vec<DetectedFace> = boxes
        .into_iter()
        .zip(kps_all)
        .zip(scores)
        .filter_map(|((box4, kps), score)| {
            let [x1, y1, x2, y2] = box4;
            (score >= threshold).then(|| DetectedFace {
                box_x: x1 / scale,
                box_y: y1 / scale,
                box_w: (x2 - x1) / scale,
                box_h: (y2 - y1) / scale,
                score,
                kps: kps.map(|p| [p[0] / scale, p[1] / scale]),
            })
        })
        .collect();
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    Ok(nms(faces, NMS_IOU))
}

/// 单个 ONNX 输出：（shape, data）。
type Head = (Vec<i64>, Vec<f32>);

/// ONNX 输出收集：与输出名序一致地提 (shape, data)。
fn collect_heads(
    outputs: &ort::session::SessionOutputs<'_>,
    names: &[String],
) -> Result<Vec<Head>, String> {
    let mut heads = Vec::with_capacity(names.len());
    for name in names {
        let value = outputs
            .get(name)
            .ok_or_else(|| format!("输出 {name} 不存在"))?;
        let (shape, data) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| format!("输出 {name} 解析失败: {e}"))?;
        heads.push((shape.to_vec(), data.to_vec()));
    }
    Ok(heads)
}

/// 行数 N（rank-2 [N,C] → shape[0]；rank-3 [1,N,C] → shape[1]）。
fn head_rows(shape: &[i64]) -> usize {
    shape
        .len()
        .checked_sub(2)
        .and_then(|i| shape.get(i))
        .copied()
        .unwrap_or(0) as usize
}

/// 输出分类：shape 末维 1/4/10 → score/box/kps，组内 N 降序 = stride 升序；
/// 返回解码取用序 [s8, s16, s32, b8, b16, b32, k8, k16, k32]（heads 下标）。
/// 形状不可判（不足 9 头 / 命名异常导出）时按 immich 导出位序兜底：
/// level, level+3, level+6。
fn split_heads(heads: &[(Vec<i64>, Vec<f32>)]) -> [usize; 9] {
    let group_of = |want_last: i64| -> [usize; 3] {
        let mut group: Vec<usize> = (0..heads.len())
            .filter(|i| {
                let (shape, _) = &heads[*i];
                shape.len() >= 2 && shape.last().is_some_and(|d| *d == want_last)
            })
            .collect();
        group.sort_by_key(|i| std::cmp::Reverse(head_rows(&heads[*i].0)));
        let mut out = [usize::MAX; 3];
        for (slot, idx) in out.iter_mut().zip(group) {
            *slot = idx;
        }
        out
    };
    let (scores, boxes, kps) = (group_of(1), group_of(4), group_of(10));
    let complete = scores
        .iter()
        .chain(boxes.iter())
        .chain(kps.iter())
        .all(|i| *i < usize::MAX);
    let mut order = [usize::MAX; 9];
    if complete {
        for (slot, v) in order
            .iter_mut()
            .zip(scores.into_iter().chain(boxes).chain(kps))
        {
            *slot = v;
        }
        return order;
    }
    for (idx, slot) in order.iter_mut().enumerate() {
        *slot = idx.min(heads.len().saturating_sub(1));
    }
    order
}

/// anchor 解码：每级 grid × grid × anchors，anchor-major（中心重复 anchors
/// 次）；distance/offset × stride 还原；kps 逐点（中心平铺 5 次 + 偏移）。
fn decode_heads(
    heads: &[(Vec<i64>, Vec<f32>)],
    boxes: &mut Vec<[f32; 4]>,
    kps: &mut Vec<[[f32; 2]; 5]>,
    scores: &mut Vec<f32>,
) {
    let order = split_heads(heads);
    for (level, stride) in DET_STRIDES.iter().enumerate() {
        let Some(&si) = order.get(level) else {
            continue;
        };
        let Some(&bi) = order.get(level + 3) else {
            continue;
        };
        let Some(&ki) = order.get(level + 6) else {
            continue;
        };
        let (Some(sn), Some(sd)) = (heads.get(si), heads.get(bi)) else {
            continue;
        };
        let Some((_kn, kd)) = heads.get(ki) else {
            continue;
        };
        let grid = (DET_SIZE / stride) as usize;
        let cells = grid * grid;
        let anchors = head_rows(&sn.0) / cells.max(1);
        if anchors == 0 {
            continue;
        }
        for cell in 0..cells {
            let (cx, cy) = ((cell % grid) as f32, (cell / grid) as f32);
            let (bx, by) = (cx * *stride as f32, cy * *stride as f32);
            for a in 0..anchors {
                let row = cell * anchors + a;
                let s = sn.1.get(row).copied().unwrap_or(0.0);
                let d = &sd.1[row * 4..(row + 1) * 4];
                boxes.push([
                    bx - d[0] * *stride as f32,
                    by - d[1] * *stride as f32,
                    bx + d[2] * *stride as f32,
                    by + d[3] * *stride as f32,
                ]);
                let mut points = [[0f32; 2]; 5];
                for (j, point) in points.iter_mut().enumerate() {
                    let off = &kd[row * 10 + j * 2..row * 10 + j * 2 + 2];
                    *point = [bx + off[0] * *stride as f32, by + off[1] * *stride as f32];
                }
                kps.push(points);
                scores.push(s);
            }
        }
    }
}

/// 贪心 NMS（IoU 阈值；输入需已按 score 降序）。
fn nms(faces: Vec<DetectedFace>, iou_threshold: f32) -> Vec<DetectedFace> {
    let mut kept: Vec<DetectedFace> = Vec::new();
    'outer: for face in faces {
        let (ax1, ay1, ax2, ay2) = (
            face.box_x,
            face.box_y,
            face.box_x + face.box_w,
            face.box_y + face.box_h,
        );
        for k in &kept {
            let (bx1, by1, bx2, by2) = (k.box_x, k.box_y, k.box_x + k.box_w, k.box_y + k.box_h);
            let ix = (ax1.max(bx1), ay1.max(by1), ax2.min(bx2), ay2.min(by2));
            let inter = (ix.2 - ix.0).max(0.0) * (ix.3 - ix.1).max(0.0);
            let union = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter;
            if union > 0.0 && inter / union > iou_threshold {
                continue 'outer;
            }
        }
        kept.push(face);
    }
    kept
}

// ---------------------------------------------------------------------------
// 对齐 + ArcFace 特征
// ---------------------------------------------------------------------------

/// 5 点 2D 相似变换闭式解（Umeyama；复数域最小二乘），返回 2×3 仿射矩阵
/// [[a, b, tx], [d, e, ty]]。退化（源点重合）返回 None。
pub fn similarity_matrix(src: &[[f32; 2]; 5], dst: &[[f32; 2]; 5]) -> Option<[[f32; 3]; 2]> {
    let sm = (
        src.iter().map(|p| p[0]).sum::<f32>() / 5.0,
        src.iter().map(|p| p[1]).sum::<f32>() / 5.0,
    );
    let dm = (
        dst.iter().map(|p| p[0]).sum::<f32>() / 5.0,
        dst.iter().map(|p| p[1]).sum::<f32>() / 5.0,
    );
    // c = Σ q' · conj(p') / Σ|p'|²（复数商：实部 dot、虚部叉积）
    let (mut re, mut im, mut norm) = (0f32, 0f32, 0f32);
    for (p, q) in src.iter().zip(dst) {
        let (px, py) = (p[0] - sm.0, p[1] - sm.1);
        let (qx, qy) = (q[0] - dm.0, q[1] - dm.1);
        re += qx * px + qy * py;
        im += qy * px - qx * py;
        norm += px * px + py * py;
    }
    if norm <= f32::EPSILON {
        return None;
    }
    let (scale, angle) = ((re * re + im * im).sqrt() / norm, im.atan2(re));
    let (cos, sin) = (angle.cos(), angle.sin());
    let (a, b, d, e) = (scale * cos, -scale * sin, scale * sin, scale * cos);
    Some([
        [a, b, dm.0 - (a * sm.0 + b * sm.1)],
        [d, e, dm.1 - (d * sm.0 + e * sm.1)],
    ])
}

/// 按 2×3 相似矩阵逆向映射 + 双线性采样，输出 ALIGNED_SIZE² 裁剪。
pub fn warp_similarity(img: &RgbImage, m: &[[f32; 3]; 2]) -> RgbImage {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let mut out = RgbImage::new(ALIGNED_SIZE, ALIGNED_SIZE);
    if det.abs() < f32::EPSILON {
        return out;
    }
    let inv = [
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ];
    for y in 0..ALIGNED_SIZE {
        for x in 0..ALIGNED_SIZE {
            let (dx, dy) = (x as f32 - m[0][2], y as f32 - m[1][2]);
            let sx = inv[0][0] * dx + inv[0][1] * dy;
            let sy = inv[1][0] * dx + inv[1][1] * dy;
            let x0 = sx.floor() as i64;
            let y0 = sy.floor() as i64;
            if x0 < 0 || y0 < 0 || x0 + 1 >= w || y0 + 1 >= h {
                continue; // 界外留黑
            }
            let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
            for ch in 0..3 {
                let px = |xx: i64, yy: i64| img.get_pixel(xx as u32, yy as u32).0[ch] as f32;
                let top = px(x0, y0) * (1.0 - fx) + px(x0 + 1, y0) * fx;
                let bot = px(x0, y0 + 1) * (1.0 - fx) + px(x0 + 1, y0 + 1) * fx;
                out.get_pixel_mut(x, y).0[ch] =
                    (top * (1.0 - fy) + bot * fy).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// 5 关键点对齐裁剪 112×112（模板 = ArcFace canonical）。
pub fn align_face(img: &RgbImage, kps: &[[f32; 2]; 5]) -> Option<RgbImage> {
    let m = similarity_matrix(kps, &ARCFACE_DST)?;
    Some(warp_similarity(img, &m))
}

impl ModelManager {
    /// ArcFace 特征：112×112 RGB → 512 维 L2 归一化向量。
    pub fn embed_aligned(&self, aligned: &RgbImage) -> Result<Vec<f32>, String> {
        // ArcFace 预处理（garavv/arcface-onnx 模型卡 pin 死）：NHWC RGB
        // [1,112,112,3]，`(px - 127.5) / 128`，输出 [1,512]。
        let mut data = Vec::with_capacity(3 * ALIGNED_SIZE as usize * ALIGNED_SIZE as usize);
        for px in aligned.pixels() {
            for ch in 0..3 {
                data.push(px.0[ch] as f32 / 128.0 - 127.5 / 128.0);
            }
        }
        // DML 运行时故障同 embed/det 语义：毒化 + 纯 CPU 重建重跑一次
        let use_gpu = self.ai_params().use_gpu;
        let vec = super::run_with_dml_fallback(
            use_gpu,
            || {
                let tensor = Tensor::from_array((
                    vec![1i64, ALIGNED_SIZE as i64, ALIGNED_SIZE as i64, 3],
                    data.clone(),
                ))
                .map_err(|e| format!("构造识别张量失败: {e}"))?;
                let mut slots = face_slots().lock().expect("face slots mutex poisoned");
                self.ensure_rec(&mut slots)?;
                let session = slots.rec.as_mut().expect("ensure_rec 已保证");
                let input_name = session
                    .inputs()
                    .first()
                    .map(|i| i.name().to_string())
                    .unwrap_or_else(|| "input".into());
                let output_names: Vec<String> = session
                    .outputs()
                    .iter()
                    .map(|o| o.name().to_string())
                    .collect();
                let outputs = session
                    .run(ort::inputs![input_name => tensor])
                    .map_err(|e| format!("ArcFace 推理失败: {e}"))?;
                // 取末维 = 512 的输出（不同导出版本输出名不一，按形状定位最稳）
                let mut picked: Option<Vec<f32>> = None;
                for name in &output_names {
                    let Some(value) = outputs.get(name) else {
                        continue;
                    };
                    let (shape, extracted) = value
                        .try_extract_tensor::<f32>()
                        .map_err(|e| format!("识别输出 {name} 解析失败: {e}"))?;
                    if shape.last().is_some_and(|d| *d as usize == FACE_EMBED_DIM) {
                        picked = Some(extracted.to_vec());
                        break;
                    }
                }
                picked.ok_or_else(|| {
                    format!("识别输出缺少 {FACE_EMBED_DIM} 维向量（现有: {output_names:?}）")
                })
            },
            || {
                let mut slots = face_slots().lock().expect("face slots mutex poisoned");
                slots.rec = None;
            },
        )?;
        Ok(normalize(vec))
    }

    /// 单图全链路：检测（截 MAX_FACES_PER_IMAGE）→ 对齐 → 特征。
    pub fn detect_and_embed(
        &self,
        img: &RgbImage,
    ) -> Result<Vec<(DetectedFace, Vec<f32>)>, String> {
        let faces = detect_faces(self, img)?;
        let mut out = Vec::with_capacity(faces.len().min(MAX_FACES_PER_IMAGE));
        for face in faces.into_iter().take(MAX_FACES_PER_IMAGE) {
            let Some(crop) = align_face(img, &face.kps) else {
                continue;
            };
            let emb = self.embed_aligned(&crop)?;
            out.push((face, emb));
        }
        Ok(out)
    }
}

fn normalize(mut v: Vec<f32>) -> Vec<f32> {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

// ---------------------------------------------------------------------------
// 在线聚类
// ---------------------------------------------------------------------------

/// 在线聚类器（簇心 = 成员归一化向量的均值；比较时再归一化，argmax cos）。
/// 簇人数 > 500 的近簇合并 v1 不做：误合并不可逆且无人工纠簇界面，
/// 等 M8 真库实测误聚率再定（当前阈值 0.4 下 Immich 同款经验无爆炸合并）。
#[derive(Debug, Default)]
pub struct OnlineClusterer {
    sums: HashMap<i64, (Vec<f32>, u32)>,
    threshold: f32,
}

impl OnlineClusterer {
    /// 空聚类器（测试构造入口；生产路径走 from_sums）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn new(threshold: f32) -> Self {
        Self {
            sums: HashMap::new(),
            threshold,
        }
    }

    /// 从持久化簇和恢复（重启后首脸重建缓存）。
    pub fn from_sums(sums: HashMap<i64, (Vec<f32>, u32)>, threshold: f32) -> Self {
        Self { sums, threshold }
    }

    /// 最佳匹配簇：(cluster_id, cos)。空聚类返回 None。
    pub fn best_match(&self, vec: &[f32]) -> Option<(i64, f32)> {
        self.sums
            .iter()
            .map(|(id, (sum, _))| {
                let centroid = normalize(sum.clone());
                let cos: f32 = centroid.iter().zip(vec).map(|(a, b)| a * b).sum();
                (*id, cos)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
    }

    /// 簇心滑动平均吸收新成员（sum += vec, count += 1）。
    pub fn absorb(&mut self, cluster_id: i64, vec: &[f32]) {
        let entry = self
            .sums
            .entry(cluster_id)
            .or_insert_with(|| (vec![0f32; vec.len()], 0));
        for (slot, v) in entry.0.iter_mut().zip(vec) {
            *slot += v;
        }
        entry.1 += 1;
    }

    /// 归簇判定：最佳簇 cos ≥ 阈值 → 该簇；否则 None（调用方开新簇）。
    pub fn assign(&self, vec: &[f32]) -> Option<i64> {
        match self.best_match(vec) {
            Some((id, cos)) if cos >= self.threshold => Some(id),
            _ => None,
        }
    }

    /// 簇数（测试/观测）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn cluster_count(&self) -> usize {
        self.sums.len()
    }
}

// ---------------------------------------------------------------------------
// 簇心缓存（进程内；dbDir → 共享）
// ---------------------------------------------------------------------------

/// 全局簇心缓存池：dbDir → 聚类器（face 通道单 worker，Mutex 足够）。
fn cache_pool() -> &'static Mutex<HashMap<PathBuf, std::sync::Arc<Mutex<OnlineClusterer>>>> {
    static POOL: std::sync::OnceLock<
        Mutex<HashMap<PathBuf, std::sync::Arc<Mutex<OnlineClusterer>>>>,
    > = std::sync::OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 规范化库根（池键；canonicalize 失败退回原样）。
fn pool_key(db_dir: &Path) -> PathBuf {
    std::fs::canonicalize(db_dir).unwrap_or_else(|_| db_dir.to_path_buf())
}

/// 取（或惰性重建）某库的聚类器缓存。
fn cluster_cache(
    db: &Db,
    db_dir: &Path,
    threshold: f32,
) -> Result<std::sync::Arc<Mutex<OnlineClusterer>>, String> {
    let key = pool_key(db_dir);
    let mut pool = cache_pool().lock().expect("cluster pool mutex poisoned");
    if let Some(hit) = pool.get(&key) {
        return Ok(std::sync::Arc::clone(hit));
    }
    let sums = db
        .face_cluster_sums()
        .map_err(|e| format!("读取簇心失败: {e}"))?;
    let clusterer = std::sync::Arc::new(Mutex::new(OnlineClusterer::from_sums(sums, threshold)));
    pool.insert(key, std::sync::Arc::clone(&clusterer));
    Ok(clusterer)
}

/// 簇心缓存失效（ai_face_data_clear 后调用，下次按空聚类重建）。
pub fn invalidate_cluster_cache(db_dir: &Path) {
    cache_pool()
        .lock()
        .expect("cluster pool mutex poisoned")
        .remove(&pool_key(db_dir));
}

/// 在线归簇：命中阈值 → 簇 id；否则建新簇（people 落行 + 缓存吸收）。
fn assign_cluster(db: &Db, clusterer: &mut OnlineClusterer, emb: &[f32]) -> Result<i64, String> {
    if let Some(id) = clusterer.assign(emb) {
        return Ok(id);
    }
    let id = db.create_person().map_err(|e| e.to_string())?;
    Ok(id)
}

// ---------------------------------------------------------------------------
// index_tasks kind="face" 通道（CPU 串行；接线见 ipc 钩子 + 启动恢复）
// ---------------------------------------------------------------------------

/// 单条 face 任务处理：2048 档缩略图（256 档人脸像素不足）→ 检测 → 对齐 →
/// 特征 → 在线归簇 → faces 落行 → 记账。返回成功与否（失败走 attempts 封顶）。
fn process_face_task(db: &Db, db_dir: &Path, manager: &ModelManager, asset_id: i64) -> bool {
    let Some((path, _)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return false; // 资产已删除（级联清任务前的防御兜底）
    };
    let Some(thumb) = crate::thumbs::thumb_file(db_dir, Path::new(&path), 2048) else {
        return false;
    };
    let img = image::ImageReader::open(&thumb)
        .ok()
        .and_then(|r| r.decode().ok())
        .map(|d| d.to_rgb8());
    let Some(img) = img else { return false };
    let faces = match manager.detect_and_embed(&img) {
        Ok(f) => f,
        Err(_) => return false,
    };
    // 聚类阈值（settings.ai.face_cluster_threshold，默认 CLUSTER_COS_THRESHOLD）
    let cache = match cluster_cache(db, db_dir, manager.ai_params().face_cluster_threshold) {
        Ok(c) => c,
        Err(_) => return false,
    };
    let mut clusterer = cache.lock().expect("clusterer mutex poisoned");
    for (face, emb) in faces {
        let cluster_id = match assign_cluster(db, &mut clusterer, &emb) {
            Ok(id) => id,
            Err(_) => return false,
        };
        clusterer.absorb(cluster_id, &emb);
        let face_id = match db.insert_face(
            asset_id,
            face.box_x as f64,
            face.box_y as f64,
            face.box_w as f64,
            face.box_h as f64,
            &emb,
            Some(cluster_id),
        ) {
            Ok(id) => id,
            Err(_) => return false,
        };
        // 封面 = 簇内最大框人脸（首张无条件担任）
        let _ = db.maybe_promote_cover(face_id, cluster_id, (face.box_w * face.box_h) as f64);
    }
    db.set_face_indexed(asset_id).is_ok()
}

/// 人脸回填 + 串行执行（模型齐备 && enable_face 时由调用方触发）：
/// ① 为 `face_indexed_at IS NULL` 的照片建任务 ② 单 worker 跑 face 通道，
/// 逐条发布 indexTaskProgress{kind:"face"}。返回本轮成功数。
pub fn run_face_backfill(
    db_dir: &Path,
    manager: std::sync::Arc<ModelManager>,
    bus: &EventBus,
) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        eprintln!("人脸回填：库打开失败（{}），本轮跳过", db_dir.display());
        return 0;
    };
    // 与语义回填同款修复：派 worker 看存量 pending，不看本轮新建数
    // （任务已存在时新建数为 0，此前直接空转返回——点击立即索引假成功）。
    if let Err(e) = db.create_face_tasks_for_unindexed() {
        eprintln!("人脸回填：补种任务失败: {e}");
    }
    let total = db.pending_index_task_count("face").unwrap_or(0);
    if total == 0 {
        return 0;
    }
    let mut done = 0u64;
    // 单 worker：在线聚类是顺序敏感状态，且 ONNX 会话本就互斥串行。
    loop {
        let task = match db.claim_index_task("face") {
            Ok(Some(t)) => t,
            Ok(None) => break,
            Err(e) => {
                eprintln!("人脸回填：认领任务失败，worker 退出: {e}");
                break;
            }
        };
        let ok = process_face_task(&db, db_dir, &manager, task.asset_id);
        let _ = db.finish_index_task(task.id, ok);
        done += u64::from(ok);
        bus.publish(AppEvent::IndexTaskProgress {
            kind: "face".into(),
            done,
            total,
        });
    }
    done
}

/// 便利入口：导入钩子 / 启动恢复 / 模型装好后的统一触发。门槛：scrfd +
/// arcface 齐备（未齐静默跳过——下载完成钩子会再触发）；enable_face 由
/// 调用方把关（与 enable_clip 同款约定）。
pub fn kick_face_if_ready(
    db_dir: PathBuf,
    manager: &ModelManager,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    if !manager.face_models_ready() {
        return;
    }
    let manager = std::sync::Arc::new(manager.clone());
    let bus = bus.clone();
    let _ = supervisor.spawn_unique("index", "face-backfill".into(), move |_| {
        run_face_backfill(&db_dir, manager, &bus);
    });
}
