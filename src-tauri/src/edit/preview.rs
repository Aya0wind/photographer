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
    original_size:[u32;2],
    document: Document,
    raw: Option<Arc<source::RawSource>>,
    refined: Option<([f64; 3], Document)>,
    interactive_document: Document,
    interactive_renderer: Arc<Mutex<preview_renderer::Renderer>>,
    refined_renderer: Arc<Mutex<preview_renderer::Renderer>>,
    open_ms: f64,
    native_ready: bool,
    pipeline_ready: bool,
    pending_rawlab:Option<PreparedSource>,
    sensor_raw: bool,
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

fn new_renderer(document: &Document,original:[u32;2]) -> Arc<Mutex<preview_renderer::Renderer>> {
    Arc::new(Mutex::new(preview_renderer::Renderer::new(document).with_source_size(original,document)))
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
    histogram_pixels(&pixels)
}
fn histogram_pixels(pixels:&photocraft_compose::Buffer)->Result<Vec<Vec<u64>>,String>{
    let histogram = photocraft_algo::histogram::RgbHistogram::from_rgba(&pixels.px);
    let mut histogram: Vec<Vec<u64>> = histogram
        .channels
        .iter()
        .map(|bins| bins.to_vec())
        .collect();
    let values:Vec<f32>=pixels.px.iter().flat_map(|p|if p.iter().all(|v|v.is_finite()){*p}else{[0.0;4]}).collect();
    let gray = photocraft_codecs::Image::from_f32(pixels.rect.width(),pixels.rect.height(),photocraft_codecs::ChannelLayout::Rgba,&values).map_err(|e|e.to_string())?
        .convert(
            photocraft_codecs::ChannelLayout::GrayA,
            photocraft_codecs::SampleType::F32,
        );
    let gray_pixels:Vec<[f32;4]>=gray.data().chunks_exact(8).map(|p|{let value=f32::from_ne_bytes(p[..4].try_into().unwrap());let alpha=f32::from_ne_bytes(p[4..].try_into().unwrap());[value,value,value,alpha]}).collect();
    histogram.push(
        photocraft_algo::histogram::RgbHistogram::from_rgba(&gray_pixels).channels[0].to_vec(),
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

#[derive(Clone,Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewOpened {
    #[serde(skip_serializing_if="Option::is_none")]
    preview_url:Option<String>,
    session_id: String,
    source_url: String,
    width: u32,
    height: u32,
    sensor_raw: bool,
    bit_depth: String,
    warnings: Vec<String>,
    histogram: Vec<Vec<u64>>,
    native_ready: bool,
    rawlab_ready:bool,
}

#[derive(Clone)]
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
        original_size: [width,height],
        document: full,
        raw,
        warnings,
    } = source::open_preview_if_current(path, PREVIEW_EDGE, &current)?;
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
        preview_url:None,
        session_id: uuid::Uuid::new_v4().to_string(),
        source_url,
        width,
        height,
        sensor_raw,
        bit_depth,
        warnings,
        histogram,
        native_ready: true,
        rawlab_ready:false,
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
    let (pixels,jpeg)=super::proxy_cache::get_or_create(db_dir,asset,PREVIEW_EDGE,|| {
        let cache=crate::thumbs::cached(db_dir,path,2048)
            .or_else(||if !path.is_file() {crate::thumbs::cached_for_asset(db_dir,path,2048,&asset.mtime)} else {None})
            .or_else(||crate::thumbs::thumb_file(db_dir,path,if asset.kind==crate::events::AssetKind::Raw {3072} else {2048}))
            .or_else(||crate::thumbs::thumb_file(db_dir,path,2048))?;
        crate::thumbs::jpeg_scaled(std::path::Path::new(&cache),PREVIEW_EDGE as u16)
            .or_else(||image::open(&cache).ok().map(|img|img.thumbnail(PREVIEW_EDGE,PREVIEW_EDGE).to_rgb8()))
    })?;
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
        preview_url:None,
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
        rawlab_ready:false,
    };
    Some(PreparedSource {
        interactive_document: document.clone(),
        document,
        raw: None,
        details,
    })
}

fn prepare_rawlab_fallback(path:&std::path::Path)->Result<PreparedSource,String>{
    let source=super::rawlab::open(path,Some(PREVIEW_EDGE),&AtomicBool::new(false))?;
    let neutral=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","colorPipeline":{"version":1,"backend":"rawlab","coreRevision":super::rawlab::core_revision()}}))?;
    let (display,_)=super::rawlab::display(&source.document,&neutral)?;
    let details=PreviewOpened{preview_url:None,session_id:uuid::Uuid::new_v4().to_string(),source_url:jpeg_url(&display)?,width:source.original_size[0],height:source.original_size[1],sensor_raw:true,bit_depth:"32 bit linear proxy".into(),warnings:source.warnings,histogram:source_histogram(&display)?,native_ready:true,rawlab_ready:true};
    Ok(PreparedSource{interactive_document:source.document.clone(),document:source.document,raw:None,details})
}

#[tauri::command]
pub async fn edit_preview_open(
    state: State<'_, SharedState>,
    library_id: String,
    asset_id: String,
    rawlab:Option<bool>,
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
            .unwrap_or_else(||if rawlab.unwrap_or(false)&&asset.kind==crate::events::AssetKind::Raw {prepare_rawlab_fallback(&path)}else{prepare_source(&path)})?;
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
                original_size:[details.width,details.height],
                interactive_renderer: new_renderer(&interactive_document,[details.width,details.height]),
                refined_renderer: new_renderer(&document,[details.width,details.height]),
                open_ms: opened_at.elapsed().as_secs_f64() * 1000.0,
                native_ready: details.native_ready,
                pipeline_ready:details.rawlab_ready,
                pending_rawlab:None,
                sensor_raw:details.sensor_raw,
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
    recipe:Option<serde_json::Value>,
) -> Result<PreviewOpened, String> {
    let values=recipe.as_ref().map(recipe::parse_recipe).transpose()?;
    run_blocking(state.inner().clone(), move |_| {
        let preview = sessions()
            .lock()
            .map_err(|_| "预览会话锁不可用")?
            .get(&session_id)
            .cloned()
            .ok_or("预览会话已关闭")?;
        let (path, generation, modified, length, sensor_raw) = {
            let p = preview.lock().map_err(|_| "预览会话锁不可用")?;
            if if values.as_ref().is_some_and(|r|r.color_pipeline.is_some()){p.pipeline_ready}else{p.native_ready} {
                return Err("原片已准备完成".into());
            }
            (p.path.clone(), p.generation.clone(), p.modified, p.length,p.sensor_raw)
        };
        if generation.closed.load(Ordering::Acquire) {
            return Err("预览会话已关闭".into());
        }
        let PreparedSource {
            document,
            interactive_document,
            raw,
            mut details,
        } = if values.as_ref().is_some_and(|r|r.color_pipeline.is_some()) {
            let source=if sensor_raw{super::rawlab::open(&path,Some(PREVIEW_EDGE),&generation.closed)?}else{
                let p=preview.lock().map_err(|_|"预览会话锁不可用")?;
                source::Opened{original_size:p.original_size,document:p.document.clone(),raw:None,warnings:vec![]}
            };
            let mut neutral=values.clone().ok_or("色彩配方缺失")?;neutral.advanced=None;neutral.adjustments=None;neutral.legacy_adjustments=None;neutral.masks.clear();neutral.crop=None;neutral.rotate_quarter=0;neutral.geometry=None;neutral.text_layers.clear();neutral.brush_strokes.clear();
            if let Some(p)=neutral.color_pipeline.as_mut(){p.film=None;}
            let (display,_)=super::rawlab::display(&source.document,&neutral)?;
            PreparedSource {interactive_document:source.document.clone(),document:source.document,raw:None,details:PreviewOpened {
                preview_url:None,session_id:session_id.clone(),source_url:jpeg_url(&display)?,width:source.original_size[0],height:source.original_size[1],sensor_raw,bit_depth:if sensor_raw{"32 bit linear proxy".into()}else{"RGB preview".into()},warnings:source.warnings,histogram:source_histogram(&display)?,native_ready:true,rawlab_ready:true,
            }}
        } else {prepare_source_if_current(&path, || !generation.closed.load(Ordering::Acquire))?};
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
        if values.as_ref().is_some_and(|r|r.color_pipeline.is_some()){
            let mut p=preview.lock().map_err(|_|"预览会话锁不可用")?;
            if generation.closed.load(Ordering::Acquire){return Err("预览会话已关闭".into());}
            details.session_id=session_id;
            p.pending_rawlab=Some(PreparedSource{document,interactive_document,raw,details:details.clone()});
            return Ok(details);
        }
        if let Some(values)=&values {
            // Produce a complete candidate frame before replacing the public source.
            let mut candidate=preview_renderer::Renderer::new(&document).with_source_size([details.width,details.height],&document);
            let jpeg=candidate.render(&document,values,90)?;
            details.preview_url=Some(format!("data:image/jpeg;base64,{}",base64::engine::general_purpose::STANDARD.encode(jpeg)));
        }
        let mut p = preview.lock().map_err(|_| "预览会话锁不可用")?;
        if generation.closed.load(Ordering::Acquire) {
            return Err("预览会话已关闭".into());
        }
        p.original_size=[details.width,details.height];
        p.interactive_renderer = new_renderer(&interactive_document,p.original_size);
        p.refined_renderer = new_renderer(&document,p.original_size);
        p.document = document;
        p.interactive_document = interactive_document;
        p.raw = raw;
        p.native_ready = true;
        p.pipeline_ready=values.as_ref().is_some_and(|r|r.color_pipeline.is_some());
        details.session_id = session_id;
        Ok(details)
    })
    .await
}

/// Activate only after development; render with the recipe supplied at this point.
#[tauri::command]
pub async fn edit_preview_activate(state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value)->Result<PreviewOpened,String>{
    let values=recipe::parse_recipe(&recipe)?;
    if values.color_pipeline.is_none(){return Err("未选择胶片显影".into());}
    run_blocking(state.inner().clone(),move |_|{
        let preview=sessions().lock().map_err(|_|"预览会话锁不可用")?.get(&session_id).cloned().ok_or("预览会话已关闭")?;
        let (staged,generation,path,modified,length)={let p=preview.lock().map_err(|_|"预览会话锁不可用")?;
            (p.pending_rawlab.clone().ok_or("线性代理尚未准备完成")?,p.generation.clone(),p.path.clone(),p.modified,p.length)};
        if generation.closed.load(Ordering::Acquire){return Err("预览会话已关闭".into());}
        let mut renderer=preview_renderer::Renderer::new(&staged.document).with_source_size([staged.details.width,staged.details.height],&staged.document);
        let frame=renderer.render(&staged.document,&values,90)?;
        let meta=std::fs::metadata(path).map_err(|e|e.to_string())?;
        if meta.len()!=length||meta.modified().ok()!=modified{return Err("源文件已改变，请重新打开编辑器".into());}
        let mut p=preview.lock().map_err(|_|"预览会话锁不可用")?;
        if generation.closed.load(Ordering::Acquire)||p.pending_rawlab.as_ref().is_none_or(|s|s.document.id!=staged.document.id){return Err("代理准备已被更新或关闭".into());}
        let staged=p.pending_rawlab.take().ok_or("代理准备已被更新")?;
        let mut details=staged.details;
        details.preview_url=Some(format!("data:image/jpeg;base64,{}",base64::engine::general_purpose::STANDARD.encode(frame)));
        p.original_size=[details.width,details.height];p.document=staged.document;p.interactive_document=staged.interactive_document;p.raw=staged.raw;p.refined=None;
        let renderer=Arc::new(Mutex::new(renderer));p.interactive_renderer=renderer.clone();p.refined_renderer=renderer;p.native_ready=true;p.pipeline_ready=true;p.touched=Instant::now();
        Ok(details)
    }).await
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
    if recipe.color_pipeline.is_some(){return Ok((base,values));}
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
    let mut cache = sessions().lock().map_err(|_| "预览会话锁不可用")?;
    let preview = cache.remove(&session_id);
    if cache.is_empty() { super::gpu::release_cached_documents(); }
    drop(cache);
    super::native_canvas::edit_native_close(session_id);
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

/// Recovery for dropped windows / failed close IPC. Never wait on a rendering session.
pub fn release_expired() {
    if let Ok(mut cache) = sessions().try_lock() {
        let mut expired = Vec::new();
        cache.retain(|id, session| {
            if let Ok(preview) = session.try_lock() {
                if preview.touched.elapsed() >= IDLE_TTL {
                    preview.generation.closed.store(true, Ordering::Release);
                    expired.push(id.clone());
                    return false;
                }
            }
            true
        });
        if cache.is_empty() { super::gpu::release_cached_documents(); }
        drop(cache);
        for id in expired { super::native_canvas::edit_native_close(id); }
    }
}

/// 原生高位深文档取色和曲线黑/灰/白点；UI 只接收参数，不处理像素。
fn sampled_hsl_range(rgb:[f32;3])->Option<&'static str> {
    let (hue,saturation,_)=photocraft_compose::adjust::rgb_to_hsl(rgb);
    let ranges=["reds","yellows","greens","cyans","blues","magentas"];
    (saturation>0.01).then(||ranges[((hue*6.0).round() as usize)%6])
}

#[tauri::command]
pub async fn edit_preview_pick(
    state: State<'_, SharedState>,
    session_id: String,
    recipe: serde_json::Value,
    x: f64,
    y: f64,
    picker: String,
    mask_id: Option<String>,
    curve_channel:Option<String>,
    sample_size:Option<u32>,
) -> Result<serde_json::Value, String> {
    let recipe = recipe::parse_recipe(&recipe)?;
    let sample_size=sample_size.unwrap_or(1);
    if ![1,3,5,11].contains(&sample_size){return Err("取色范围无效".into());}
    if curve_channel.as_ref().is_some_and(|c|!["rgb","red","green","blue","luminance"].contains(&c.as_str())){return Err("曲线通道无效".into());}
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
        if picker=="readout" {
            // Reuse the proxy and adjustment caches; compose only the hovered pixel.
            let (base,renderer)={let p=preview.lock().map_err(|_|"预览锁不可用")?;(p.interactive_document.clone(),p.interactive_renderer.clone())};
            let doc=renderer.lock().map_err(|_|"预览渲染锁不可用")?.readout_document(&base,&recipe)?;
            let px=(x*f64::from(doc.size.width)).floor().min(f64::from(doc.size.width-1)) as i32;
            let py=(y*f64::from(doc.size.height)).floor().min(f64::from(doc.size.height-1)) as i32;
            let mut sample=sample_region(&doc,px,py,1)?;
            let profile=photocraft_engine::color_cmds::document_profile(&doc);
            let transform=photocraft_cms::cached(&profile,photocraft_cms::Builtin::Srgb.profile(),photocraft_cms::TransformOptions::default()).map_err(|e|e.to_string())?;
            transform.apply(&mut sample,3);
            return Ok(serde_json::json!({"sample":sample.map(|v|v.clamp(0.0,1.0)*255.0)}));
        }
        let (base,original_size)={let preview=preview.lock().map_err(|_|"预览锁不可用")?;(preview.document.clone(),preview.original_size)};
        let source_scale=(base.size.width.max(base.size.height) as f32/original_size[0].max(original_size[1]).max(1) as f32).min(1.0);
        let (a,mut before,base)=if let Some(id)=&mask_id {
            let index=recipe.masks.iter().position(|m|&m.id==id).ok_or("所选蒙版不存在")?;
            let mask=&recipe.masks[index];
            let mut below=recipe.clone();below.masks.truncate(index);
            let base=photocraft::adjusted_document_scaled(&base,&below,source_scale)?;
            let mut local=recipe.clone();local.masks.clear();local.adjustments=mask.adjustments;local.advanced=mask.advanced.clone();
            local.legacy_adjustments=None;local.color_pipeline=None;
            (mask.advanced.clone().unwrap_or_default(),local,base)
        } else {(recipe.advanced.clone().unwrap_or_default(),recipe.clone(),base)};
        picker_input_recipe(&mut before,&picker,curve_channel.as_deref());
        let doc = if mask_id.is_some() {
            // The lower stack already contains adjustment layers. Local camera
            // controls must append sampled upstream curves, never destructively
            // target that stack's top adjustment layer as if it were a raster.
            before.rotate_quarter=0;before.geometry=None;before.crop=None;
            before.text_layers.clear();before.brush_strokes.clear();
            let spatial=before.advanced.as_ref().and_then(|a|a.development.as_ref()).is_some_and(|d|d.active());
            let base=if spatial {photocraft::processing_input(&base)?}else{base};
            preview_renderer::Renderer::new(&base).with_pixel_scale(source_scale).canvas_document(&base,&before)?
        } else {photocraft::adjusted_document_scaled(&base, &before,source_scale)?};
        let px = (x * f64::from(doc.size.width))
            .floor()
            .min(f64::from(doc.size.width - 1)) as i32;
        let py = (y * f64::from(doc.size.height))
            .floor()
            .min(f64::from(doc.size.height - 1)) as i32;
        let sample=sample_region(&doc,px,py,sample_size)?;
        if picker=="hsl" {
            let range=sampled_hsl_range([sample[0],sample[1],sample[2]]);
            return Ok(serde_json::json!({"sample":[sample[0]*255.0,sample[1]*255.0,sample[2]*255.0],"range":range}));
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
            "eyedropper": {"point": picker, "color": sample}});
        for key in ["points", "red", "green", "blue"] {
            if params[key].as_array().is_some_and(|v| v.is_empty()) {
                params.as_object_mut().unwrap().remove(key);
            }
        }
        let adjustment =
            photocraft_engine::adjust_params::curves_eyedropper_from_params(&session, &params)
                .map_err(|e| e.to_string())?
                .ok_or("取色参数无效")?;
        let mut result=photocraft_engine::adjust_params::to_params(&adjustment);
        result["sample"]=serde_json::json!([sample[0]*255.0,sample[1]*255.0,sample[2]*255.0]);
        Ok(result)
    })
    .await
}

/// Average the same input-stage pixels for HSL, curve targets and white balance
/// eyedroppers. Clip to the photo edge and weight by opacity so transparent
/// padding cannot darken the sampled colour.
fn sample_region(doc:&photocraft_engine::doc::Document,x:i32,y:i32,size:u32)->Result<[f32;3],String>{
    let radius=(size/2) as i32;
    let left=(x-radius).max(0);let top=(y-radius).max(0);
    let right=(x+radius+1).min(doc.size.width as i32);let bottom=(y+radius+1).min(doc.size.height as i32);
    let area=photocraft_engine::doc::Rect::from_xywh(left,top,(right-left) as u32,(bottom-top) as u32);
    let rendered=photocraft_compose::render(doc,area);
    let mut total=[0.0f64;3];let mut weight=0.0f64;
    for p in rendered.px {let alpha=f64::from(p[3]);if alpha>0.0 {weight+=alpha;for c in 0..3 {total[c]+=f64::from(p[c])*alpha;}}}
    if weight<=0.0{return Err("不能对透明像素取色".into());}
    Ok(total.map(|v|(v/weight) as f32))
}

fn picker_input_recipe(recipe:&mut recipe::EditRecipe,picker:&str,channel:Option<&str>) {
    // Local masks run after the global stack. A global picker must not sample
    // downstream local adjustments; local callers have already composed below.
    recipe.masks.clear();
    if let Some(a)=&mut recipe.advanced {
        a.lookup=None;a.color_grading=None;
        if picker!="point"||channel.is_none(){a.curves.clear();a.channel_curves=Default::default();}
        else if channel==Some("luminance"){a.channel_curves.luminance.clear();}
        else {
            a.curves.clear();a.channel_curves.luminance.clear();
            match channel {Some("red")=>a.channel_curves.red.clear(),Some("green")=>a.channel_curves.green.clear(),Some("blue")=>a.channel_curves.blue.clear(),_=>{}}
        }
        if picker=="hsl" {a.hsl.clear();a.color_balance=None;a.black_white=None;a.selective_color=None;a.parametric_curve=None;}
    }
}

#[tauri::command]
pub async fn edit_preview_mask(
    state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value,mask_id:String,
)->Result<tauri::ipc::Response,String> {
    let recipe=recipe::parse_recipe(&recipe)?;
    run_blocking(state.inner().clone(),move |_|{
        use image::ImageEncoder;
        let preview=sessions().lock().map_err(|_|"预览锁不可用")?.get(&session_id).cloned().ok_or("预览会话已关闭")?;
        let (base,renderer)={let preview=preview.lock().map_err(|_|"预览锁不可用")?;(preview.interactive_document.clone(),preview.interactive_renderer.clone())};
        let doc=renderer.lock().map_err(|_|"蒙版锁不可用")?.mask_overlay(&base,&recipe,&mask_id)?;
        let pixels=photocraft_compose::flatten(&doc);
        let rgba:Vec<u8>=pixels.px.iter().flat_map(|p|p.map(|v|(v.clamp(0.0,1.0)*255.0).round() as u8)).collect();
        let mut png=Vec::new();
        image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba,doc.size.width,doc.size.height,image::ExtendedColorType::Rgba8).map_err(|e|format!("蒙版预览失败：{e}"))?;
        Ok(tauri::ipc::Response::new(png))
    }).await
}

fn clipping_pixels(pixels:&[[f32;4]],warnings:u8)->Vec<u8>{
    pixels.iter().flat_map(|p|{let (lo,hi)=photocraft_algo::histogram::clipping(*p);
        if warnings&2!=0&&hi!=0 {[255,0,0,255]}else if warnings&1!=0&&lo!=0 {[0,0,255,255]}else{[0,0,0,0]}
    }).collect()
}
fn photo_pixels(base:&Document,recipe:&recipe::EditRecipe,scale:f32)->Result<photocraft_compose::Buffer,String>{
    let mut recipe=recipe.clone();recipe.text_layers.clear();recipe.brush_strokes.clear();
    let doc=photocraft::render_document_scaled(base,&recipe,scale)?;
    let mut pixels=photocraft_compose::flatten(&doc);
    let mapping=super::native_geometry::mapping(&recipe,base.size);
    for (i,p) in pixels.px.iter_mut().enumerate(){
        let x=(i as u32%doc.size.width) as f32/doc.size.width as f32+0.5/doc.size.width as f32;
        let y=(i as u32/doc.size.width) as f32/doc.size.height as f32+0.5/doc.size.height as f32;
        let u=mapping[0]*x+mapping[1]*y+mapping[2];let v=mapping[4]*x+mapping[5]*y+mapping[6];
        if !(0.0..=1.0).contains(&u)||!(0.0..=1.0).contains(&v){p[3]=0.0;}
    }
    Ok(pixels)
}
fn clipping_overlay(base:&Document,recipe:&recipe::EditRecipe,scale:f32,warnings:u8)->Result<(Vec<u8>,[u32;2]),String>{
    let pixels=photo_pixels(base,recipe,scale)?;
    Ok((clipping_pixels(&pixels.px,warnings),[pixels.rect.width(),pixels.rect.height()]))
}
#[tauri::command]
pub async fn edit_preview_histogram(state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value,original:bool)->Result<Vec<Vec<u64>>,String>{
    let mut recipe=recipe::parse_recipe(&recipe)?;
    if original{if let Some(p)=recipe.color_pipeline.as_mut(){p.film=None;}recipe.adjustments=None;recipe.legacy_adjustments=None;recipe.advanced=None;recipe.masks.clear();}
    run_blocking(state.inner().clone(),move |_|{
        // 拖动期高频调用：交互文档（≤2048）渲染，毫秒级——255 桶显示直方图
        // 不需要全分辨率统计（2026-10-12 实时反馈改造）。
        let base=interactive_source(&session_id)?;
        histogram_pixels(&photo_pixels(&base,&recipe,1.0)?)
    }).await
}
#[tauri::command]
pub async fn edit_preview_clipping(state:State<'_,SharedState>,session_id:String,recipe:serde_json::Value,original:bool,warnings:u8)->Result<tauri::ipc::Response,String>{
    if warnings>3{return Err("曝光提示参数无效".into());}
    let mut recipe=recipe::parse_recipe(&recipe)?;
    if original {if let Some(p)=recipe.color_pipeline.as_mut(){p.film=None;}recipe.adjustments=None;recipe.legacy_adjustments=None;recipe.advanced=None;recipe.masks.clear();recipe.text_layers.clear();recipe.brush_strokes.clear();}
    run_blocking(state.inner().clone(),move |_|{
        use image::ImageEncoder;
        let base=canvas_source(&session_id)?;let size=canvas_original_size(&session_id)?;
        let scale=(base.size.width.max(base.size.height) as f32/size[0].max(size[1]).max(1) as f32).min(1.0);
        let (rgba,size)=clipping_overlay(&base,&recipe,scale,warnings)?;
        let mut png=Vec::new();image::codecs::png::PngEncoder::new(&mut png).write_image(&rgba,size[0],size[1],image::ExtendedColorType::Rgba8).map_err(|e|format!("曝光提示失败：{e}"))?;
        Ok(tauri::ipc::Response::new(png))
    }).await
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

#[tauri::command]
pub async fn edit_parametric_samples(state:State<'_,SharedState>,value:serde_json::Value)->Result<Vec<f32>,String>{
    let recipe=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"parametricCurve":value}}))?;
    run_blocking(state.inner().clone(),move |_|{
        let curve=recipe.advanced.and_then(|a|a.parametric_curve).ok_or("曲线分区参数缺失")?;
        if let photocraft_engine::doc::Adjustment::Curves{master,..}=super::parametric::adjustment(&curve){Ok(master.into_iter().map(|p|p.output*255.0).collect())}else{Err("曲线分区不可用".into())}
    }).await
}

/// CPU metadata snapshot for the direct canvas. Image pixels remain in stable
/// native tiles; the compositor uploads them once and keeps them on the GPU.
pub(super) fn canvas_source(session_id:&str)->Result<Document,String> {
    let value=sessions().lock().map_err(|_|"预览锁不可用")?.get(session_id).cloned().ok_or("预览会话已关闭")?;
    let mut p=value.lock().map_err(|_|"预览锁不可用")?;
    if p.generation.closed.load(Ordering::Acquire) {return Err("预览会话已关闭".into());}
    p.touched=Instant::now();Ok(p.document.clone())
}
/// 交互预览文档（≤2048 代理解码）：直方图等拖动期高频按需渲染走这里，
/// 不碰全分辨率 `document`（2026-10-12：滑块拖动中实时反馈）。
pub(super) fn interactive_source(session_id:&str)->Result<Document,String> {
    let value=sessions().lock().map_err(|_|"预览锁不可用")?.get(session_id).cloned().ok_or("预览会话已关闭")?;
    let mut p=value.lock().map_err(|_|"预览锁不可用")?;
    if p.generation.closed.load(Ordering::Acquire) {return Err("预览会话已关闭".into());}
    p.touched=Instant::now();Ok(p.interactive_document.clone())
}
pub(super) fn canvas_original_size(session_id:&str)->Result<[u32;2],String> {
    let value=sessions().lock().map_err(|_|"预览锁不可用")?.get(session_id).cloned().ok_or("预览会话已关闭")?;
    let p=value.lock().map_err(|_|"预览锁不可用")?;Ok(p.original_size)
}

#[cfg(test)]
mod startup_tests {
    use super::*;
    #[test]
    fn background_expiry_fences_closed_sessions_and_preserves_inflight_arcs() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("Expiry",Size::new(4,4),ColorMode::Rgb,SampleType::U8,Color::rgba(0.5,0.5,0.5,1.0));
        let generation=Arc::new(Generation::default());
        let preview=Arc::new(Mutex::new(Preview {
            original_size:[4,4], document:base.clone(), raw:None, refined:None,
            interactive_document:base.clone(), interactive_renderer:new_renderer(&base,[4,4]),
            refined_renderer:new_renderer(&base,[4,4]), open_ms:0.0, native_ready:false,
            pipeline_ready:false, pending_rawlab:None, sensor_raw:false,
            generation:generation.clone(), path:PathBuf::new(),modified:None,length:0,
            touched:Instant::now()-IDLE_TTL-Duration::from_secs(1),source_checked:Instant::now(),
        }));
        let id=format!("expiry-test-{}",uuid::Uuid::new_v4());
        sessions().lock().unwrap().insert(id.clone(),preview.clone());
        release_expired();
        assert!(!sessions().lock().unwrap().contains_key(&id));
        assert!(generation.closed.load(Ordering::Acquire));
        assert_eq!(Arc::strong_count(&preview),1);
        assert!(preview.lock().is_ok());
    }

    #[test]
    fn histogram_uses_upstream_float_bins_and_grayscale_conversion_without_phantom_pixels(){
        let pixels=photocraft_compose::Buffer{rect:photocraft_engine::doc::Rect::from_xywh(0,0,3,1),px:vec![[1.0,0.0,0.0,1.0],[0.0,1.0,0.0,0.0],[f32::NAN,0.0,0.0,1.0]]};
        let bins=histogram_pixels(&pixels).unwrap();assert_eq!(bins.len(),4);assert_eq!(bins[0][255],1);assert_eq!(bins[1][0],1);assert_eq!(bins[3][54],1);
        for channel in bins{assert_eq!(channel.iter().sum::<u64>(),1);}
    }
    #[test]
    fn current_histogram_follows_exposure_and_visible_crop(){
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("Histogram",Size::new(12,8),ColorMode::Rgb,SampleType::U8,Color::rgba(0.25,0.5,0.75,1.0));
        let neutral=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft"})).unwrap();
        let before=histogram_pixels(&photo_pixels(&base,&neutral,1.0).unwrap()).unwrap();
        let edited=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"exposure":1},"crop":{"x":0,"y":0,"w":0.5,"h":1}})).unwrap();
        let after=histogram_pixels(&photo_pixels(&base,&edited,1.0).unwrap()).unwrap();assert_ne!(before,after);
        for channel in before{assert_eq!(channel.iter().sum::<u64>(),96);}for channel in after{assert_eq!(channel.iter().sum::<u64>(),48);}
    }
    #[test]
    fn clipping_overlay_excludes_arbitrary_rotation_padding_and_annotations(){
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("Gray",Size::new(12,8),ColorMode::Rgb,SampleType::U8,Color::rgba(0.5,0.5,0.5,1.0));
        let r=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","geometry":{"angle":45},"brushStrokes":[{"id":"stroke","points":[{"x":0,"y":0},{"x":1,"y":1}],"widthRel":0.1,"color":"#000000"}]})).unwrap();
        let (rgba,_)=clipping_overlay(&base,&r,1.0,3).unwrap();assert!(rgba.chunks_exact(4).all(|p|p[3]==0));
    }
    #[test]
    fn clipping_warnings_match_upstream_channels_transparency_and_highlight_priority(){
        let pixels=[[0.0,0.5,0.5,1.0],[0.5,1.0,0.5,1.0],[0.0,1.0,0.5,1.0],[0.0,1.0,0.5,0.0],[f32::NAN,0.0,1.0,1.0],[0.001,0.5,0.999,1.0]];
        assert_eq!(clipping_pixels(&pixels,3),vec![0,0,255,255,255,0,0,255,255,0,0,255,0,0,0,0,0,0,0,0,0,0,0,0]);
        assert_eq!(&clipping_pixels(&pixels,1)[8..12],&[0,0,255,255]);assert_eq!(&clipping_pixels(&pixels,2)[0..4],&[0,0,0,0]);
        assert!(clipping_pixels(&pixels,0).iter().all(|v|*v==0));
    }
    #[test]
    fn region_sampling_clips_edges_and_ignores_transparent_padding(){
        let pixels=[255,0,0,255, 0,0,255,255, 0,255,0,0];
        let image=photocraft_codecs::Image::from_u8(3,1,photocraft_codecs::ChannelLayout::Rgba,pixels.to_vec()).unwrap();
        let doc=source::from_image("Sample",&image).unwrap();
        assert_eq!(sample_region(&doc,0,0,1).unwrap(),[1.0,0.0,0.0]);
        assert_eq!(sample_region(&doc,0,0,3).unwrap(),[0.5,0.0,0.5]);
        assert_eq!(sample_region(&doc,1,0,11).unwrap(),[0.5,0.0,0.5]);
        assert!(sample_region(&doc,2,0,1).is_err());
    }
    #[test]
    fn average_sample_uses_opacity_and_feeds_upstream_neutral_gray_picker(){
        let pixels=[255,0,0,255, 0,0,255,128, 0,255,0,0];
        let image=photocraft_codecs::Image::from_u8(3,1,photocraft_codecs::ChannelLayout::Rgba,pixels.to_vec()).unwrap();
        let doc=source::from_image("Sample",&image).unwrap();let sample=sample_region(&doc,1,0,3).unwrap();
        assert!((sample[0]-255.0/383.0).abs()<0.0001);assert!((sample[2]-128.0/383.0).abs()<0.0001);assert_eq!(sample[1],0.0);
        let mut session=photocraft_engine::Session::new();session.add_document(doc,None);
        let params=serde_json::json!({"eyedropper":{"point":"gray","color":sample}});
        let actual=photocraft_engine::adjust_params::curves_eyedropper_from_params(&session,&params).unwrap().unwrap();
        let expected=photocraft_engine::adjust_params::curves_eyedropper(&serde_json::json!({}),photocraft_engine::adjust_params::CurvesEyedropper::NeutralGray,sample,photocraft_engine::doc::ColorMode::Rgb).unwrap();
        assert_eq!(actual,expected);
    }
    #[test]
    fn curve_target_channels_sample_their_input_stage() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let recipe=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"curves":[[0,0],[255,128]],"channelCurves":{"red":[[0,0],[255,200]],"green":[[0,0],[255,180]],"luminance":[[0,0],[255,180]]}}})).unwrap();
        for picker in ["black","gray","white"] {
            let mut before=recipe.clone();picker_input_recipe(&mut before,picker,Some("red"));
            let a=before.advanced.unwrap();assert!(a.curves.is_empty()&&a.channel_curves.red.is_empty()&&a.channel_curves.green.is_empty()&&a.channel_curves.luminance.is_empty());
        }
        let base=Document::with_background("Curve input",Size::new(8,8),ColorMode::Rgb,SampleType::U8,Color::rgba(0.8,0.6,0.3,1.0));
        let mut rgb=recipe.clone();picker_input_recipe(&mut rgb,"point",Some("rgb"));
        let rgb=photocraft_compose::flatten(&photocraft::adjusted_document(&base,&rgb).unwrap()).px[0];
        assert!((rgb[0]-0.8*200.0/255.0).abs()<0.01);assert!((rgb[1]-0.6*180.0/255.0).abs()<0.01);
        let mut red=recipe.clone();picker_input_recipe(&mut red,"point",Some("red"));
        let red=photocraft_compose::flatten(&photocraft::adjusted_document(&base,&red).unwrap()).px[0];assert!((red[0]-0.8).abs()<0.01);
        let mut luminance=recipe;picker_input_recipe(&mut luminance,"point",Some("luminance"));
        let luminance=photocraft_compose::flatten(&photocraft::adjusted_document(&base,&luminance).unwrap()).px[0];
        assert!((luminance[0]-rgb[0]*128.0/255.0).abs()<0.01);
    }
    #[test]
    fn hsl_picker_excludes_downstream_black_white_and_local_adjustments() {
        let mut recipe=recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"blackWhite":{"enabled":true,"weights":[40,60,40,60,20,80],"tint":null},"colorBalance":{"shadows":[0,0,0],"midtones":[20,0,0],"highlights":[0,0,0],"preserveLuminosity":true},"selectiveColor":{"relative":true,"ranges":{"reds":[20,0,0,0]}},"hsl":{"reds":{"hue":90,"saturation":0,"lightness":0}}}})).unwrap();
        let original=recipe.clone();picker_input_recipe(&mut recipe,"hsl",None);
        let a=recipe.advanced.as_ref().unwrap();assert!(a.black_white.is_none()&&a.color_balance.is_none()&&a.selective_color.is_none()&&a.hsl.is_empty());
        let base=Document::with_background("Picker",photocraft_engine::doc::Size::new(8,8),photocraft_engine::doc::ColorMode::Rgb,photocraft_engine::doc::SampleType::U8,photocraft_engine::doc::Color::rgba(1.0,0.0,0.0,1.0));
        assert_eq!(photocraft_compose::flatten(&photocraft::adjusted_document(&base,&recipe).unwrap()).px,photocraft_compose::flatten(&base).px);
        let mut curve_input=original;picker_input_recipe(&mut curve_input,"point",None);assert!(curve_input.advanced.unwrap().black_white.is_some());
    }
    #[test]
    fn hsl_picker_uses_upstream_hue_and_does_not_classify_gray_as_red() {
        assert_eq!(sampled_hsl_range([1.0,0.0,0.0]),Some("reds"));
        assert_eq!(sampled_hsl_range([1.0,1.0,0.0]),Some("yellows"));
        assert_eq!(sampled_hsl_range([0.0,1.0,0.0]),Some("greens"));
        assert_eq!(sampled_hsl_range([0.0,1.0,1.0]),Some("cyans"));
        assert_eq!(sampled_hsl_range([0.0,0.0,1.0]),Some("blues"));
        assert_eq!(sampled_hsl_range([1.0,0.0,1.0]),Some("magentas"));
        assert_eq!(sampled_hsl_range([0.5,0.5,0.5]),None);
    }

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
