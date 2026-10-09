//! 编辑器临时预览会话；只读源文件，内存文档有数量/有效期限制，不写照片库存储。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use photocraft_engine::doc::Document;
use serde::Serialize;
use tauri::State;

use super::{photocraft, preview_renderer, recipe, source};
use crate::ipc::{run_blocking, SharedState};

const PREVIEW_EDGE: u32 = 1600;
const MAX_SESSIONS: usize = 4;
const IDLE_TTL: Duration = Duration::from_secs(30 * 60);

struct Preview {
    document: Document,
    raw: Option<Arc<source::RawSource>>,
    refined: Option<([f64; 3], Document)>,
    interactive_document: Document,
    interactive_renderer: Arc<Mutex<preview_renderer::Renderer>>,
    refined_renderer: Arc<Mutex<preview_renderer::Renderer>>,
    open_ms: f64,
    native_ready: bool,
    generation: Arc<Generation>,
    path: PathBuf,
    modified: Option<SystemTime>,
    length: u64,
    touched: Instant,
    source_checked: Instant,
}

#[derive(Default)]
struct Generation {
    revision: AtomicU64,
    closed: AtomicBool,
}

struct PreviewRequest {
    interactive: bool,
    revision: Option<u64>,
    generation: Arc<Generation>,
}

impl PreviewRequest {
    fn current(&self) -> bool {
        !self.generation.closed.load(Ordering::Acquire)
            && (self.interactive
                || self.revision.is_none_or(|revision| {
                    revision == self.generation.revision.load(Ordering::Acquire)
                }))
    }

    fn check(&self) -> Result<(), String> {
        if self.current() {
            Ok(())
        } else {
            Err("预览请求已被更新或关闭".into())
        }
    }
}

fn new_renderer(document: &Document) -> Arc<Mutex<preview_renderer::Renderer>> {
    Arc::new(Mutex::new(preview_renderer::Renderer::new(document)))
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
    native_ready: bool,
}

struct PreparedSource {
    document: Document,
    interactive_document: Document,
    raw: Option<Arc<source::RawSource>>,
    details: PreviewOpened,
}

fn prepare_source(path: &std::path::Path) -> Result<PreparedSource, String> {
    prepare_source_if_current(path, || true)
}

fn prepare_source_if_current(
    path: &std::path::Path,
    current: impl Fn() -> bool,
) -> Result<PreparedSource, String> {
    let source::Opened {
        document: full,
        raw,
        warnings,
    } = source::open_if_current(path, &current)?;
    let (width, height) = (full.size.width, full.size.height);
    let bit_depth = match full.depth {
        photocraft_engine::doc::SampleType::U8 => "8 bit",
        photocraft_engine::doc::SampleType::U16 => "16 bit",
        photocraft_engine::doc::SampleType::F32 => "32 bit",
    }
    .to_string();
    let sensor_raw = raw.is_some();
    if !current() {
        return Err("预览会话已关闭".into());
    }
    let document = photocraft::resize(&full, Some(PREVIEW_EDGE))?;
    drop(full);
    let interactive_document = document.clone();
    let source_url = jpeg_url(&document)?;
    let histogram = source_histogram(&interactive_document)?;
    let details = PreviewOpened {
        session_id: uuid::Uuid::new_v4().to_string(),
        source_url,
        width,
        height,
        sensor_raw,
        bit_depth,
        warnings,
        histogram,
        native_ready: true,
    };
    Ok(PreparedSource {
        document,
        interactive_document,
        raw,
        details,
    })
}

fn prepare_proxy(db_dir: &std::path::Path, asset: &crate::db::AssetRow) -> Option<PreparedSource> {
    let path = std::path::Path::new(&asset.path);
    let cache =
        crate::thumbs::cached_for_asset(db_dir, path, 2048, &asset.mtime).or_else(|| {
            crate::thumbs::thumb_file(
                db_dir,
                path,
                if asset.kind == crate::events::AssetKind::Raw {
                    3072
                } else {
                    2048
                },
            )
        })?;
    let pixels = image::open(cache)
        .ok()?
        .thumbnail(PREVIEW_EDGE, PREVIEW_EDGE)
        .to_rgb8();
    let image = photocraft_codecs::Image::from_u8(
        pixels.width(),
        pixels.height(),
        photocraft_codecs::ChannelLayout::Rgb,
        pixels.as_raw().clone(),
    )
    .ok()?
    .with_icc(Some(
        photocraft_cms::Builtin::Srgb.profile().to_bytes().to_vec(),
    ));
    let document = source::from_image("Preview", &image).ok()?;
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    let mut histogram = vec![vec![0u64; 256]; 4];
    for pixel in pixels.pixels() {
        for ch in 0..3 {
            histogram[ch][pixel[ch] as usize] += 1;
        }
        let lum =
            (u32::from(pixel[0]) * 54 + u32::from(pixel[1]) * 183 + u32::from(pixel[2]) * 19) / 256;
        histogram[3][lum as usize] += 1;
    }
    let (mut width, mut height) = (
        asset.width.unwrap_or(pixels.width()),
        asset.height.unwrap_or(pixels.height()),
    );
    if (width > height) != (pixels.width() > pixels.height()) {
        std::mem::swap(&mut width, &mut height);
    }
    let details = PreviewOpened {
        session_id: uuid::Uuid::new_v4().to_string(),
        source_url: format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ),
        width,
        height,
        sensor_raw: asset.kind == crate::events::AssetKind::Raw,
        bit_depth: "8 bit preview".into(),
        warnings: Vec::new(),
        histogram,
        native_ready: false,
    };
    Some(PreparedSource {
        interactive_document: document.clone(),
        document,
        raw: None,
        details,
    })
}

#[tauri::command]
pub async fn edit_preview_open(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
) -> Result<PreviewOpened, String> {
    run_blocking(state.inner().clone(), move |state| {
        let opened_at = Instant::now();
        let (_, asset) = super::project::resolve(state, &library_id, &asset_id)?;
        let path = PathBuf::from(&asset.path);
        let metadata = std::fs::metadata(&path).map_err(|e| format!("源文件不可用: {e}"))?;
        let PreparedSource {
            document,
            interactive_document,
            raw,
            details,
        } = prepare_proxy(&crate::ipc::app_database_dir(state)?, &asset)
            .map(Ok)
            .unwrap_or_else(|| prepare_source(&path))?;
        let mut cache = sessions().lock().map_err(|_| "预览会话锁不可用")?;
        cache.retain(|_, value| {
            value
                .lock()
                .map(|p| p.touched.elapsed() < IDLE_TTL)
                .unwrap_or(false)
        });
        // 不淘汰仍在编辑的会话；前端关闭编辑器时主动释放。
        admit_session(&cache, raw.as_deref())?;
        cache.insert(
            details.session_id.clone(),
            Arc::new(Mutex::new(Preview {
                interactive_renderer: new_renderer(&interactive_document),
                refined_renderer: new_renderer(&document),
                open_ms: opened_at.elapsed().as_secs_f64() * 1000.0,
                native_ready: details.native_ready,
                generation: Arc::new(Generation::default()),
                document,
                raw,
                refined: None,
                interactive_document,
                path,
                modified: metadata.modified().ok(),
                length: metadata.len(),
                touched: Instant::now(),
                source_checked: Instant::now(),
            })),
        );
        Ok(details)
    })
    .await
}

#[tauri::command]
pub async fn edit_preview_prepare(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<PreviewOpened, String> {
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览会话锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭")?;
        let (path, generation, modified, length) = {
            let p = preview.lock().map_err(|_| "预览会话锁不可用")?;
            if p.native_ready {
                return Err("原片已准备完成".into());
            }
            (p.path.clone(), p.generation.clone(), p.modified, p.length)
        };
        if generation.closed.load(Ordering::Acquire) {
            return Err("预览会话已关闭".into());
        }
        let PreparedSource {
            document,
            interactive_document,
            raw,
            mut details,
        } = prepare_source_if_current(&path, || !generation.closed.load(Ordering::Acquire))?;
        let metadata = std::fs::metadata(&path).map_err(|e| e.to_string())?;
        if metadata.modified().ok() != modified || metadata.len() != length {
            return Err("源文件已改变，请重新打开编辑器".into());
        }
        // Preserve the existing aggregate RAW sensor budget for staged sessions.
        if let Some(next) = &raw {
            let cache = sessions().lock().map_err(|_| "预览会话锁不可用")?;
            let bytes: usize = cache
                .values()
                .filter_map(|value| {
                    value
                        .lock()
                        .ok()
                        .and_then(|p| p.raw.as_ref().map(|raw| raw.bytes()))
                })
                .sum();
            if bytes.saturating_add(next.bytes()) > 512 * 1024 * 1024 {
                return Err("RAW 预览缓存预算不足，请关闭其他编辑会话".into());
            }
        }
        let mut p = preview.lock().map_err(|_| "预览会话锁不可用")?;
        if generation.closed.load(Ordering::Acquire) {
            return Err("预览会话已关闭".into());
        }
        p.interactive_renderer = new_renderer(&interactive_document);
        p.refined_renderer = new_renderer(&document);
        p.document = document;
        p.interactive_document = interactive_document;
        p.raw = raw;
        p.native_ready = true;
        details.session_id = session_id;
        Ok(details)
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
    request: &PreviewRequest,
) -> Result<(Document, recipe::EditRecipe), String> {
    let (mut base, raw, refined) = snapshot;
    let mut values = recipe.clone();
    let Some(raw) = raw.filter(|_| !request.interactive) else {
        return Ok((base, values));
    };
    let a = recipe.advanced.clone().unwrap_or_default();
    let tuning = [a.exposure, a.temperature, a.tint];
    if tuning != [0.0; 3] {
        base = match refined.filter(|(key, _)| *key == tuning) {
            Some((_, document)) => document,
            None => {
                let full = raw.develop_if_current(recipe, || request.current())?;
                request.check()?;
                let document = photocraft::resize(&full, Some(PREVIEW_EDGE))?;
                drop(full);
                request.check()?;
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

struct PreviewInput {
    snapshot: Snapshot,
    renderer: Arc<Mutex<preview_renderer::Renderer>>,
    request: PreviewRequest,
}

fn preview_input(
    preview: &Mutex<Preview>,
    interactive: bool,
    revision: Option<u64>,
) -> Result<PreviewInput, String> {
    let mut p = preview.lock().map_err(|_| "预览会话锁不可用")?;
    if p.touched.elapsed() >= IDLE_TTL {
        return Err("预览会话已过期，请重新打开编辑器".into());
    }
    // SMB metadata requests must not run on every slider frame. Export still
    // performs an unconditional source identity check before producing a file.
    if p.source_checked.elapsed() >= Duration::from_secs(1) {
        let meta = std::fs::metadata(&p.path).map_err(|e| format!("源文件不可用: {e}"))?;
        if meta.len() != p.length || meta.modified().ok() != p.modified {
            return Err("源文件已改变，请重新打开编辑器".into());
        }
        p.source_checked = Instant::now();
    }
    p.touched = Instant::now();
    let base = if interactive {
        p.interactive_document.clone()
    } else {
        p.document.clone()
    };
    let renderer = if interactive {
        p.interactive_renderer.clone()
    } else {
        p.refined_renderer.clone()
    };
    if let Some(revision) = revision {
        p.generation.revision.fetch_max(revision, Ordering::AcqRel);
    }
    let request = PreviewRequest {
        interactive,
        revision,
        generation: p.generation.clone(),
    };
    Ok(PreviewInput {
        snapshot: (
            base,
            if interactive { None } else { p.raw.clone() },
            if interactive { None } else { p.refined.clone() },
        ),
        renderer,
        request,
    })
}

#[tauri::command]
pub async fn edit_preview_render(
    state: State<'_, SharedState>,
    session_id: String,
    recipe: serde_json::Value,
    interactive: Option<bool>,
    revision: Option<u64>,
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
        let PreviewInput {
            snapshot,
            renderer,
            request,
        } = preview_input(&preview, interactive.unwrap_or(false), revision)?;
        request.check()?;
        let refining = Instant::now();
        let (base, mut values) = refine_source(&preview, snapshot, &recipe, &request)?;
        let raw_ms = refining.elapsed().as_secs_f64() * 1000.0;
        // 输出偏好不改变固定代理预览；不使它们失效整个渲染缓存。
        values.output = Default::default();
        request.check()?;
        let mut renderer = renderer.lock().map_err(|_| "预览渲染锁不可用")?;
        request.check()?;
        let jpeg = renderer.render(&base, &values, 90)?;
        renderer.timings.raw_ms = raw_ms;
        renderer.timings.total_ms += raw_ms;
        request.check()?;
        Ok(tauri::ipc::Response::new(jpeg))
    })
    .await
}

#[tauri::command]
pub fn edit_preview_close(session_id: String) -> Result<(), String> {
    let preview = sessions()
        .lock()
        .map_err(|_| "预览会话锁不可用")?
        .remove(&session_id);
    if let Some(preview) = preview {
        preview
            .lock()
            .map_err(|_| "预览会话锁不可用")?
            .generation
            .closed
            .store(true, Ordering::Release);
    }
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStats {
    open_ms: f64,
    interactive: preview_renderer::Timings,
    refined: preview_renderer::Timings,
}

/// 按需查询诊断数据；不为每一帧增加额外 IPC，且不暴露源路径。
#[tauri::command]
pub async fn edit_preview_stats(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<PreviewStats, String> {
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭")?;
        let (open_ms, interactive, refined) = {
            let p = preview.lock().map_err(|_| "预览锁不可用")?;
            (
                p.open_ms,
                p.interactive_renderer.clone(),
                p.refined_renderer.clone(),
            )
        };
        let interactive = interactive
            .lock()
            .map_err(|_| "渲染锁不可用")?
            .timings
            .clone();
        let refined = refined.lock().map_err(|_| "渲染锁不可用")?.timings.clone();
        Ok(PreviewStats {
            open_ms,
            interactive,
            refined,
        })
    })
    .await
}

/// CPU metadata snapshot for the direct canvas. Image pixels remain in stable
/// native tiles; the compositor uploads them once and keeps them on the GPU.
pub(super) fn canvas_source(session_id:&str)->Result<Document,String> {
    let value=sessions().lock().map_err(|_|"预览锁不可用")?.get(session_id).cloned().ok_or("预览会话已关闭")?;
    let mut p=value.lock().map_err(|_|"预览锁不可用")?;
    if p.generation.closed.load(Ordering::Acquire) {return Err("预览会话已关闭".into());}
    p.touched=Instant::now();Ok(p.document.clone())
}

#[cfg(test)]
mod startup_tests {
    use super::*;

    #[test]
    fn cached_proxy_opens_without_decoding_the_original_and_keeps_one_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("original.jpg");
        image::RgbImage::from_pixel(2400, 1600, image::Rgb([80, 120, 160]))
            .save(&path)
            .unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let modified: chrono::DateTime<chrono::Utc> = metadata.modified().unwrap().into();
        let asset: crate::db::AssetRow = serde_json::from_value(serde_json::json!({
            "path":path.to_string_lossy(),"filename":"original.jpg","size":metadata.len(),
            "mtime":modified.to_rfc3339(),"xxhash":0,"kind":"photo","source":"test",
            "createdAt":modified.to_rfc3339(),"width":2400,"height":1600
        }))
        .unwrap();
        crate::thumbs::thumb_file(dir.path(), &path, 2048).unwrap();
        // Proves startup consumes the cached pixels, not the original file.
        std::fs::remove_file(path).unwrap();
        let prepared = prepare_proxy(dir.path(), &asset).unwrap();
        assert!(!prepared.details.native_ready);
        assert_eq!(prepared.details.width, 2400);
        assert_eq!(prepared.details.height, 1600);
        assert_eq!(prepared.document.size, prepared.interactive_document.size);
        assert_eq!(
            prepared
                .document
                .size
                .width
                .max(prepared.document.size.height),
            PREVIEW_EDGE
        );
        assert!(prepared.raw.is_none());
        assert!(prepared
            .details
            .source_url
            .starts_with("data:image/jpeg;base64,"));
    }
}
