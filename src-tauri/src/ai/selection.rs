//! AI 辅助选片（阶段 C，0021）：闭眼检测 + 疑似失焦标签。
//!
//! 输出只是可筛选建议（ai_analysis 表），与用户决定分层，**绝不自动写
//! XMP**（roadmap §6；XMP 写入仅由用户显式选片操作触达）。
//!
//! ## 失焦（blur 通道，无模型依赖，始终可用）
//! 512 档已缓存缩略图上计算拉普拉斯方差 → 饱和归一 0-100 清晰度分；
//! 有人脸框（faces 表，**归一化 0..1 坐标**——2026-09-28 v2 空间，随
//! face.rs process 管线切换；映射见 [`face_crop_window_px`]）时取「全图与
//! 人脸局部最小」双分保守判定。value：`soft`（低于保守阈值，疑似）/
//! `sharp` / `unknown`（缩略图缺失或解码失败）。运动模糊/浅景深天然误报
//! ——分数给 UI 展示，不硬判。
//!
//! ## 闭眼（eyes 通道，2026-09-28 实装）
//! 独立 index_tasks 通道：任务处理时以 SCRFD 现场检测（5 关键点含双眼
//! 位置），双眼点构造旋转 + MediaPipe ROI（正方形，检测框长边 ×1.5）裁
//! 256² → **facemesh**（MediaPipe Face Landmarker 478 点 ONNX，
//! Apache-2.0，收录见 ai::mod CATALOG 注释）推理 468 点眼睑网格 →
//! EAR（眼部长宽比）几何判据 → 三态（closed / maybe / unknown）。
//! 检测源与 face 通道同款降档（face::detection_source，缓存档优先）。
//!
//! ### 预处理（照抄 yakhyo/mediapipe-face-mesh-onnx `models/onnx_model.py`
//! 的 FaceMesh 类，2026-09-28 核实）
//! - ROI = 正方形 `(1 + 2×0.25) × max(box_w, box_h)`（= 1.5 倍检测框
//!   长边，MediaPipe detection_to_roi 规则），中心 = 框中心，旋转角 =
//!   双眼连线角（SCRFD kps[0] 左眼 → kps[1] 右眼，`atan2(dy, dx)`）。
//! - 相似变换一次重采样直出 256²（避免二次插值），RGB `/255` NCHW f32。
//! - landmarks 输出为 256 裁剪像素坐标，经 ROI 逆相似矩阵回映源图坐标。
//!
//! ### 实装校准注记（Python/ONNXRuntime 对照官方 face_landmarker，实测）
//! - 468 点眼睑网格与官方对齐良好（双眼部索引环形结构经仓库
//!   tessellation.py 核实存在：眼周三角化边 [160,159]/[159,158]/
//!   [153,154]/[145,144]/[385,384]/[387,386]/[373,374]/[374,380] 等）。
//! - **虹膜点 468-477 不可用**：相对眼睑环中心漂移 0.01-0.6 IOD 无规律
//!   （官方模型 iris 应 <0.1 IOD）→ 原计划「虹膜辅助判据」弃用，质量
//!   门槛改用几何自洽带（IOD/ROI 边长比，带值真库标定见阈值注释）。
//! - **score 头不判据**：导出仓 docstring 称「confident faces 20-40」，
//!   实测正脸裁剪 logit −8~−31，且人脸/背景无判别方向（2026-09-28
//!   多图对照）→ 该头输出解析但不参与判定，人脸存在性由 SCRFD 检测
//!   置信（≥ settings.ai.face_detect_threshold）+ 几何自洽带把关。
//!
//! ### 三态分带（阈值 settings.ai.eyes_ear_closed / eyes_ear_maybe，真库标定）
//! EAR 越小越闭：脸级取双眼**更差**（min）EAR；资产级任一脸 closed →
//! closed，否则任一 maybe → maybe，否则全睁不落记录。质量差（几何自洽
//! 带外 / 小脸 / 非有限值）的脸记 unknown 维度；无人脸 → done 且不产生
//! 记录（≠ 没有闭眼，UI 语义注意）。

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
pub const BLUR_ALGO_VERSION: &str = "laplacian-v1";

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
    let px = |x: usize, y: usize| -> f64 { f64::from(gray.get_pixel(x as u32, y as u32).0[0]) };
    // 逐像素拉普拉斯响应（跳 1px 边界），welford 单遍方差（数值稳定）
    let mut mean = 0f64;
    let mut m2 = 0f64;
    let mut n = 0u64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let lap = px(x, y + 1) + px(x, y - 1) + px(x + 1, y) + px(x - 1, y) - 4.0 * px(x, y);
            n += 1;
            let d = lap - mean;
            mean += d / n as f64;
            m2 += d * (lap - mean);
        }
    }
    (n > 1).then(|| m2 / (n - 1) as f64)
}

/// 拉普拉斯方差 → 0-100 归一清晰度分（饱和曲线）。
pub fn normalize_blur_score(variance: f64) -> f64 {
    let v = variance.max(0.0);
    100.0 * v / (v + BLUR_LAPLACE_K)
}

/// blur 任务核：512 档缩略图 → 全图分（有人脸框时并入人脸局部最小双分）
/// → ai_analysis('blur')。缩略图缺失/解码失败 → value='unknown' 按完成收尾。
pub fn process_blur_task(db: &Db, db_dir: &Path, asset_id: i64, soft_threshold: f64) -> bool {
    let Some((path, _thumb_state)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return false; // 资产已删除（级联清任务前的防御兜底）
    };
    let src = PathBuf::from(&path);
    let verdict = (|| -> Option<(Option<String>, f64)> {
        let thumb = crate::thumbs::thumb_file(db_dir, &src, 512)?;
        let img = image::ImageReader::open(&thumb)
            .ok()?
            .decode()
            .ok()?
            .to_luma8();
        let full = laplacian_variance(&img)?;
        let mut worst = normalize_blur_score(full);
        // 人脸局部双分：faces 表框为归一化 0..1（v2 坐标空间）→ 按缩略图
        // 实际宽高映射回像素
        let faces: Vec<(f64, f64, f64)> = {
            let mut stmt =
                db.0.prepare("SELECT box_x, box_y, box_w FROM faces WHERE asset_id = ?1")
                    .ok()?;
            let rows = stmt
                .query_map([asset_id], |r| {
                    Ok((
                        r.get::<_, f64>(0)?,
                        r.get::<_, f64>(1)?,
                        r.get::<_, f64>(2)?,
                    ))
                })
                .ok()?;
            rows.collect::<Result<Vec<_>, _>>().ok()?
        };
        for (bx, by, bw) in faces {
            let (x, y, w, h) = face_crop_window_px(bx, by, bw, img.width(), img.height());
            let crop = image::imageops::crop_imm(&img, x, y, w, h).to_image();
            if let Some(v) = laplacian_variance(&crop) {
                let local = normalize_blur_score(v);
                if local < worst {
                    worst = local;
                }
            }
        }
        Some((
            Some(
                if worst < soft_threshold {
                    "soft"
                } else {
                    "sharp"
                }
                .to_string(),
            ),
            (worst * 100.0).round() / 100.0,
        ))
    })();
    match verdict {
        Some((value, score)) => db
            .set_ai_analysis(
                asset_id,
                "blur",
                value.as_deref(),
                Some(score),
                BLUR_ALGO_VERSION,
            )
            .is_ok(),
        None => {
            // 缩略图缺失/解码失败：unknown 记录 + 按完成收尾（不占重试额度）
            db.set_ai_analysis(asset_id, "blur", Some("unknown"), None, BLUR_ALGO_VERSION)
                .is_ok()
        }
    }
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
pub const EYES_ALGO_VERSION: &str = "facemesh-ear-v1";
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

/// 单脸眼睛状态分类器（trait 注入：生产 = [`FacemeshEarClassifier`]；
/// 测试用 stub）。输入 = 源图 + SCRFD 检出脸；输出 = 该脸双眼中**更差**
/// （min）EAR；关键点质量差/小脸/几何自洽带外 → None（unknown 维度）。
/// 推理故障 → Err（任务按失败重试，不静默吞）。
pub trait FaceEyeStateClassifier: Send + Sync {
    fn face_min_ear(
        &self,
        img: &image::RgbImage,
        face: &super::face::DetectedFace,
    ) -> Result<Option<f32>, String>;
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
        super::run_with_dml_fallback(
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
                    if shape.len() == 3 && shape[1] >= 468 && shape[2] == 3 {
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
    fn face_min_ear(
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

/// eyes 任务核：检测源取图（face 通道同款降档）→ SCRFD 现场检测 →
/// 逐脸 facemesh EAR → 三态聚合 → ai_analysis('eyes')。无人脸 → done 且
/// 无记录；检测/分类失败 → false（走 attempts 封顶重试）。
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
    let thumb = match super::face::detection_source(db_dir, Path::new(&path)) {
        Some(t) => t,
        None => return false,
    };
    let img = match image::ImageReader::open(&thumb)
        .ok()
        .and_then(|r| r.decode().ok())
        .map(|d| d.to_rgb8())
    {
        Some(i) => i,
        None => return false,
    };
    let faces = match super::face::detect_faces(manager, &img) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut face_ears: Vec<Option<f32>> =
        Vec::with_capacity(faces.len().min(super::face::MAX_FACES_PER_IMAGE));
    for face in faces.into_iter().take(super::face::MAX_FACES_PER_IMAGE) {
        match classifier.face_min_ear(&img, &face) {
            Ok(ear) => face_ears.push(ear),
            Err(_) => return false, // 推理故障：按失败重试
        }
    }
    match aggregate_eyes_ear(&face_ears, eyes_ear_closed(), eyes_ear_maybe()) {
        Some((value, score)) => db
            .set_ai_analysis(
                asset_id,
                "eyes",
                Some(&value),
                Some((score * 1000.0).round() / 1000.0),
                EYES_ALGO_VERSION,
            )
            .is_ok(),
        None => true, // 全睁或无人脸：done 且不产生记录
    }
}

/// eyes 回填 + 串行执行（单 worker：SCRFD + facemesh 会话互斥天然串行，
/// 与 face 通道阶段 1 同款约束；无聚类等顺序敏感状态，无需两阶段）：
/// 模型未就绪 → 跳过并计数（任务保持 pending，不占失败额度；模型装好
/// 后 kick 自然续跑）。panic 归一为该任务失败（attempts 封顶回收）。
/// 返回本轮成功数。
pub fn run_eyes_backfill(
    db_dir: &Path,
    manager: &super::ModelManager,
    bus: &crate::events::EventBus,
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
    let classifier = FacemeshEarClassifier::new(manager);
    let total = db.pending_index_task_count("eyes").unwrap_or(0);
    let mut done = 0u64;
    while let Some(task) = db.claim_index_task("eyes").ok().flatten() {
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
        done += u64::from(ok);
        bus.publish(crate::events::AppEvent::IndexTaskProgress {
            kind: "eyes".into(),
            done,
            total,
        });
    }
    done
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
    let _ = supervisor.spawn_unique("index", "eyes-backfill".into(), move |_| {
        run_eyes_backfill(&db_dir, &manager, &bus);
    });
}
