//! Region evidence for selection. Candidate scores are uncalibrated, never
//! accuracy percentages. Inference failure and absent source are not an open eye.
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

pub const EYE_MODEL: &str = "open-closed-eye";
pub const PREPROCESS_VERSION: &str = "thumbnail-1024-eye-bgr-127-255-v8";
pub const ANALYSIS_SIZE: u16 = 1024;
static INCLUDE_SINGLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn set_include_single(value: bool) {
    INCLUDE_SINGLE.store(value, std::sync::atomic::Ordering::Relaxed);
}
pub fn include_single() -> bool {
    INCLUDE_SINGLE.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn analysis_lock() -> &'static Mutex<()> {
    static LOCK: Mutex<()> = Mutex::new(());
    &LOCK
}

/// No original-file read on cache hits and never full RAW development. The
/// thumbnail is already oriented, and its real pixel dimensions remain evidence.
pub fn thumbnail(
    db: &crate::db::Db,
    db_dir: &Path,
    asset_id: i64,
) -> Option<(image::RgbImage, &'static str)> {
    let (path, mtime): (String, String) =
        db.0.query_row(
            "SELECT path,mtime FROM assets WHERE id=?1",
            [asset_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok()?;
    let src = Path::new(&path);
    let cache = crate::thumbs::cached_for_asset(db_dir, src, ANALYSIS_SIZE, &mtime)
        .or_else(|| crate::thumbs::cached_without_source(db_dir, src, ANALYSIS_SIZE))
        .or_else(|| crate::thumbs::thumb_file(db_dir, src, ANALYSIS_SIZE))?;
    let decoded = image::ImageReader::open(cache).ok()?.decode().ok()?;
    Some((
        decoded
            .thumbnail(ANALYSIS_SIZE.into(), ANALYSIS_SIZE.into())
            .to_rgb8(),
        "thumbnail",
    ))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Region {
    pub kind: String,
    pub person: usize,
    pub side: Option<String>,
    pub bounds: [f64; 4],
    pub state: String,
    pub reason: Option<String>,
    pub raw_score: Option<f64>,
    pub auxiliary_ear: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub preprocessing: String,
    pub calibrated: bool,
    pub models: Vec<String>,
    pub parameters: serde_json::Value,
    pub reason: Option<String>,
    pub regions: Vec<Region>,
}

type Source = (Arc<image::RgbImage>, &'static str);
type CachedSource = (PathBuf, std::time::SystemTime, u64, u16, Source);
fn source_cache() -> &'static Mutex<Option<CachedSource>> {
    static CACHE: OnceLock<Mutex<Option<CachedSource>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}
/// One retained source, decoded serially. No unbounded high-resolution cache and
/// no browsing-thumbnail permits held while an ONNX session is running.
pub fn source(path: &Path, size: u16) -> Option<Source> {
    let metadata = std::fs::metadata(path).ok()?;
    let mtime = metadata.modified().ok()?;
    let mut cache = source_cache().lock().ok()?;
    if let Some((p, t, bytes, s, value)) = &*cache {
        if p == path && *t == mtime && *bytes == metadata.len() && *s == size {
            return Some(value.clone());
        }
    }
    // Drop the previous allocation before decoding another RAW.
    *cache = None;
    let (image, origin) = crate::thumbs::selection_source(path, size)?;
    let value = (Arc::new(image), origin);
    *cache = Some((path.to_owned(), mtime, metadata.len(), size, value.clone()));
    Some(value)
}

pub fn evidence(img: &image::RgbImage, origin: &str) -> Evidence {
    Evidence {
        source: origin.into(),
        width: img.width(),
        height: img.height(),
        preprocessing: if origin == "thumbnail" {
            PREPROCESS_VERSION
        } else {
            "high-resolution-evaluation-eye-bgr-127-255-v1"
        }
        .into(),
        ..Default::default()
    }
}

/// Rotate a high-resolution eye directly into model input; no padded pixels or
/// intermediate face upsampling. The evidence box encloses the rotated crop.
pub fn eye_crop(
    img: &image::RgbImage,
    points: &[[f32; 2]],
    margin: f32,
) -> Option<(image::RgbImage, [f64; 4])> {
    if points.len() != 6 || points.iter().flatten().any(|n| !n.is_finite()) {
        return None;
    }
    let mut dx = points[3][0] - points[0][0];
    let mut dy = points[3][1] - points[0][1];
    if dx < 0.0 {
        dx = -dx;
        dy = -dy;
    }
    let width = (dx * dx + dy * dy).sqrt();
    if width < 12.0 {
        return None;
    }
    let (c, s) = (dx / width, dy / width);
    let cx = points.iter().map(|p| p[0]).sum::<f32>() / 6.0;
    let cy = points.iter().map(|p| p[1]).sum::<f32>() / 6.0;
    let side = width * margin;
    let extent = side * (c.abs() + s.abs()) / 2.0;
    if cx - extent < 0.0
        || cy - extent < 0.0
        || cx + extent > img.width() as f32
        || cy + extent > img.height() as f32
    {
        return None;
    }
    let scale = 32.0 / side;
    let matrix = [
        [scale * c, scale * s, 16.0 - scale * (c * cx + s * cy)],
        [-scale * s, scale * c, 16.0 - scale * (-s * cx + c * cy)],
    ];
    let crop = super::face::warp_similarity_sized(img, &matrix, 32);
    let b = [
        (cx - extent) as f64 / img.width() as f64,
        (cy - extent) as f64 / img.height() as f64,
        (2.0 * extent) as f64 / img.width() as f64,
        (2.0 * extent) as f64 / img.height() as f64,
    ];
    Some((crop, b))
}

/// A textureless or almost entirely clipped eye crop contains no usable eyelid
/// evidence. This is an input quality gate, not a focus/occlusion classifier.
pub fn eye_has_detail(img: &image::RgbImage) -> bool {
    let gray = image::imageops::grayscale(img);
    let mut histogram = [0usize; 256];
    for pixel in gray.pixels() {
        histogram[pixel[0] as usize] += 1;
    }
    let count = gray.width() as usize * gray.height() as usize;
    if count == 0 {
        return false;
    }
    let percentile = |fraction: f64| {
        let mut sum = 0;
        for (value, n) in histogram.iter().enumerate() {
            sum += n;
            if sum as f64 >= count as f64 * fraction {
                return value;
            }
        }
        255
    };
    percentile(0.95).saturating_sub(percentile(0.05)) >= 16
}

fn slots() -> &'static Mutex<Option<ort::session::Session>> {
    static SESSION: OnceLock<Mutex<Option<ort::session::Session>>> = OnceLock::new();
    SESSION.get_or_init(|| Mutex::new(None))
}
pub fn release() {
    *slots().lock().expect("eye classifier mutex poisoned") = None;
    *source_cache()
        .lock()
        .expect("selection cache mutex poisoned") = None;
}

/// Original OMZ ONNX (not IR): mean/scale happen here, not inside the graph.
pub fn closed_score(manager: &super::ModelManager, crop: &image::RgbImage) -> Result<f64, String> {
    let crop = image::imageops::resize(crop, 32, 32, image::imageops::FilterType::Triangle);
    let mut data = Vec::with_capacity(3 * 32 * 32);
    for ch in [2, 1, 0] {
        for p in crop.pixels() {
            data.push((p[ch] as f32 - 127.0) / 255.0);
        }
    }
    let gpu = manager.ai_params().use_gpu;
    super::run_with_acceleration_fallback(
        gpu,
        EYE_MODEL,
        || {
            super::idle::touch();
            let mut slot = slots().lock().map_err(|e| e.to_string())?;
            if slot.is_none() {
                *slot = Some(super::build_session(
                    &manager.model_path(EYE_MODEL),
                    None,
                    gpu,
                    EYE_MODEL,
                )?);
            }
            let session = slot.as_mut().unwrap();
            let input = session
                .inputs()
                .first()
                .ok_or("eye classifier has no input")?
                .name()
                .to_owned();
            let tensor = ort::value::Tensor::from_array((vec![1i64, 3, 32, 32], data.clone()))
                .map_err(|e| e.to_string())?;
            let output = session
                .run(ort::inputs![input => tensor])
                .map_err(|e| e.to_string())?;
            let (shape, values) = output
                .get("19")
                .ok_or("eye classifier output missing")?
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if shape.as_ref() != [1, 2, 1, 1]
                || values.len() != 2
                || values
                    .iter()
                    .any(|s| !s.is_finite() || !(0.0..=1.0).contains(s))
                || (values[0] + values[1] - 1.0).abs() > 0.01
            {
                return Err("invalid eye softmax output".into());
            }
            Ok(values[1] as f64)
        },
        release,
    )
}

/// Strong geometric opening is usable after upstream localization/pixel/pose
/// gates. The infrared-trained candidate is supporting evidence, not a veto on
/// obvious opening in RGB photos. Closure still requires both model crops and EAR.
pub fn eye_is_clearly_open(ear: Option<f32>, maybe: f32) -> bool {
    ear.is_some_and(|value| value.is_finite() && value >= (maybe + 0.02).max(0.22))
}

pub fn eye_state(
    scores: [f64; 2],
    ear: Option<f32>,
    closed: f32,
    maybe: f32,
) -> (&'static str, Option<&'static str>) {
    let Some(ear) = ear.filter(|e| e.is_finite()) else {
        return ("unknown", Some("localization"));
    };
    if scores
        .iter()
        .any(|s| !s.is_finite() || !(0.0..=1.0).contains(s))
    {
        return ("unknown", Some("model_output"));
    }
    // A clear margin above the ambiguous band prevents the IR candidate from
    // marking visibly open, well-localized eyelids as possibly closed.
    if eye_is_clearly_open(Some(ear), maybe) {
        return ("open", None);
    }
    if (scores[0] - scores[1]).abs() > 0.2 {
        return ("unknown", Some("crop_disagreement"));
    }
    if scores.iter().all(|s| *s >= 0.9) && ear < closed {
        return ("closed", None);
    }
    if scores.iter().all(|s| *s <= 0.1) && ear >= maybe {
        return ("open", None);
    }
    if (scores.iter().all(|s| *s >= 0.9) && ear >= maybe)
        || (scores.iter().all(|s| *s <= 0.1) && ear < closed)
    {
        return ("maybe", Some("model_geometry_disagreement"));
    }
    ("maybe", Some("ambiguous"))
}

/// Only this native-pixel gate decides whether an eye was located. Input quality
/// never adds an abstention state after a detected eye has finite landmarks.
pub const MIN_EYE_DETECTION_PIXELS: f32 = 4.0;
pub fn detected_eye_bounds(img: &image::RgbImage, points: &[[f32; 2]]) -> Option<[f64; 4]> {
    if points.len() != 6 || points.iter().flatten().any(|p| !p.is_finite()) {
        return None;
    }
    if points.iter().any(|p| {
        p[0] < 0.0 || p[1] < 0.0 || p[0] >= img.width() as f32 || p[1] >= img.height() as f32
    }) {
        return None;
    }
    let width =
        ((points[3][0] - points[0][0]).powi(2) + (points[3][1] - points[0][1]).powi(2)).sqrt();
    if width < MIN_EYE_DETECTION_PIXELS {
        return None;
    }
    let x = points.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min);
    let y = points.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let right = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    Some([
        x as f64 / img.width() as f64,
        y as f64 / img.height() as f64,
        (right - x) as f64 / img.width() as f64,
        (bottom - y).max(1.0) as f64 / img.height() as f64,
    ])
}

pub fn binary_eye_state(ear: f32, closed_threshold: f32) -> &'static str {
    if ear < closed_threshold {
        "closed"
    } else {
        "open"
    }
}

/// Missing detections are separate from the binary states of detected eyes.
pub fn photo_eyes(regions: &[Region], include_single: bool) -> &'static str {
    if regions.is_empty() {
        return "no_face";
    }
    let mut detected = false;
    let mut single = false;
    let people: std::collections::BTreeSet<_> = regions.iter().map(|r| r.person).collect();
    for person in people {
        let eyes: Vec<_> = regions
            .iter()
            .filter(|r| {
                r.person == person && r.kind == "eye" && (r.state == "open" || r.state == "closed")
            })
            .collect();
        detected |= !eyes.is_empty();
        let closed = eyes.iter().filter(|r| r.state == "closed").count();
        if closed == 2 || (include_single && closed > 0) {
            return "closed";
        }
        single |= closed > 0;
    }
    if !detected {
        "no_eye"
    } else if single {
        "single_closed"
    } else {
        "open"
    }
}
