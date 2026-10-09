//! Fast selection evidence: oriented 1024px cached thumbnails, one SCRFD pass,
//! per-eye candidate analysis. Insufficient pixels remain unknown. High-resolution
//! source/tile adapters are retained only for explicit offline evaluation.
//! Analysis never edits photos, ratings, decisions, or XMP.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::db::Db;
// ---------------------------------------------------------------------------
// 失焦（blur）：拉普拉斯清晰度分
// ---------------------------------------------------------------------------

/// 归一饱和常数 K：score = 100·var/(var+K)。K=50 时 var=200（清晰）→ 80，
/// var=20（糊）→ 29，量程内区分度好且天然封顶 100。
pub const BLUR_LAPLACE_K: f64 = 50.0;
/// 算法版本标签（落 ai_analysis.model_version；随指纹入重建判定）。
pub const BLUR_ALGO_VERSION: &str = "regional-focus-1024-v8-heuristic";

/// blur 软阈值运行时快照（×1000 整数原子存；worker 线程无 settings 访问，
/// 启动 / settings_set 时刷新——同 thumbs::set_thumb_cache_cap_bytes 模式）。
static BLUR_SOFT_THRESHOLD_MILLI: std::sync::atomic::AtomicU32 =
    std::sync::atomic::AtomicU32::new(30_000);

/// 刷新 blur 软阈值快照（启动 / settings_set 调用；单位 = 设置原值）。
pub fn set_blur_soft_threshold(threshold: f32) {
    BLUR_SOFT_THRESHOLD_MILLI.store(
        (threshold.clamp(0.0, 100.0) * 1000.0).round() as u32,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// worker 侧读取（0-100）。
pub fn blur_soft_threshold() -> f64 {
    f64::from(BLUR_SOFT_THRESHOLD_MILLI.load(std::sync::atomic::Ordering::Relaxed)) / 1000.0
}
/// 灰度拉普拉斯 3×3 卷积核 [0,1,0;1,-4,1;0,1,0] 的方差（清晰度代理）。
pub fn laplacian_variance(gray: &image::GrayImage) -> Option<f64> {
    let (w, h) = (gray.width() as usize, gray.height() as usize);
    if w < 3 || h < 3 {
        return None;
    }
    let pixels = gray.as_raw();
    // Integer moments are exact for the bounded thumbnail input. This avoids
    // five checked pixel calls and a floating division on every pixel.
    let mut sum = 0i64;
    let mut squares = 0u64;
    let mut n = 0u64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let lap = i64::from(pixels[i + w])
                + i64::from(pixels[i - w])
                + i64::from(pixels[i + 1])
                + i64::from(pixels[i - 1])
                - 4 * i64::from(pixels[i]);
            n += 1;
            sum += lap;
            squares += (lap * lap) as u64;
        }
    }
    (n > 1).then(|| ((squares as f64 - (sum as f64).powi(2) / n as f64) / (n - 1) as f64).max(0.0))
}

/// 拉普拉斯方差 → 0-100 归一清晰度分（饱和曲线）。
pub fn normalize_blur_score(variance: f64) -> f64 {
    let v = variance.max(0.0);
    100.0 * v / (v + BLUR_LAPLACE_K)
}

/// High-resolution diagnostic regions. Final focus verdict awaits a validated model.
pub fn process_blur_task(db: &Db, db_dir: &Path, asset_id: i64, soft_threshold: f64) -> bool {
    let Some((path, _thumb_state)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return false; // 资产已删除（级联清任务前的防御兜底）
    };
    let _ = path;
    let Some((img, origin)) = super::selection_regions::thumbnail(db, db_dir, asset_id) else {
        let details =
            serde_json::json!({"reason": "source_unavailable", "regions": [], "calibrated": false});
        return db
            .set_ai_analysis_details(
                asset_id,
                "blur",
                "unknown",
                None,
                BLUR_ALGO_VERSION,
                &details,
            )
            .is_ok();
    };
    let mut evidence = super::selection_regions::evidence(&img, origin);
    evidence.models = vec!["regional-laplacian-sobel-v1-heuristic".into()];
    evidence.parameters = serde_json::json!({"sourceSize":super::selection_regions::ANALYSIS_SIZE,"parameterVersion":8,"softThreshold":soft_threshold});
    let gray = image::imageops::grayscale(&img);
    let faces: Vec<(f64, f64, f64, f64)> = {
        let Ok(mut stmt) = db.0.prepare(
            "SELECT box_x,box_y,box_w,box_h FROM faces WHERE asset_id=?1 ORDER BY box_w*box_h DESC",
        ) else {
            return false;
        };
        let Ok(rows) = stmt.query_map([asset_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        }) else {
            return false;
        };
        let Ok(rows) = rows.collect::<Result<Vec<_>, _>>() else {
            return false;
        };
        rows
    };
    let largest = faces.first().map(|f| f.2 * f.3).unwrap_or(0.0);
    for (x, y, w, h) in faces {
        if [x, y, w, h].iter().any(|n| !n.is_finite()) || w * h < largest * 0.25 {
            continue;
        }
        let px = (x.clamp(0.0, 1.0) * img.width() as f64) as u32;
        let py = (y.clamp(0.0, 1.0) * img.height() as f64) as u32;
        if px >= img.width() || py >= img.height() {
            continue;
        }
        let pw = ((w * img.width() as f64) as u32).min(img.width() - px);
        let ph = ((h * img.height() as f64) as u32).min(img.height() - py);
        if pw.min(ph) < 48 {
            continue;
        }
        let metric = super::focus_quality::measure(
            &image::imageops::crop_imm(&gray, px, py, pw, ph).to_image(),
            soft_threshold,
        );
        evidence.regions.push(super::selection_regions::Region {
            kind: "face".into(),
            person: 0,
            side: None,
            bounds: [x, y, w, h],
            state: metric.state.into(),
            reason: metric.reason.map(str::to_owned),
            raw_score: metric.score,
            auxiliary_ear: None,
        });
    }
    if !evidence.regions.iter().any(|r| r.raw_score.is_some()) {
        evidence.regions.clear();
        for (bounds, metric) in super::focus_quality::detail_regions(&gray, soft_threshold) {
            evidence.regions.push(super::selection_regions::Region {
                kind: "detail".into(),
                person: 0,
                side: None,
                bounds,
                state: metric.state.into(),
                reason: metric.reason.map(str::to_owned),
                raw_score: metric.score,
                auxiliary_ear: None,
            });
        }
    }
    let usable: Vec<_> = evidence
        .regions
        .iter()
        .filter(|r| r.raw_score.is_some())
        .collect();
    let score = usable
        .iter()
        .filter_map(|r| r.raw_score)
        .max_by(f64::total_cmp);
    let sharp = usable.iter().any(|r| r.state == "sharp");
    let soft = usable.iter().any(|r| r.state == "soft");
    let value = if usable.is_empty() {
        "unknown"
    } else if sharp && !soft {
        "sharp"
    } else if soft && !sharp && usable.iter().all(|r| r.state == "soft") {
        "soft"
    } else {
        "maybe"
    };
    evidence.reason = Some(
        if usable.is_empty() {
            "low_texture"
        } else {
            "focus_heuristic"
        }
        .into(),
    );
    db.set_ai_analysis_details(
        asset_id,
        "blur",
        value,
        score,
        BLUR_ALGO_VERSION,
        &serde_json::to_value(evidence).expect("selection evidence serializes"),
    )
    .is_ok()
}

/// faces 归一化框 `(x, y, w)` ∈ 0..1 → 缩略图像素裁剪窗 `(x, y, w, h)`
/// （外扩 20%、h=w 方窗；起点夹图内、宽不越右/下边界，最小 8px 但绝不清过
/// 可用边界——极小图/贴边框退化为可用窗，不 panic）。纯函数可测。
pub fn face_crop_window_px(
    bx: f64,
    by: f64,
    bw: f64,
    img_w: u32,
    img_h: u32,
) -> (u32, u32, u32, u32) {
    let (bx, by, bw) = (
        bx * f64::from(img_w),
        by * f64::from(img_h),
        bw * f64::from(img_w) * 1.2,
    );
    let x = (bx.max(0.0) as u32).min(img_w.saturating_sub(1));
    let y = (by.max(0.0) as u32).min(img_h.saturating_sub(1));
    let w = (bw.max(0.0) as u32).max(8).min(img_w - x);
    let h = w.min(img_h - y);
    (x, y, w, h)
}

// ---------------------------------------------------------------------------
// 闭眼（eyes）：SCRFD 检测 → facemesh ROI 推理 → EAR 三态
// ---------------------------------------------------------------------------

/// facemesh 清单 id（模型文件 root/facemesh.onnx）。
pub const FACEMESH_MODEL_ID: &str = "facemesh";
/// facemesh 输入边长（face_landmarker 256²）。
pub const EYES_INPUT_SIZE: u32 = 256;
/// facemesh 输出点数（468 眼睑网格 + 10 虹膜；虹膜不可用，见模块注释）。
pub const FACEMESH_POINTS: usize = 478;
/// eyes 通道算法版本标签（落 ai_analysis.model_version）。
pub const EYES_ALGO_VERSION: &str = "thumbnail-eye-1024-v9-binary";
/// ROI 外扩系数（MediaPipe detection_to_roi：1 + 2×margin，margin=0.25）。
const ROI_MARGIN_SCALE: f32 = 1.5;

/// 左眼 EAR 六点 [p1 外角, p2 上睑, p3 上睑内, p4 内角, p5 下睑内, p6 下睑]。
/// MediaPipe 468 点拓扑标准眼部索引（眼周环形结构经 yakhyo 仓 tessellation.py
/// 核实：相邻索引均有三角化边）。「左」= 被摄者左眼（图像右侧）。
pub const EYE_LEFT_EAR: [usize; 6] = [33, 160, 158, 133, 153, 144];
/// 右眼 EAR 六点（同上，「右」= 被摄者右眼，图像左侧）。
pub const EYE_RIGHT_EAR: [usize; 6] = [362, 385, 387, 263, 373, 380];

/// EAR 阈值运行时快照（×1000 整数原子存；启动 / settings_set 时刷新——同
/// blur 软阈值模式）。默认值真库标定（2026-09-28：497 张主库 335 张检出
/// 脸，min-EAR p10=0.089 / p50=0.241；≤0.13 眼睑贴合确判闭，0.13-0.20
/// 眯眼/半睁，≥0.22 确定睁眼——分布与逐张肉眼核验详见 settings 注释与
/// tests/ai_selection_test.rs 标定测试）。
static EYES_EAR_CLOSED_MILLI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(130);
static EYES_EAR_MAYBE_MILLI: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(200);

/// 刷新 EAR 阈值快照（启动 / settings_set 调用；closed < maybe 约定由
/// settings 侧校验兜底，此处钳制保序防脏值）。
pub fn set_eyes_ear_thresholds(closed: f32, maybe: f32) {
    let (closed, maybe) = (closed.clamp(0.0, 1.0), maybe.clamp(0.0, 1.0));
    let (closed, maybe) = if closed <= maybe {
        (closed, maybe)
    } else {
        (maybe, closed)
    };
    EYES_EAR_CLOSED_MILLI.store(
        (closed * 1000.0).round() as u32,
        std::sync::atomic::Ordering::Relaxed,
    );
    EYES_EAR_MAYBE_MILLI.store(
        (maybe * 1000.0).round() as u32,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// worker 侧读取：closed 阈（EAR < 此值判闭）。
pub fn eyes_ear_closed() -> f32 {
    EYES_EAR_CLOSED_MILLI.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
}

/// worker 侧读取：maybe 阈（closed ≤ EAR < 此值判疑似）。
pub fn eyes_ear_maybe() -> f32 {
    EYES_EAR_MAYBE_MILLI.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0
}

/// EAR = (‖p2−p6‖ + ‖p3−p5‖) / (2‖p1−p4‖)（Soukupová & Čech 2016 六点
/// 定义；纯函数可测）。眼宽非有限/≤0 或高度非有限 → None（该眼不可判定）。
pub fn eye_aspect_ratio(lm: &[[f32; 2]], idx: &[usize; 6]) -> Option<f32> {
    let p = |i: usize| -> Option<[f32; 2]> { lm.get(idx[i]).copied() };
    let (p1, p4) = (p(0)?, p(3)?);
    let width = dist(p1, p4);
    if !width.is_finite() || width <= f32::EPSILON {
        return None;
    }
    let v1 = dist(p(1)?, p(5)?);
    let v2 = dist(p(2)?, p(4)?);
    if !v1.is_finite() || !v2.is_finite() {
        return None;
    }
    Some((v1 + v2) / (2.0 * width))
}

/// 两点欧氏距离。
fn dist(a: [f32; 2], b: [f32; 2]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

/// 三态聚合（纯函数，可测）：逐脸 EAR（None = 质量不可判定）→ 资产级
/// (value, score)。任一脸 closed → closed；否则任一 maybe → maybe；全睁
/// → 无记录（None，≠ unknown）；空（无人脸）→ 无记录；全 None（有人脸
/// 但全不可判定）→ unknown 保守三态。score = 闭眼置信 0..1
/// （`(maybe − ear) / maybe` 截断，越闭越高；unknown 记 0.0——前端契约
/// 要求 score 为有限数值，null 会被整通道剔除）。
pub fn aggregate_eyes_ear(
    face_ears: &[Option<f32>],
    closed_threshold: f32,
    maybe_threshold: f32,
) -> Option<(String, f64)> {
    if face_ears.is_empty() {
        return None; // 无人脸：分析完成但不产生记录
    }
    let valid: Vec<f32> = face_ears
        .iter()
        .filter_map(|e| e.filter(|v| v.is_finite()))
        .collect();
    if valid.is_empty() {
        return Some(("unknown".into(), 0.0));
    }
    let worst = valid.iter().copied().fold(f32::INFINITY, f32::min);
    let closedness = |ear: f32| -> f64 {
        f64::from(((maybe_threshold - ear) / maybe_threshold).clamp(0.0, 1.0))
    };
    if worst < closed_threshold {
        Some(("closed".into(), closedness(worst)))
    } else if worst < maybe_threshold {
        Some(("maybe".into(), closedness(worst)))
    } else {
        None // 全睁：分析完成但不产生记录
    }
}

/// Per-eye classifier contract. Unknown evidence is distinct from inference
/// failure; the latter is retried by the task system. EAR is not required.
pub trait FaceEyeStateClassifier: Send + Sync {
    fn classify(
        &self,
        img: &image::RgbImage,
        face: &super::face::DetectedFace,
        person: usize,
    ) -> Result<Vec<super::selection_regions::Region>, String>;
}

/// SCRFD 脸 → facemesh ROI 相似矩阵（2×3 正映射，源像素 → `out`² 裁剪
/// 像素）：绕框中心旋转 −θ（θ = 双眼连线角，y 轴向下）使眼线水平、
/// 缩放 `out / (1.5 × 框长边)`、平移使 ROI 中心落裁剪中心。与参考实现
/// （cv2.getRotationMatrix2D + 平移补偿）数学等价——对眼线方向向量做
/// 旋转后恒为水平（合成关键点往返测试把关）。退化（框长边非有限/≤0、
/// 双眼点重合）→ None。
pub fn face_roi_matrix(face: &super::face::DetectedFace, out: u32) -> Option<[[f32; 3]; 2]> {
    let side = ROI_MARGIN_SCALE * face.box_w.max(face.box_h);
    if !side.is_finite() || side <= f32::EPSILON {
        return None;
    }
    let (dx, dy) = (
        face.kps[1][0] - face.kps[0][0],
        face.kps[1][1] - face.kps[0][1],
    );
    let norm = (dx * dx + dy * dy).sqrt();
    if !norm.is_finite() || norm <= f32::EPSILON {
        return None; // 双眼点重合：旋转未定义
    }
    // 眼线单位方向 (c, s) 与其法向 (s, −c)：正映射把眼线旋到 +x 轴
    let (c, s) = (dx / norm, dy / norm);
    let scale = out as f32 / side;
    let (cx, cy) = (face.box_x + face.box_w / 2.0, face.box_y + face.box_h / 2.0);
    let (a, b, d, e) = (scale * c, scale * s, -scale * s, scale * c);
    Some([
        [a, b, out as f32 / 2.0 - (a * cx + b * cy)],
        [d, e, out as f32 / 2.0 - (d * cx + e * cy)],
    ])
}

/// 2×3 相似矩阵逆映射（裁剪像素 → 源像素）：src = inv2×2 · (crop − t)。
/// 行列式退化 → None。纯函数可测（landmark 回映与测试共用）。
pub fn map_similarity_inv(m: &[[f32; 3]; 2], x: f32, y: f32) -> Option<(f32, f32)> {
    let det = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    if det.abs() < f32::EPSILON {
        return None;
    }
    let (dx, dy) = (x - m[0][2], y - m[1][2]);
    Some((
        (m[1][1] * dx - m[0][1] * dy) / det,
        (-m[1][0] * dx + m[0][0] * dy) / det,
    ))
}

/// 几何自洽带：IOD（‖lm33 − lm263‖ 源像素）/ ROI 边长。MediaPipe 规范
/// 人脸该比值稳定（真库 497 张标定：335 张检出脸中仅 5 张落带外 →
/// unknown，见模块注释）；带外 = 网格未对齐/侧脸挤压 → 该脸 unknown。
pub const IOD_ROI_RATIO_MIN: f32 = 0.15;
/// 几何自洽带上界。
pub const IOD_ROI_RATIO_MAX: f32 = 0.70;

// ---------------------------------------------------------------------------
// facemesh 会话槽 + 推理
// ---------------------------------------------------------------------------

/// 惰性会话槽（与 face.rs 同款互斥串行约定；模型件单一无档位切换）。
fn facemesh_slots() -> &'static Mutex<Option<ort::session::Session>> {
    static SLOTS: std::sync::OnceLock<Mutex<Option<ort::session::Session>>> =
        std::sync::OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(None))
}

/// 空闲卸载（ai::idle::release_all 调用；取锁置空，在途推理不受影响）。
pub fn release() {
    super::selection_defocus::release();
    super::selection_regions::release();
    *facemesh_slots()
        .lock()
        .expect("facemesh slots mutex poisoned") = None;
}

/// 生产分类器：SCRFD 双眼点 → ROI 旋转裁剪 → facemesh 推理 → EAR/质量门。
pub struct FacemeshEarClassifier<'a> {
    manager: &'a super::ModelManager,
}

impl<'a> FacemeshEarClassifier<'a> {
    pub fn new(manager: &'a super::ModelManager) -> Self {
        Self { manager }
    }

    /// facemesh 推理：ROI 矩阵 → 256² 裁剪 → RGB /255 NCHW → 会话 Run
    /// （DML 优先，毒化按模型 id `facemesh` 隔离）→ landmarks（478×3 裁剪
    /// 像素；score 头留档解析但不作判据，见模块注释）回映源图坐标返回。
    fn infer(
        &self,
        img: &image::RgbImage,
        face: &super::face::DetectedFace,
    ) -> Result<Vec<[f32; 2]>, String> {
        let matrix = face_roi_matrix(face, EYES_INPUT_SIZE)
            .ok_or_else(|| "ROI 构造退化（框/眼点无效）".to_string())?;
        let crop = super::face::warp_similarity_sized(img, &matrix, EYES_INPUT_SIZE);
        // 预处理照抄参考实现：RGB /255，NCHW f32
        let mut data = Vec::with_capacity(3 * EYES_INPUT_SIZE as usize * EYES_INPUT_SIZE as usize);
        for ch in 0..3 {
            for px in crop.pixels() {
                data.push(px.0[ch] as f32 / 255.0);
            }
        }
        let use_gpu = self.manager.ai_params().use_gpu;
        super::run_with_acceleration_fallback(
            use_gpu,
            FACEMESH_MODEL_ID,
            || {
                let tensor = ort::value::Tensor::from_array((
                    vec![
                        1i64,
                        3,
                        i64::from(EYES_INPUT_SIZE),
                        i64::from(EYES_INPUT_SIZE),
                    ],
                    data.clone(),
                ))
                .map_err(|e| format!("构造 facemesh 张量失败: {e}"))?;
                super::idle::touch();
                let mut slots = facemesh_slots()
                    .lock()
                    .expect("facemesh slots mutex poisoned");
                if slots.is_none() {
                    let path = self.manager.model_path(FACEMESH_MODEL_ID);
                    if !path.is_file() {
                        return Err(format!(
                            "模型 {FACEMESH_MODEL_ID} 未下载（设置页下载后再试）"
                        ));
                    }
                    *slots = Some(super::build_session(
                        &path,
                        None,
                        use_gpu,
                        FACEMESH_MODEL_ID,
                    )?);
                }
                let session = slots.as_mut().expect("上文已保证");
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
                    .map_err(|e| format!("facemesh 推理失败: {e}"))?;
                // 输出按形状定位（导出件命名 landmarks/score，按形状取用最稳）：
                // 秩 3 且 [.,≥468,3] = landmarks（裁剪像素，回映源图坐标）；
                // 单元素输出 = score logit（解析留档，不作判据——见模块注释）。
                let mut landmarks: Option<Vec<[f32; 2]>> = None;
                let mut _score_logit = f32::NAN;
                for name in &output_names {
                    let Some(value) = outputs.get(name) else {
                        continue;
                    };
                    let Ok((shape, extracted)) = value.try_extract_tensor::<f32>() else {
                        continue;
                    };
                    if shape.len() == 3
                        && shape[1] >= FACEMESH_POINTS as i64
                        && shape[2] == 3
                        && extracted.len() >= FACEMESH_POINTS * 3
                    {
                        let mut pts = Vec::with_capacity(FACEMESH_POINTS);
                        for i in 0..FACEMESH_POINTS {
                            let (x, y) = (extracted[i * 3], extracted[i * 3 + 1]);
                            let (sx, sy) =
                                map_similarity_inv(&matrix, x, y).unwrap_or((f32::NAN, f32::NAN));
                            pts.push([sx, sy]);
                        }
                        landmarks = Some(pts);
                    } else if extracted.len() == 1 {
                        _score_logit = extracted[0];
                    }
                }
                let landmarks = landmarks
                    .ok_or_else(|| format!("facemesh 输出缺 landmarks（{FACEMESH_POINTS}×3）"))?;
                Ok(landmarks)
            },
            || {
                let mut slots = facemesh_slots()
                    .lock()
                    .expect("facemesh slots mutex poisoned");
                *slots = None;
            },
        )
    }
}

impl FaceEyeStateClassifier for FacemeshEarClassifier<'_> {
    fn classify(
        &self,
        img: &image::RgbImage,
        face: &super::face::DetectedFace,
        person: usize,
    ) -> Result<Vec<super::selection_regions::Region>, String> {
        use super::selection_regions::{binary_eye_state, detected_eye_bounds, Region};
        let lm = self.infer(img, face)?;
        let mut results = Vec::new();
        for (side, indices) in [("left", EYE_LEFT_EAR), ("right", EYE_RIGHT_EAR)] {
            let points: Vec<_> = indices.iter().map(|i| lm[*i]).collect();
            let ear = eye_aspect_ratio(&lm, &indices).filter(|value| value.is_finite());
            let bounds = detected_eye_bounds(img, &points);
            let detected = bounds.is_some() && ear.is_some();
            results.push(Region {
                kind: "eye".into(),
                person,
                side: Some(side.into()),
                bounds: bounds.unwrap_or_else(|| {
                    super::face::normalized_box(face, img.width(), img.height())
                }),
                state: if detected {
                    binary_eye_state(ear.unwrap(), eyes_ear_closed())
                } else {
                    "not_detected"
                }
                .into(),
                reason: (!detected).then(|| "eye_not_detected".into()),
                raw_score: None,
                auxiliary_ear: ear.map(f64::from),
            });
        }
        Ok(results)
    }
}

impl FacemeshEarClassifier<'_> {
    /// Legacy comparison only; the production classifier contract is per-eye.
    pub fn face_min_ear(
        &self,
        img: &image::RgbImage,
        face: &super::face::DetectedFace,
    ) -> Result<Option<f32>, String> {
        let lm = self.infer(img, face)?;
        // 几何自洽带：IOD/ROI 边长比（虹膜辅助判据的替代质量门，
        // 见模块注释——虹膜点该导出不可用）
        let roi_side = ROI_MARGIN_SCALE * face.box_w.max(face.box_h);
        let (Some(l_outer), Some(r_outer)) = (lm.get(33), lm.get(263)) else {
            return Ok(None);
        };
        let iod = dist(*l_outer, *r_outer);
        let ratio = iod / roi_side;
        if !ratio.is_finite() || !(IOD_ROI_RATIO_MIN..=IOD_ROI_RATIO_MAX).contains(&ratio) {
            return Ok(None); // 网格未对齐/极端侧脸 → unknown
        }
        // 双眼 EAR 取更差；单眼退化（眼宽过小等）跳过该眼；双眼全退化 → None
        let ears: Vec<f32> = [EYE_LEFT_EAR, EYE_RIGHT_EAR]
            .iter()
            .filter_map(|idx| eye_aspect_ratio(&lm, idx))
            .collect();
        if ears.is_empty() {
            return Ok(None);
        }
        Ok(Some(ears.iter().copied().fold(f32::INFINITY, f32::min)))
    }
}

/// Oriented source -> tiled SCRFD -> per-eye evidence -> conservative summary.
/// Missing source is unknown; detector/inference errors use task retries.
pub fn process_eyes_task(
    db: &Db,
    db_dir: &Path,
    manager: &super::ModelManager,
    classifier: &dyn FaceEyeStateClassifier,
    asset_id: i64,
) -> bool {
    let Some((path, _)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return false; // 资产已删除（级联清任务前的防御兜底）
    };
    let _ = path;
    let size = super::selection_regions::ANALYSIS_SIZE;
    let Some((img, origin)) = super::selection_regions::thumbnail(db, db_dir, asset_id) else {
        return db.set_ai_analysis_details(asset_id, "eyes", "unknown", None, EYES_ALGO_VERSION,
            &serde_json::json!({"reason":"source_unavailable", "regions":[], "calibrated":false})).is_ok();
    };
    let faces = match super::face::detect_faces(manager, &img) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut evidence = super::selection_regions::evidence(&img, origin);
    evidence.models = [
        super::face_detect_model_id(manager.ai_params().quality_tier),
        FACEMESH_MODEL_ID,
    ]
    .iter()
    .map(|id| {
        super::catalog()
            .iter()
            .find(|m| m.id == **id)
            .map(|m| format!("{}:{}", m.id, m.version))
            .unwrap_or_else(|| id.to_string())
    })
    .collect();
    evidence.parameters = serde_json::json!({"sourceSize":size,"parameterVersion":9,
        "decisionMode":"binary-ear","earClosed":eyes_ear_closed(),
        "includeSingle":super::selection_regions::include_single(),
        "faceDetectThreshold":manager.ai_params().face_detect_threshold,
        "minimumEyeWidth":super::selection_regions::MIN_EYE_DETECTION_PIXELS});
    for (index, face) in faces.into_iter().enumerate() {
        match classifier.classify(&img, &face, index + 1) {
            Ok(regions) => evidence.regions.extend(regions),
            Err(_) => return false,
        }
    }
    let value = super::selection_regions::photo_eyes(
        &evidence.regions,
        super::selection_regions::include_single(),
    );
    evidence.reason = (value == "no_eye").then(|| "eye_not_detected".into());
    db.set_ai_analysis_details(
        asset_id,
        "eyes",
        value,
        None,
        EYES_ALGO_VERSION,
        &serde_json::to_value(evidence).expect("selection evidence serializes"),
    )
    .is_ok()
}

/// Bounded parallel eye analysis; CPU/cache preparation overlaps shared ONNX
/// sessions. Missing models leave tasks pending; failures retain capped retries.
pub fn run_eyes_backfill(
    db_dir: &Path,
    manager: &super::ModelManager,
    bus: &crate::events::EventBus,
) -> u64 {
    run_eyes_backfill_gated(db_dir, manager, bus, None)
}

fn run_eyes_backfill_gated(
    db_dir: &Path,
    manager: &super::ModelManager,
    bus: &crate::events::EventBus,
    controls: Option<&crate::tasks::TaskControls>,
) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return 0;
    };
    if !manager.selection_eyes_ready() {
        // 模型未收录/未就绪：跳过并计数（pending 不动，不空转处理）
        let pending = db.pending_index_task_count("eyes").unwrap_or(0);
        if pending > 0 {
            eprintln!("闭眼通道：眼部状态模型未收录，{pending} 条任务保持 pending 跳过");
        }
        return 0;
    }
    let total = db.pending_index_task_count("eyes").unwrap_or(0);
    let processed = std::sync::atomic::AtomicU64::new(0);
    let workers = crate::tasks::index_parallelism();
    // Parallel CPU/cache preparation feeds the existing serialized ONNX slots;
    // no duplicate GPU sessions; image tasks share the CPU/2 admission budget.
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let processed = &processed;
            scope.spawn(move || run_eyes_worker(db_dir, manager, bus, controls, processed, total));
        }
    });
    processed.load(std::sync::atomic::Ordering::Relaxed)
}

fn run_eyes_worker(
    db_dir: &Path,
    manager: &super::ModelManager,
    bus: &crate::events::EventBus,
    controls: Option<&crate::tasks::TaskControls>,
    processed: &std::sync::atomic::AtomicU64,
    total: u64,
) {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return;
    };
    let classifier = FacemeshEarClassifier::new(manager);
    loop {
        while controls.is_some_and(|c| c.is_paused() && !c.is_cancelled()) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if controls.is_some_and(|c| c.is_cancelled()) {
            break;
        }
        let _permit = crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image)
            .acquire(1);
        if controls.is_some_and(|c| c.is_paused() || c.is_cancelled()) {
            continue;
        }
        let barrier = crate::index::image_index_lock(db_dir);
        let _guard = barrier.read().expect("image index barrier poisoned");
        let Some(task) = db.claim_index_task("eyes").ok().flatten() else {
            break;
        };
        let ok = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_eyes_task(&db, db_dir, manager, &classifier, task.asset_id)
        })) {
            Ok(ok) => ok,
            Err(_) => {
                eprintln!(
                    "闭眼回填：任务 asset_id={} 处理 panic，按失败回收",
                    task.asset_id
                );
                false
            }
        };
        let _ = db.finish_index_task(task.id, ok);
        let done = processed.fetch_add(u64::from(ok), std::sync::atomic::Ordering::Relaxed)
            + u64::from(ok);
        bus.publish(crate::events::AppEvent::IndexTaskProgress {
            kind: "eyes".into(),
            done,
            total,
        });
    }
}

/// 便利入口：模型收录后统一触发（今日恒早退）。
pub fn kick_eyes_if_ready(
    db_dir: PathBuf,
    manager: &super::ModelManager,
    bus: &crate::events::EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    if !manager.selection_eyes_ready() {
        return;
    }
    let manager = std::sync::Arc::new(manager.clone());
    let bus = bus.clone();
    let _ = supervisor.spawn_coalesced(
        "index",
        format!("eyes-backfill:{}", db_dir.display()),
        move |controls| {
            run_eyes_backfill_gated(&db_dir, &manager, &bus, Some(&controls));
        },
    );
}
