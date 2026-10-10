//! Evaluation adapter for a locally exported defocus model. The author has not
//! published a redistribution license for D-DFFNet weights, so this module does
//! not add those weights to the application download catalog.
use super::selection_regions::{Evidence, Region};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    path::PathBuf,
    sync::{Mutex, OnceLock},
};

// 评估适配器（D-DFFNet 权重无再分发许可，不入下载目录）——保留待重评估。
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    model_version: String,
    sha256: String,
    /// Export wrapper explicitly converts the original positive class to blur.
    contract: String,
}
type Slot = Option<(PathBuf, String, ort::session::Session)>;
fn slots() -> &'static Mutex<Slot> {
    static SLOTS: OnceLock<Mutex<Slot>> = OnceLock::new();
    SLOTS.get_or_init(|| Mutex::new(None))
}
pub fn release() {
    *slots().lock().expect("defocus slot poisoned") = None;
}

/// Evaluation only. Returns uncalibrated regional evidence, never changes a
/// production verdict or user rating. RGB, ImageNet normalization, 320 square;
/// ONNX wrapper outputs sigmoid blur map [1,1,320,320] named `blur_map`.
#[allow(dead_code)] // 评估适配器（见 Manifest 注释）；重评估接入后移除。
pub fn evaluate(
    manager: &super::ModelManager,
    img: &image::RgbImage,
    source: &str,
    subject_boxes: &[[f64; 4]],
) -> Result<Evidence, String> {
    let path = manager.model_path("defocus-candidate");
    let manifest: Manifest = serde_json::from_slice(
        &std::fs::read(path.with_extension("json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if manifest.schema_version != 1
        || manifest.contract != "rgb-imagenet-320-sigmoid-blur-v1"
        || manifest.model_version.is_empty()
    {
        return Err("unsupported defocus export contract".into());
    }
    let crop = image::imageops::resize(img, 320, 320, image::imageops::FilterType::Triangle);
    let mut data = Vec::with_capacity(3 * 320 * 320);
    for ch in 0..3 {
        for p in crop.pixels() {
            data.push(
                (p[ch] as f32 / 255.0 - [0.485, 0.456, 0.406][ch]) / [0.229, 0.224, 0.225][ch],
            );
        }
    }
    let gpu = manager.ai_params().use_gpu;
    let map = super::run_with_acceleration_fallback(
        gpu,
        "defocus-candidate",
        || {
            super::idle::touch();
            let mut slot = slots().lock().map_err(|e| e.to_string())?;
            if slot
                .as_ref()
                .is_none_or(|(p, hash, _)| p != &path || hash != &manifest.sha256)
            {
                let actual = format!(
                    "{:x}",
                    Sha256::digest(std::fs::read(&path).map_err(|e| e.to_string())?)
                );
                if actual != manifest.sha256 {
                    return Err("defocus candidate SHA256 mismatch".into());
                }
                *slot = Some((
                    path.clone(),
                    actual,
                    super::build_session(&path, None, gpu, "defocus-candidate")?,
                ));
            }
            let session = &mut slot.as_mut().unwrap().2;
            let tensor = ort::value::Tensor::from_array((vec![1i64, 3, 320, 320], data.clone()))
                .map_err(|e| e.to_string())?;
            let input = session
                .inputs()
                .first()
                .ok_or("defocus model has no input")?
                .name()
                .to_string();
            let output = session
                .run(ort::inputs![input => tensor])
                .map_err(|e| e.to_string())?;
            let (shape, values) = output
                .get("blur_map")
                .ok_or("defocus output missing")?
                .try_extract_tensor::<f32>()
                .map_err(|e| e.to_string())?;
            if shape.as_ref() != [1, 1, 320, 320]
                || values
                    .iter()
                    .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            {
                return Err("invalid defocus map".into());
            }
            Ok(values.to_vec())
        },
        release,
    )?;
    let mut result = super::selection_regions::evidence(img, source);
    result.models = vec![format!(
        "defocus-candidate:{}:{}",
        manifest.model_version, manifest.sha256
    )];
    result.parameters =
        serde_json::json!({"parameterVersion":1,"inputSize":320,"minimumSubjectPixels":96});
    result.preprocessing = format!("{}:{}", manifest.contract, manifest.model_version);
    result.reason = Some(
        if subject_boxes.is_empty() {
            "subject_unknown"
        } else {
            "focus_model_unvalidated"
        }
        .into(),
    );
    let mut subject_mask = vec![false; 320 * 320];
    for (index, bounds) in subject_boxes.iter().enumerate() {
        if bounds
            .iter()
            .any(|n| !n.is_finite() || !(0.0..=1.0).contains(n))
        {
            return Err("invalid subject box".into());
        }
        let [x, y, w, h] = *bounds;
        let x0 = (x * 320.0) as usize;
        let y0 = (y * 320.0) as usize;
        let x1 = ((x + w) * 320.0).ceil().min(320.0) as usize;
        let y1 = ((y + h) * 320.0).ceil().min(320.0) as usize;
        let mut local = Vec::new();
        for y in y0..y1 {
            for x in x0..x1 {
                local.push(map[y * 320 + x]);
                subject_mask[y * 320 + x] = true;
            }
        }
        let usable = w * img.width() as f64 >= 96.0 && h * img.height() as f64 >= 96.0;
        result.regions.push(Region {
            kind: "subject".into(),
            person: index + 1,
            side: None,
            bounds: *bounds,
            state: "unknown".into(),
            reason: Some(
                if usable {
                    "focus_model_unvalidated"
                } else {
                    "small_face"
                }
                .into(),
            ),
            raw_score: (!local.is_empty() && usable)
                .then(|| local.iter().map(|n| *n as f64).sum::<f64>() / local.len() as f64),
            auxiliary_ear: None,
        });
    }
    if !subject_boxes.is_empty() {
        let background: Vec<_> = map
            .iter()
            .enumerate()
            .filter(|(i, _)| !subject_mask[*i])
            .map(|(_, n)| *n)
            .collect();
        result.regions.push(Region {
            kind: "background".into(),
            person: 0,
            side: None,
            bounds: [0.0, 0.0, 1.0, 1.0],
            state: "unknown".into(),
            reason: Some("focus_model_unvalidated".into()),
            raw_score: (!background.is_empty()).then(|| {
                background.iter().map(|n| *n as f64).sum::<f64>() / background.len() as f64
            }),
            auxiliary_ear: None,
        });
    }
    Ok(result)
}
