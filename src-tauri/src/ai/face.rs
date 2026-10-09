//! 人脸全链路（M4）：SCRFD 343G 检测 + 5 关键点对齐 + ArcFace 512 维特征 +
//! 在线聚类（index_tasks kind="face" 通道）。
//!
//! ## 性能结构（2026-09-28 三件套：检测源降档 + 两阶段并行 + 毒化隔离）
//! - **检测源降档**：检测/对齐/特征全在缓存档缩略图上做（SCRFD 输入恒
//!   640 letterbox，源 ≥512 即可，1024→640 与 2048→640 检测质量等价），
//!   优先命中 [`DETECTION_SOURCE_TIERS`] 的已缓存档，全未命中才生成最便宜
//!   档 512——此前无条件 `thumb_file(2048)` 同步显影 61MP RAW（首张半秒~
//!   两秒）是流水线最大固定成本。
//! - **faces 表坐标空间 v2 = 归一化 0..1**（box 两轴各自除以源图宽高；
//!   v1 是 2048 档像素坐标）。全库消费方已同步（selection.rs blur 人脸
//!   局部裁剪按归一化映射；db 封面面积的 w*h 比较在同一空间内自洽）。
//!   **旧像素坐标数据不兼容**——重建入口 `index_rebuild(face)`（开发期
//!   无兼容包袱，497 张重建很快）。
//! - **两阶段并行**：[`run_face_backfill`] 阶段 1 多 worker（FACE_WORKERS）
//!   认领→取图→推理（SCRFD/ArcFace 会话互斥天然串行化 GPU），阶段 2
//!   单线程消费 mpsc 结果做在线聚类（顺序敏感单点）+ 落库。
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
//! ## 保守的人物归类
//! 先排除几何异常和接近空白图的无身份特征，再比较簇心与固定参考脸。
//! 最佳候选与次佳候选难以区分、或参考脸不支持时保留为未归类人脸；
//! 同一张照片的不同人脸不能进入同一人物，防止错误簇持续吸收成员。
//! 聚类为顺序敏感的单遍状态 → 回填阶段 2 单线程消费（见 run_face_backfill）；
//! 簇心缓存在进程内（重启后从 faces 表重建）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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
    /// det 会话对应的模型 id（档位切换后换件重建：fast=scrfd-10g，
    /// normal/accurate=scrfd；不匹配即弃缓存会话重载）。
    det_model: Option<String>,
    rec: Option<Session>,
}

fn face_slots() -> &'static Mutex<FaceSlots> {
    static SLOTS: std::sync::OnceLock<Mutex<FaceSlots>> = std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(FaceSlots::default()))
}

/// 空闲卸载（ai::idle::release_all 调用；取锁置空，在途推理不受影响）。
pub fn release() {
    *face_slots().lock().expect("face slots mutex poisoned") = FaceSlots::default();
}

impl ModelManager {
    /// 人脸两件套是否齐备（检测件按当前画质档位解析：fast = scrfd-10g +
    /// arcface；normal/accurate = scrfd + arcface）。
    pub fn face_models_ready(&self) -> bool {
        let det = super::face_detect_model_id(self.ai_params().quality_tier);
        [det, "arcface"]
            .iter()
            .all(|id| self.model_path(id).is_file())
    }

    /// 闭眼检测依赖：当前档 SCRFD、FaceMesh 定位、专用睁闭眼候选分类器。
    /// 未安装 → eyes 通道跳过并计数，任务保持 pending（下载完成后
    /// eyes-postinstall watch 自然续跑）。
    pub fn selection_eyes_ready(&self) -> bool {
        [
            "facemesh",
            super::selection_regions::EYE_MODEL,
            super::face_detect_model_id(self.ai_params().quality_tier),
        ]
        .iter()
        .all(|id| self.model_path(id).is_file())
    }

    /// 惰性加载检测会话（模型按当前档位解析；切档后 det_model 不匹配即
    /// 弃旧会话重载新件）。
    fn ensure_det(&self, slots: &mut FaceSlots) -> Result<(), String> {
        super::idle::touch();
        let model_id = super::face_detect_model_id(self.ai_params().quality_tier);
        if slots.det.is_some() && slots.det_model.as_deref() == Some(model_id) {
            return Ok(());
        }
        let path = self.model_path(model_id);
        if !path.is_file() {
            return Err(format!("模型 {model_id} 未下载（设置页下载后再试）"));
        }
        // EP 序列与 embed 共用（use_gpu → [DML, CPU]；DML 失败自动落 CPU）；
        // 毒化位按模型 id 隔离（scrfd 与 scrfd-10g 互不连坐）
        let session = super::build_session(&path, None, self.ai_params().use_gpu, model_id)?;
        slots.det = Some(session);
        slots.det_model = Some(model_id.to_string());
        Ok(())
    }

    fn ensure_rec(&self, slots: &mut FaceSlots) -> Result<(), String> {
        super::idle::touch();
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
    // DML 运行时故障同 embed 语义：毒化（按当前档位检测模型 id 隔离）+
    // 纯 CPU 重建重跑一次
    let use_gpu = manager.ai_params().use_gpu;
    let det_model = super::face_detect_model_id(manager.ai_params().quality_tier);
    let heads = super::run_with_acceleration_fallback(
        use_gpu,
        det_model,
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

/// Selection-only tiled detection. Overlapping tiles retain small faces at the
/// detector's native resolution; NMS merges duplicates in source coordinates.
pub fn detect_selection_faces(
    manager: &ModelManager,
    img: &RgbImage,
) -> Result<Vec<DetectedFace>, String> {
    let mut faces = detect_faces(manager, img)?;
    let tile = 1024u32;
    if img.width().max(img.height()) > tile {
        let starts = |length: u32| {
            let mut result = vec![0];
            while *result.last().unwrap() + tile < length {
                let next = (*result.last().unwrap() + 768).min(length.saturating_sub(tile));
                if result.last() == Some(&next) {
                    break;
                }
                result.push(next);
            }
            result
        };
        for y in starts(img.height()) {
            for x in starts(img.width()) {
                let crop = image::imageops::crop_imm(
                    img,
                    x,
                    y,
                    tile.min(img.width() - x),
                    tile.min(img.height() - y),
                )
                .to_image();
                for mut face in detect_faces(manager, &crop)? {
                    // A cropped face at an internal tile boundary is not evidence
                    // of a second person. Its full representation belongs to the overlap.
                    if (x > 0 && face.box_x < 8.0)
                        || (y > 0 && face.box_y < 8.0)
                        || (x + crop.width() < img.width()
                            && face.box_x + face.box_w > crop.width() as f32 - 8.0)
                        || (y + crop.height() < img.height()
                            && face.box_y + face.box_h > crop.height() as f32 - 8.0)
                    {
                        continue;
                    }
                    face.box_x += x as f32;
                    face.box_y += y as f32;
                    for point in &mut face.kps {
                        point[0] += x as f32;
                        point[1] += y as f32;
                    }
                    faces.push(face);
                }
            }
        }
    }
    faces.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut faces = nms(faces, NMS_IOU);
    faces.truncate(MAX_FACES_PER_IMAGE);
    Ok(faces)
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
    warp_similarity_sized(img, m, ALIGNED_SIZE)
}

/// [`warp_similarity`] 的尺寸推广（eyes 通道 facemesh 256² 裁剪复用，
/// 2026-09-28）：按 2×3 相似矩阵逆向映射 + 双线性采样，输出 `size`² 裁剪。
pub fn warp_similarity_sized(img: &RgbImage, m: &[[f32; 3]; 2], size: u32) -> RgbImage {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let mut out = RgbImage::new(size, size);
    if det.abs() < f32::EPSILON {
        return out;
    }
    let inv = [
        [m[1][1] / det, -m[0][1] / det],
        [-m[1][0] / det, m[0][0] / det],
    ];
    for y in 0..size {
        for x in 0..size {
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
        // DML 运行时故障同 embed/det 语义：毒化（仅 arcface，按模型隔离）
        // + 纯 CPU 重建重跑一次
        let use_gpu = self.ai_params().use_gpu;
        let vec = super::run_with_acceleration_fallback(
            use_gpu,
            "arcface",
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
            // 能检测到小人脸不等于能可靠识别身份。几像素的远景框经放大
            // 后特征趋同，容易把完全不同的照片塞进同一个人物。
            if !usable_face(&face, self.ai_params().face_detect_threshold) {
                continue;
            }
            let Some(crop) = align_face(img, &face.kps) else {
                continue;
            };
            if !usable_crop(&crop) {
                continue;
            }
            let emb = self.embed_aligned(&crop)?;
            if !identity_has_signal(&emb, &nonface_references(self)?) {
                continue;
            }
            out.push((face, emb));
        }
        Ok(out)
    }
}

/// 识别质量规则代际：升级后只重建人脸索引，清除旧的低质量归类。
pub const FACE_INDEX_GENERATION: u64 = 3;
const MIN_FACE_PIXELS: f32 = 24.0;

pub fn usable_face(face: &DetectedFace, threshold: f32) -> bool {
    let eye_span = ((face.kps[0][0] - face.kps[1][0]).powi(2)
        + (face.kps[0][1] - face.kps[1][1]).powi(2))
    .sqrt();
    face.score.is_finite()
        && face.score >= threshold
        && [face.box_x, face.box_y, face.box_w, face.box_h]
            .iter()
            .all(|v| v.is_finite())
        && face.box_w >= MIN_FACE_PIXELS
        && face.box_h >= MIN_FACE_PIXELS
        && face.kps.iter().flatten().all(|v| v.is_finite())
        && eye_span >= 8.0
        && eye_span >= face.box_w * 0.15
        && eye_span <= face.box_w * 0.85
        && face.kps.iter().all(|p| {
            p[0] >= face.box_x - face.box_w * 0.15
                && p[0] <= face.box_x + face.box_w * 1.15
                && p[1] >= face.box_y - face.box_h * 0.15
                && p[1] <= face.box_y + face.box_h * 1.15
        })
        && (face.kps[0][1] - face.kps[1][1]).abs() <= eye_span * 0.5
        && face.kps[2][1] > (face.kps[0][1] + face.kps[1][1]) * 0.5
        && (face.kps[3][1] + face.kps[4][1]) * 0.5 > face.kps[2][1]
}

/// ArcFace 对空白/无信息裁片也会输出非零向量，不能只检查向量范数。
/// 用同一个识别模型生成负样本参考：与无信息输入相似的特征直接拒绝归类。
fn nonface_references(manager: &ModelManager) -> Result<Vec<Vec<f32>>, String> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<PathBuf, Vec<Vec<f32>>>>> =
        std::sync::OnceLock::new();
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .expect("nonface reference cache");
    let key = manager.model_path("arcface");
    if let Some(references) = cache.get(&key) {
        return Ok(references.clone());
    }
    let references: Vec<Vec<f32>> = [0, 128, 255]
        .into_iter()
        .map(|value| {
            manager.embed_aligned(&RgbImage::from_pixel(
                ALIGNED_SIZE,
                ALIGNED_SIZE,
                image::Rgb([value; 3]),
            ))
        })
        .collect::<Result<_, _>>()?;
    if references.iter().any(|v| !usable_embedding(v)) {
        return Err("无效的人脸识别参考特征".into());
    }
    cache.insert(key, references.clone());
    Ok(references)
}

pub fn identity_has_signal(embedding: &[f32], nonfaces: &[Vec<f32>]) -> bool {
    usable_embedding(embedding)
        && !nonfaces.is_empty()
        && nonfaces
            .iter()
            .all(|reference| usable_embedding(reference) && cosine(embedding, reference) < 0.8)
}

fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn usable_crop(crop: &RgbImage) -> bool {
    let mut sum = 0.0f64;
    let mut squares = 0.0f64;
    for pixel in crop.pixels() {
        let value = (f64::from(pixel[0]) + f64::from(pixel[1]) + f64::from(pixel[2])) / 3.0;
        sum += value;
        squares += value * value;
    }
    let n = f64::from(crop.width()) * f64::from(crop.height());
    n > 0.0 && squares / n - (sum / n).powi(2) >= 4.0
}

fn usable_embedding(embedding: &[f32]) -> bool {
    embedding.len() == FACE_EMBED_DIM
        && embedding.iter().all(|v| v.is_finite())
        && embedding.iter().map(|v| v * v).sum::<f32>() > f32::EPSILON
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

/// 在线聚类器：移动簇心匹配须同时获得固定参考脸支持。
/// 不确定匹配保留未归类，不自动创建兜底人物或吸收进已有簇。
#[derive(Debug, Default)]
pub struct OnlineClusterer {
    sums: HashMap<i64, (Vec<f32>, u32)>,
    /// 稳定参考脸，最多三张；避免仅靠移动簇心产生连续误吸收。
    anchors: HashMap<i64, Vec<Vec<f32>>>,
    threshold: f32,
}

#[derive(Debug, PartialEq)]
pub enum IdentityMatch {
    Certain(i64),
    Ambiguous,
    Novel,
}

impl OnlineClusterer {
    /// 空聚类器（测试构造入口；生产路径走 from_sums）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（lib 目标内无调用点）
    pub fn new(threshold: f32) -> Self {
        Self {
            sums: HashMap::new(),
            anchors: HashMap::new(),
            threshold,
        }
    }

    /// 从持久化簇和恢复（重启后首脸重建缓存）。
    pub fn from_sums(sums: HashMap<i64, (Vec<f32>, u32)>, threshold: f32) -> Self {
        let anchors = sums
            .iter()
            .map(|(id, (sum, _))| (*id, vec![normalize(sum.clone())]))
            .collect();
        Self {
            sums,
            anchors,
            threshold,
        }
    }

    /// 最佳匹配簇：(cluster_id, cos)。空聚类返回 None。
    #[allow(dead_code)] // 集成测试引用
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
        let anchors = self.anchors.entry(cluster_id).or_default();
        if anchors.len() < 3 {
            anchors.push(vec.to_vec());
        }
        let entry = self
            .sums
            .entry(cluster_id)
            .or_insert_with(|| (vec![0f32; vec.len()], 0));
        for (slot, v) in entry.0.iter_mut().zip(vec) {
            *slot += v;
        }
        entry.1 += 1;
    }

    /// 测试兼容入口；生产用 decide 区分新人物与不确定匹配。
    #[allow(dead_code)] // 集成测试引用
    pub fn assign(&self, vec: &[f32]) -> Option<i64> {
        match self.decide(vec, &HashSet::new()) {
            IdentityMatch::Certain(id) => Some(id),
            _ => None,
        }
    }

    pub fn decide(&self, vec: &[f32], excluded: &HashSet<i64>) -> IdentityMatch {
        if !usable_embedding(vec) {
            return IdentityMatch::Ambiguous;
        }
        let mut candidates: Vec<_> = self
            .sums
            .iter()
            .filter(|(id, _)| !excluded.contains(id))
            .map(|(id, (sum, _))| (*id, cosine(&normalize(sum.clone()), vec)))
            .collect();
        candidates.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let Some(&(id, score)) = candidates.first() else {
            return IdentityMatch::Novel;
        };
        if score < self.threshold {
            return IdentityMatch::Novel;
        }
        if candidates
            .get(1)
            .is_some_and(|(_, runner_up)| score - runner_up < 0.08)
        {
            return IdentityMatch::Ambiguous;
        }
        let Some(anchors) = self.anchors.get(&id) else {
            return IdentityMatch::Ambiguous;
        };
        let agreeing = anchors
            .iter()
            .filter(|anchor| cosine(anchor, vec) >= self.threshold)
            .count();
        if agreeing * 2 <= anchors.len() {
            return IdentityMatch::Ambiguous;
        }
        IdentityMatch::Certain(id)
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
    let mut clusterer = OnlineClusterer::from_sums(sums, threshold);
    clusterer.anchors = db
        .face_cluster_anchors()
        .map_err(|e| format!("读取参考人脸失败: {e}"))?;
    let clusterer = std::sync::Arc::new(Mutex::new(clusterer));
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
fn assign_cluster(
    db: &Db,
    clusterer: &mut OnlineClusterer,
    emb: &[f32],
    excluded: &HashSet<i64>,
) -> Result<Option<i64>, String> {
    match clusterer.decide(emb, excluded) {
        IdentityMatch::Certain(id) => Ok(Some(id)),
        IdentityMatch::Ambiguous => Ok(None),
        IdentityMatch::Novel => db.create_person().map(Some).map_err(|e| e.to_string()),
    }
}

// ---------------------------------------------------------------------------
// index_tasks kind="face" 通道（两阶段：多 worker 推理 + 单线程聚类落库）
// ---------------------------------------------------------------------------

/// 检测源档位优先级（缓存命中优先；1024 不是 thumbs 档位——SIZE_TIERS =
/// 256/512/2048 → 用 [512, 2048]）。SCRFD 输入恒 640 letterbox，源 ≥512
/// 即可（1024→640 与 2048→640 检测质量等价）；检测+对齐+ArcFace 全在
/// 同一源图上做。**三档统一本策略**（2026-09-28 用户定规：精准档不再
/// 2048 优先——检测恒 640、2048 源只贵在 ArcFace 裁片边际质量，整库
/// 同步生成 2048 是精准档索引慢且 GPU 占用低的主因，得不偿失）。
pub const DETECTION_SOURCE_TIERS: &[u16] = &[512, 2048];

/// 检测源取图（三档统一策略）：按 [`DETECTION_SOURCE_TIERS`] 顺序查
/// **缓存命中**（只查不生成，thumbs::cached），全未命中才 `thumb_file(512)`
/// 按需生成最便宜档。吃掉「无条件同步生成 2048 档（61MP ARW 首张半秒~
/// 两秒）」这一最大固定成本。blur/eyes 选片通道不分档，沿用本函数。
/// 精准档的差异只在语义 fp16 双塔（见 [`super::semantic_model_ids`]）。
pub fn detection_source(db_dir: &Path, src: &Path) -> Option<String> {
    for tier in DETECTION_SOURCE_TIERS {
        if let Some(hit) = crate::thumbs::cached(db_dir, src, *tier) {
            return Some(hit);
        }
    }
    crate::thumbs::thumb_file(db_dir, src, DETECTION_SOURCE_TIERS[0])
}

/// 检出人脸（源图像素坐标）→ faces 表归一化坐标 `[x, y, w, h]` ∈ 0..1
/// （两轴各自除以源图宽高；坐标空间 v2，见模块注释）。
/// SCRFD anchor 解码在图缘可少量越界（贴边半脸真机实测 x+w 至 1.014），
/// 按 0..1 契约写入时钳制回图内——检测语义不变（v1 像素空间同款越界，
/// 只是当时无契约约束）。
pub fn normalized_box(face: &DetectedFace, src_w: u32, src_h: u32) -> [f64; 4] {
    let (w, h) = (f64::from(src_w.max(1)), f64::from(src_h.max(1)));
    let x = (f64::from(face.box_x) / w).clamp(0.0, 1.0);
    let y = (f64::from(face.box_y) / h).clamp(0.0, 1.0);
    let bw = (f64::from(face.box_w) / w).clamp(0.0, 1.0 - x);
    let bh = (f64::from(face.box_h) / h).clamp(0.0, 1.0 - y);
    [x, y, bw, bh]
}

/// 阶段 1 产物：源图尺寸 + 检出人脸（源图像素坐标）+ ArcFace 归一化特征。
pub struct FacePayload {
    pub src_w: u32,
    pub src_h: u32,
    pub faces: Vec<(DetectedFace, Vec<f32>)>,
}

/// worker → 消费者消息：payload=None 表示该任务失败（资产缺失/取图失败/
/// 推理失败/panic——统一交阶段 2 finish_index_task(false) 回收）。
pub struct FaceTaskOutcome {
    pub task_id: i64,
    pub asset_id: i64,
    pub payload: Option<FacePayload>,
    _permit: crate::tasks::index_budget::Permit,
}

/// 阶段 1 推理核（可注入：生产 = [`infer_face_payload`]；集成测试 = 假嵌入
/// 桩 / panic 桩，验证两阶段顺序性与 panic 回收）。
pub type FaceInfer = Arc<dyn Fn(&Db, i64) -> Option<FacePayload> + Send + Sync>;

/// 阶段 1 单任务核：新档位策略取图 → 解码 → 检测+对齐+特征。任一步失败
/// 返回 None（任务按失败结算走 attempts 封顶）。
fn infer_face_payload(
    db: &Db,
    db_dir: &Path,
    manager: &ModelManager,
    asset_id: i64,
) -> Option<FacePayload> {
    let (path, _) = db.thumb_info_by_id(asset_id).ok().flatten()?; // 资产已删除（级联清任务前的防御兜底）
    let thumb = detection_source(db_dir, Path::new(&path))?;
    let img = image::ImageReader::open(&thumb)
        .ok()
        .and_then(|r| r.decode().ok())
        .map(|d| d.to_rgb8())?;
    let faces = manager.detect_and_embed(&img).ok()?;
    Some(FacePayload {
        src_w: img.width(),
        src_h: img.height(),
        faces,
    })
}

/// 阶段 2 单任务核（**仅消费线程调用**，聚类顺序敏感）：在线归簇 →
/// faces 落行（**归一化坐标**）→ 封面晋升 → face_indexed_at 记账。
/// 写库失败返回 false（任务重试；半途已插的脸行随 index_rebuild(face)
/// 幂等清理，与旧单 worker 语义一致）。
fn commit_face_payload(
    db: &Db,
    db_dir: &Path,
    manager: &ModelManager,
    asset_id: i64,
    payload: &FacePayload,
) -> bool {
    // 聚类阈值（settings.ai.face_cluster_threshold，默认 CLUSTER_COS_THRESHOLD）
    let Ok(cache) = cluster_cache(db, db_dir, manager.ai_params().face_cluster_threshold) else {
        return false;
    };
    let mut clusterer = cache.lock().expect("clusterer mutex poisoned");
    // 同一张合影的不同人脸不能被自动归为同一个人物。
    let mut assigned = HashSet::new();
    for (face, emb) in &payload.faces {
        // 消费边界再校验；无有效人脸也正常完成索引，但不创建人物。
        if !face.score.is_finite()
            || face.score < manager.ai_params().face_detect_threshold
            || ![face.box_x, face.box_y, face.box_w, face.box_h]
                .iter()
                .all(|v| v.is_finite())
            || face.box_w < MIN_FACE_PIXELS
            || face.box_h < MIN_FACE_PIXELS
            || !usable_embedding(emb)
        {
            continue;
        }
        let [x, y, w, h] = normalized_box(face, payload.src_w, payload.src_h);
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let Ok(cluster_id) = assign_cluster(db, &mut clusterer, emb, &assigned) else {
            return false;
        };
        let Ok(face_id) = db.insert_face(asset_id, x, y, w, h, emb, cluster_id) else {
            return false;
        };
        // 封面 = 簇内最大框人脸（归一化面积，同一空间内自洽；首张无条件担任）
        if let Some(cluster_id) = cluster_id {
            clusterer.absorb(cluster_id, emb);
            assigned.insert(cluster_id);
            let _ = db.maybe_promote_cover(face_id, cluster_id, w * h);
        }
    }
    db.set_face_indexed(asset_id).is_ok()
}

/// 在途门闩（认领中 + 已认领未结算的计数）：失败任务由消费者
/// finish_index_task(false) 回 pending 后**必须有人再认领**——worker 认领
/// 空时若仍有在途（别人的认领/未结算结果），等结算后重试认领再退出，
/// 否则失败任务会滞留 pending 到下一轮 kick（两阶段竞态，测试踩出）。
#[derive(Default)]
struct InFlightGate {
    state: Mutex<u64>,
    cv: std::sync::Condvar,
}

impl InFlightGate {
    /// 登记「一次认领尝试开始」。
    fn begin(&self) {
        *self.state.lock().expect("face inflight mutex poisoned") += 1;
    }

    /// 认领落空后的结算。返回 true = 可退出：撤销自己登记的瞬间计数已
    /// 归零——此刻无在途认领/未结算结果 ⇒ 不可能再有 finish 把任务回
    /// pending ⇒ 落空的认领是终局。返回 false = 等待过（期间有在途
    /// 结算，失败任务可能已回 pending）⇒ 调用方必须**重新认领**。
    /// （归零必 notify：等待者靠它醒来重认领/退出。）
    fn settle_or_wait_exit(&self) -> bool {
        let mut left = self.state.lock().expect("face inflight mutex poisoned");
        *left = left.saturating_sub(1);
        if *left == 0 {
            drop(left);
            self.cv.notify_all();
            return true;
        }
        while *left > 0 {
            left = self.cv.wait(left).expect("face inflight mutex poisoned");
        }
        false
    }

    /// 消费者结算一条结果：计数 -1 + 唤醒等待的 worker。
    fn settle_one(&self) {
        let mut left = self.state.lock().expect("face inflight mutex poisoned");
        *left = left.saturating_sub(1);
        drop(left);
        self.cv.notify_all();
    }
}

/// 阶段 1 worker 主循环：认领 → 推理（catch_unwind：panic 归一为该任务
/// 失败，不裸 unwrap、不弃任务——由阶段 2 finish_index_task(false) 沿用
/// attempts 封顶回收；worker 线程绝不因单任务崩溃退出）→ 送消费者。
fn face_stage1_loop(
    db: &Db,
    infer: &FaceInfer,
    tx: std::sync::mpsc::Sender<FaceTaskOutcome>,
    gate: &InFlightGate,
) {
    loop {
        let permit =
            crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Face).acquire(1);
        gate.begin();
        let task = match db.claim_index_task("face") {
            Ok(Some(t)) => t,
            Ok(None) => {
                drop(permit);
                // 无可认领：无在途 ⇒ 退出安全；等过 ⇒ 失败任务可能已回
                // pending，必须重新认领（防两阶段竞态下任务滞留）。
                if gate.settle_or_wait_exit() {
                    break;
                }
                continue;
            }
            Err(e) => {
                eprintln!("人脸回填：认领任务失败，worker 退出: {e}");
                gate.settle_one();
                break;
            }
        };
        let payload = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            infer(db, task.asset_id)
        })) {
            Ok(p) => p,
            Err(_) => {
                eprintln!(
                    "人脸回填：任务 asset_id={} 处理 panic，按失败回收",
                    task.asset_id
                );
                None
            }
        };
        if tx
            .send(FaceTaskOutcome {
                task_id: task.id,
                asset_id: task.asset_id,
                payload,
                _permit: permit,
            })
            .is_err()
        {
            // 消费者已退出：结算自己的在途登记，别让等待的 worker 挂死
            gate.settle_one();
            break;
        }
    }
}

/// 阶段 2 消费循环（**单线程**，mpsc 顺序消费）：聚类落库 → 任务收尾 →
/// 进度事件（done 严格非降）→ 释放该结果在途登记。返回本轮成功数。
fn face_stage2_consume(
    db: &Db,
    db_dir: &Path,
    manager: &ModelManager,
    bus: &EventBus,
    total: u64,
    rx: std::sync::mpsc::Receiver<FaceTaskOutcome>,
    gate: &InFlightGate,
) -> u64 {
    let mut done = 0u64;
    for outcome in rx {
        let ok = match &outcome.payload {
            Some(payload) => commit_face_payload(db, db_dir, manager, outcome.asset_id, payload),
            None => false,
        };
        let _ = db.finish_index_task(outcome.task_id, ok);
        done += u64::from(ok);
        bus.publish(AppEvent::IndexTaskProgress {
            kind: "face".into(),
            done,
            total,
        });
        gate.settle_one();
    }
    done
}

/// Face inference uses the dynamic CPU/2 budget; clustering stays single-threaded.
/// 人脸回填两阶段执行核（推理核可注入，见 [`FaceInfer`]；生产入口 =
/// [`run_face_backfill`]）。① 为 `face_indexed_at IS NULL` 的照片建任务
/// （派 worker 看存量 pending，不看本轮新建数——任务已存在时新建数为 0，
/// 直接空转会假成功）。② 阶段 1 CPU/2 个 worker 认领→取图（新
/// 档位策略）→解码→letterbox→SCRFD→对齐→ArcFace（会话互斥天然把 GPU
/// 推理串行化，会话槽结构不动）；阶段 2 单线程消费 mpsc → 在线聚类
/// （顺序敏感单点）→ assign/absorb → insert_face（归一化坐标）→
/// maybe_promote_cover → set_face_indexed → finish_index_task →
/// indexTaskProgress{kind:"face"}。返回本轮成功数。
pub fn run_face_backfill_with(
    db_dir: &Path,
    manager: Arc<ModelManager>,
    bus: &EventBus,
    infer: FaceInfer,
) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        eprintln!("人脸回填：库打开失败（{}），本轮跳过", db_dir.display());
        return 0;
    };
    if let Err(e) = db.create_face_tasks_for_unindexed() {
        eprintln!("人脸回填：补种任务失败: {e}");
    }
    let total = db.pending_index_task_count("face").unwrap_or(0);
    if total == 0 {
        return 0;
    }
    let (tx, rx) = std::sync::mpsc::channel::<FaceTaskOutcome>();
    let gate = Arc::new(InFlightGate::default());
    let mut handles = Vec::new();
    for n in 0..crate::tasks::index_parallelism() {
        let db_dir = db_dir.to_path_buf();
        let infer = Arc::clone(&infer);
        let tx = tx.clone();
        let gate = Arc::clone(&gate);
        match std::thread::Builder::new()
            .name(format!("index-face-{n}"))
            .spawn(move || {
                // 每 worker 独立连接（WAL + busy_timeout 5s，与语义回填同款）
                let Ok(worker_db) = crate::ipc::open_library_db(&db_dir) else {
                    return;
                };
                face_stage1_loop(&worker_db, &infer, tx, &gate);
            }) {
            Ok(h) => handles.push(h),
            Err(e) => eprintln!("人脸回填：worker{n} 派生失败: {e}"),
        }
    }
    drop(tx); // 全部 worker 退出（sender 清空）后消费循环自然收敛
    let done = face_stage2_consume(&db, db_dir, &manager, bus, total, rx, &gate);
    for h in handles {
        let _ = h.join(); // 阶段 1 循环已由「认领空且在途归零」收敛
    }
    done
}

/// 人脸回填（生产入口，真推理核）。空聚类重建/进度/启停语义与两阶段
/// 重构前一致（kick_face_if_ready 签名不动）。
pub fn run_face_backfill(db_dir: &Path, manager: Arc<ModelManager>, bus: &EventBus) -> u64 {
    let infer_db_dir = db_dir.to_path_buf();
    let infer_manager = Arc::clone(&manager);
    let infer: FaceInfer = Arc::new(move |db, asset_id| {
        infer_face_payload(db, &infer_db_dir, &infer_manager, asset_id)
    });
    run_face_backfill_with(db_dir, manager, bus, infer)
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
    let _ = supervisor.spawn_coalesced(
        "index",
        format!("face-backfill:{}", db_dir.display()),
        move |_| {
            run_face_backfill(&db_dir, Arc::clone(&manager), &bus);
        },
    );
}
