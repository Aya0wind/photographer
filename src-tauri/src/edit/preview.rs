//! 编辑器临时预览会话；只读源文件，内存文档有数量/有效期限制，不写照片库存储。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use photocraft_engine::doc::Document;
use serde::Serialize;
use tauri::State;

use super::{photocraft, recipe, source};
use crate::ipc::{run_blocking, SharedState};

const PREVIEW_EDGE: u32 = 1600;
const MAX_SESSIONS: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);

struct Preview {
    document: Document,
    raw: Option<Arc<source::RawSource>>,
    refined: Option<([f64; 3], Document)>,
    interactive_document: Document,
    path: PathBuf,
    modified: Option<SystemTime>,
    length: u64,
    touched: Instant,
}

fn sessions() -> &'static Mutex<HashMap<String, Arc<Mutex<Preview>>>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Arc<Mutex<Preview>>>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn validate_source(
    session_id: &str,
    expected_path: &std::path::Path,
) -> Result<(), String> {
    let preview = sessions()
        .lock()
        .map_err(|_| "预览会话锁不可用")?
        .get(session_id)
        .cloned()
        .ok_or("预览会话已关闭，请重新准备照片")?;
    let p = preview.lock().map_err(|_| "预览会话锁不可用")?;
    let meta = std::fs::metadata(expected_path).map_err(|e| format!("源照片不可用: {e}"))?;
    if p.path != expected_path || p.length != meta.len() || p.modified != meta.modified().ok() {
        return Err("源照片已改变，请重新准备预览后再生成成片".into());
    }
    Ok(())
}

fn jpeg_url(document: &Document) -> Result<String, String> {
    Ok(format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(photocraft::jpeg(document, 90)?)
    ))
}

fn source_histogram(document: &Document) -> Result<Vec<Vec<u64>>, String> {
    let pixels = photocraft_compose::flatten(document);
    let histogram = photocraft_algo::histogram::RgbHistogram::from_rgba(&pixels.px);
    let mut histogram: Vec<Vec<u64>> = histogram
        .channels
        .iter()
        .map(|bins| bins.to_vec())
        .collect();
    let gray = photocraft_io::document_to_image(document, &mut Vec::new())
        .map_err(|e| e.to_string())?
        .convert(
            photocraft_codecs::ChannelLayout::GrayA,
            photocraft_codecs::SampleType::F32,
        );
    let gray_doc = source::from_image("Histogram", &gray)?;
    let gray_pixels = photocraft_compose::flatten(&gray_doc);
    histogram.push(
        photocraft_algo::histogram::RgbHistogram::from_rgba(&gray_pixels.px).channels[0].to_vec(),
    );
    Ok(histogram)
}

fn admit_session(
    cache: &HashMap<String, Arc<Mutex<Preview>>>,
    raw: Option<&source::RawSource>,
) -> Result<(), String> {
    let sensor_bytes: usize = cache
        .values()
        .filter_map(|p| {
            p.lock()
                .ok()
                .and_then(|p| p.raw.as_ref().map(|raw| raw.bytes()))
        })
        .sum();
    let next_sensor_bytes = raw.map_or(0, |raw| raw.bytes());
    if sensor_bytes.saturating_add(next_sensor_bytes) > 512 * 1024 * 1024 {
        return Err("RAW 预览缓存预算不足，请关闭其他编辑会话".into());
    }
    if cache.len() >= MAX_SESSIONS {
        return Err("同时打开的编辑预览过多，请先关闭其他编辑器".into());
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpened {
    session_id: String,
    source_url: String,
    width: u32,
    height: u32,
    sensor_raw: bool,
    bit_depth: String,
    warnings: Vec<String>,
    histogram: Vec<Vec<u64>>,
}

#[tauri::command]
pub async fn edit_preview_open(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
) -> Result<PreviewOpened, String> {
    run_blocking(state.inner().clone(), move |state| {
        let (_, asset) = super::project::resolve(state, &library_id, &asset_id)?;
        let path = PathBuf::from(asset.path);
        let metadata = std::fs::metadata(&path).map_err(|e| format!("源文件不可用: {e}"))?;
        let opened = source::open(&path)?;
        let (width, height) = (opened.document.size.width, opened.document.size.height);
        let bit_depth = match opened.document.depth {
            photocraft_engine::doc::SampleType::U8 => "8 bit",
            photocraft_engine::doc::SampleType::U16 => "16 bit",
            photocraft_engine::doc::SampleType::F32 => "32 bit",
        }
        .to_string();
        let sensor_raw = opened.raw.is_some();
        let warnings = opened.warnings;
        let document = photocraft::resize(&opened.document, Some(PREVIEW_EDGE))?;
        let interactive_document = photocraft::resize(&document, Some(800))?;
        let source_url = jpeg_url(&document)?;
        let histogram = source_histogram(&interactive_document)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let mut cache = sessions().lock().map_err(|_| "预览会话锁不可用")?;
        cache.retain(|_, value| {
            value
                .lock()
                .map(|p| p.touched.elapsed() < IDLE_TTL)
                .unwrap_or(false)
        });
        // 不淘汰仍在编辑的会话；前端关闭编辑器时主动释放。
        admit_session(&cache, opened.raw.as_deref())?;
        cache.insert(
            session_id.clone(),
            Arc::new(Mutex::new(Preview {
                document,
                raw: opened.raw,
                refined: None,
                interactive_document,
                path,
                modified: metadata.modified().ok(),
                length: metadata.len(),
                touched: Instant::now(),
            })),
        );
        Ok(PreviewOpened {
            session_id,
            source_url,
            width,
            height,
            sensor_raw,
            bit_depth,
            warnings,
            histogram,
        })
    })
    .await
}

type Snapshot = (
    Document,
    Option<Arc<source::RawSource>>,
    Option<([f64; 3], Document)>,
);

fn refine_source(
    preview: &Mutex<Preview>,
    snapshot: Snapshot,
    recipe: &recipe::EditRecipe,
    interactive: bool,
) -> Result<(Document, recipe::EditRecipe), String> {
    let (mut base, raw, refined) = snapshot;
    let mut values = recipe.clone();
    let Some(raw) = raw.filter(|_| !interactive) else {
        return Ok((base, values));
    };
    let a = recipe.advanced.clone().unwrap_or_default();
    let tuning = [a.exposure, a.temperature, a.tint];
    if tuning != [0.0; 3] {
        base = match refined.filter(|(key, _)| *key == tuning) {
            Some((_, document)) => document,
            None => {
                let document = photocraft::resize(&raw.develop(recipe)?, Some(PREVIEW_EDGE))?;
                preview.lock().map_err(|_| "预览锁不可用")?.refined =
                    Some((tuning, document.clone()));
                document
            }
        };
    }
    if let Some(a) = &mut values.advanced {
        a.exposure = 0.0;
        a.temperature = 0.0;
        a.tint = 0.0;
    }
    Ok((base, values))
}

#[tauri::command]
pub async fn edit_preview_render(
    state: State<'_, SharedState>,
    session_id: String,
    recipe: serde_json::Value,
    interactive: Option<bool>,
) -> Result<tauri::ipc::Response, String> {
    // 在进入后台前只校验 JSON；源图和合成工作都在阻塞线程。
    let recipe = recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览会话锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭或过期，请重新打开编辑器")?;
        let base = {
            let mut p = preview.lock().map_err(|_| "预览会话锁不可用")?;
            if p.touched.elapsed() >= IDLE_TTL {
                return Err("预览会话已过期，请重新打开编辑器".into());
            }
            let meta = std::fs::metadata(&p.path).map_err(|e| format!("源文件不可用: {e}"))?;
            if meta.len() != p.length || meta.modified().ok() != p.modified {
                return Err("源文件已改变，请重新打开编辑器".into());
            }
            p.touched = Instant::now();
            (
                if interactive.unwrap_or(false) {
                    p.interactive_document.clone()
                } else {
                    p.document.clone()
                },
                p.raw.clone(),
                p.refined.clone(),
            )
        };
        let (base, values) = refine_source(&preview, base, &recipe, interactive.unwrap_or(false))?;
        let doc = photocraft::render_document(&base, &values)?;
        photocraft::jpeg(&doc, if interactive.unwrap_or(false) { 80 } else { 90 })
            .map(tauri::ipc::Response::new)
    })
    .await
}

#[tauri::command]
pub fn edit_preview_close(session_id: String) -> Result<(), String> {
    sessions()
        .lock()
        .map_err(|_| "预览会话锁不可用")?
        .remove(&session_id);
    Ok(())
}

/// 原生高位深文档取色和曲线黑/灰/白点；UI 只接收参数，不处理像素。
#[tauri::command]
pub async fn edit_preview_pick(
    state: State<'_, SharedState>,
    session_id: String,
    recipe: serde_json::Value,
    x: f64,
    y: f64,
    picker: String,
) -> Result<serde_json::Value, String> {
    let recipe = recipe::parse_recipe(&recipe)?;
    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(&x) || !(0.0..=1.0).contains(&y) {
        return Err("取色坐标无效".into());
    }
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭")?;
        let base = preview.lock().map_err(|_| "预览锁不可用")?.document.clone();
        let a = recipe.advanced.clone().unwrap_or_default();
        let mut before = recipe.clone();
        if let Some(a) = &mut before.advanced {
            a.curves.clear();
            a.channel_curves = Default::default();
        }
        let doc = photocraft::adjusted_document(&base, &before)?;
        let px = (x * f64::from(doc.size.width))
            .floor()
            .min(f64::from(doc.size.width - 1)) as i32;
        let py = (y * f64::from(doc.size.height))
            .floor()
            .min(f64::from(doc.size.height - 1)) as i32;
        let area = photocraft_engine::doc::Rect::from_xywh(px, py, 1, 1);
        let sample = photocraft_compose::render(&doc, area)
            .px
            .first()
            .copied()
            .unwrap_or_default();
        if sample[3] <= 0.0 {
            return Err("不能对透明像素取色".into());
        }
        if picker == "point" {
            return Ok(
                serde_json::json!({"sample": [sample[0]*255.0, sample[1]*255.0, sample[2]*255.0]}),
            );
        }
        let mut session = photocraft_engine::Session::new();
        session.add_document(doc, None);
        let mut params = serde_json::json!({"points": a.curves, "red": a.channel_curves.red,
            "green": a.channel_curves.green, "blue": a.channel_curves.blue,
            "eyedropper": {"point": picker, "at": [px, py]}});
        for key in ["points", "red", "green", "blue"] {
            if params[key].as_array().is_some_and(|v| v.is_empty()) {
                params.as_object_mut().unwrap().remove(key);
            }
        }
        let adjustment =
            photocraft_engine::adjust_params::curves_eyedropper_from_params(&session, &params)
                .map_err(|e| e.to_string())?
                .ok_or("取色参数无效")?;
        Ok(photocraft_engine::adjust_params::to_params(&adjustment))
    })
    .await
}
