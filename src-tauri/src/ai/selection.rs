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
//! ## 闭眼（eyes 通道）
//! 独立 index_tasks 通道：任务处理时以 SCRFD 现场检测（5 关键点含双眼
//! 位置），裁双眼区域送眼部状态分类器（trait 注入，ONNX 实现随模型收录
//! 落地；模型选型未过——见 ai::mod CATALOG 注释——今天恒为「未就绪」，
//! 通道跳过并计数，任务保持 pending）。检测源与 face 通道同款降档
//! （face::detection_source，缓存档优先，不再无条件生成 2048 档）。
//! 无人脸 → done 且不产生记录（≠ 没有闭眼，UI 语义注意）；分类不确定/
//! 遮挡/单眼 → `unknown` 保守三态。

use std::path::{Path, PathBuf};

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
// 闭眼（eyes）：SCRFD 现场检测 → 双眼裁剪 → 分类器
// ---------------------------------------------------------------------------

/// 眼部状态分类器（trait 注入：ONNX 实现随模型收录落地；测试用 stub）。
/// 输入 = 双眼拼接裁剪（RGB）；输出 = 闭合概率 p(closed) ∈ 0..1。
/// （模型收录前 lib 侧无调用方——stub 测试与未来实现使用。）
#[allow(dead_code)]
pub trait EyeStateClassifier: Send + Sync {
    fn classify_closed(&self, eye_crop: &image::RgbImage) -> Result<f32, String>;
}

/// 闭眼判定阈值：p ≥ 0.7 判 closed；0.5 < p < 0.7 记 maybe；≤ 0.5 睁眼。
#[allow(dead_code)]
pub const EYES_CLOSED_THRESHOLD: f32 = 0.7;
/// maybe 判定下阈。
#[allow(dead_code)]
pub const EYES_MAYBE_THRESHOLD: f32 = 0.5;
/// 分类器版本标签（未收录模型 → 通道不产出记录，该标签不落库）。
#[allow(dead_code)]
pub const EYES_MODEL_VERSION: &str = "unavailable";

/// 三态聚合（纯函数，可测）：逐眼 p(closed) → 资产级
/// (value, score)。任一 closed → closed（score=最大 p）；否则任一 maybe →
/// maybe；否则全睁 → 无记录（None，≠ unknown）；空（无人脸/无有效眼）→
/// unknown（遮挡/侧脸不可判定，保守提示）。
#[allow(dead_code)]
pub fn aggregate_eyes(
    eye_probs: &[f32],
    closed_threshold: f32,
    maybe_threshold: f32,
) -> Option<(String, f64)> {
    if eye_probs.is_empty() {
        return Some(("unknown".into(), 0.0));
    }
    let max_p = eye_probs.iter().copied().fold(0.0f32, f32::max);
    if eye_probs.iter().any(|&p| p >= closed_threshold) {
        Some(("closed".into(), f64::from(max_p)))
    } else if eye_probs.iter().any(|&p| p >= maybe_threshold) {
        Some(("maybe".into(), f64::from(max_p)))
    } else {
        None // 全睁：分析完成但不产生记录
    }
}

/// 双眼裁剪：以 kps 眼点为中心、按脸宽比例取窗（眼窗宽 = 0.32×脸宽，
/// 高 = 0.55×眼窗宽），左右眼各裁一片后水平拼接。
#[allow(dead_code)]
fn crop_eyes(img: &image::RgbImage, face: &super::face::DetectedFace) -> Option<image::RgbImage> {
    let (left, right) = (face.kps[0], face.kps[1]);
    let eye_w = face.box_w * 0.32;
    let eye_h = eye_w * 0.55;
    if eye_w < 4.0 || eye_h < 4.0 {
        return None; // 脸太小：裁不出有效眼窗
    }
    let crop_eye = |c: [f32; 2]| -> Option<image::RgbImage> {
        let x = (c[0] - eye_w / 2.0).max(0.0) as u32;
        let y = (c[1] - eye_h / 2.0).max(0.0) as u32;
        let w = (eye_w as u32).min(img.width().saturating_sub(x));
        let h = (eye_h as u32).min(img.height().saturating_sub(y));
        if w == 0 || h == 0 {
            return None;
        }
        Some(image::imageops::crop_imm(img, x, y, w, h).to_image())
    };
    let l = crop_eye(left)?;
    let r = crop_eye(right)?;
    // 拼接：左右眼并排（中间 2px 分隔）
    let mut out = image::RgbImage::new(l.width() + r.width() + 2, l.height().max(r.height()));
    image::imageops::overlay(&mut out, &l, 0, 0);
    image::imageops::overlay(&mut out, &r, i64::from(l.width() + 2), 0);
    Some(out)
}

/// eyes 任务核：检测源取图（face 通道同款降档：缓存档优先 [512, 2048]，
/// 全未命中才生成 512——不再无条件同步生成 2048 档）→ SCRFD 现场检测 →
/// 双眼裁剪分类 → 三态聚合 → ai_analysis('eyes')。无人脸 → done 且无
/// 记录；检测/分类失败 → false（走 attempts 封顶重试）。
/// （模型收录前 lib 侧无调用方——由 run_eyes_backfill 就绪分支与测试使用。）
#[allow(dead_code)]
pub fn process_eyes_task(
    db: &Db,
    db_dir: &Path,
    manager: &super::ModelManager,
    classifier: &dyn EyeStateClassifier,
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
    let mut probs: Vec<f32> = Vec::new();
    for face in &faces {
        if let Some(eyes) = crop_eyes(&img, face) {
            match classifier.classify_closed(&eyes) {
                Ok(p) => probs.push(p.clamp(0.0, 1.0)),
                Err(_) => return false, // 分类器故障：按失败重试
            }
        }
    }
    match aggregate_eyes(&probs, EYES_CLOSED_THRESHOLD, EYES_MAYBE_THRESHOLD) {
        Some((value, score)) => db
            .set_ai_analysis(
                asset_id,
                "eyes",
                Some(&value),
                Some(score),
                EYES_MODEL_VERSION,
            )
            .is_ok(),
        None => true, // 全睁：done 且不产生记录
    }
}

/// eyes 回填 + 串行执行：模型未就绪 → 跳过并计数（任务保持 pending，
/// 不占失败额度；模型收录后 kick 自然续跑）。返回本轮成功数。
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
    // 分类器构造随模型收录落地（ONNX IO 依赖具体模型规格）；今日恒不走此支
    let _ = &db;
    let _ = bus;
    0
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
