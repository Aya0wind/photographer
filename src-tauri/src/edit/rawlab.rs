//! Host orchestration for the pinned native core. Algorithms live upstream.
use super::{photocraft, recipe::EditRecipe, source};
use photocraft_engine::doc::{DocId, Document};
use photographer_media::rawlab::{Backend, ColorOptions, ColorSession, DevelopOptions, GpuMode};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc, Arc, OnceLock,
    },
    time::{Duration, Instant},
};

static LIBRARY: OnceLock<PathBuf> = OnceLock::new();
static BACKEND: OnceLock<Result<Arc<Backend>, String>> = OnceLock::new();
static CACHE_ROOT: OnceLock<PathBuf> = OnceLock::new();
pub(crate) fn init(resources: &Path, config: &Path) {
    let name = if cfg!(windows) {
        "photographer_raw.dll"
    } else if cfg!(target_os = "macos") {
        "libphotographer_raw.dylib"
    } else {
        "libphotographer_raw.so"
    };
    let _ = LIBRARY.set(resources.join("rawlab").join(name));
    let _ = CACHE_ROOT.set(config.join("smart-previews"));
}
pub(super) fn inspect_film(path: &Path) -> Result<photographer_media::rawlab::LutInfo, String> {
    backend()?.lut_info(path.to_str().ok_or("胶片路径不是 UTF-8")?)
}
fn backend() -> Result<Arc<Backend>, String> {
    BACKEND
        .get_or_init(|| {
            let path = LIBRARY.get().ok_or("RawLab 资源目录未初始化")?;
            // Only an app-bundled path is accepted; never a recipe/user library path.
            unsafe { Backend::load(path) }.map(Arc::new)
        })
        .clone()
}
pub(super) fn core_revision() -> String {
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../native/rawlab/upstream.lock.json"))
            .expect("checked upstream lock");
    lock["commit"].as_str().expect("locked commit").to_owned()
}
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Availability {
    available: bool,
    core_revision: String,
}
#[tauri::command]
pub async fn edit_rawlab_available(
    state: tauri::State<'_, crate::ipc::SharedState>,
) -> Result<Availability, String> {
    crate::ipc::run_blocking(state.inner().clone(), move |_| {
        Ok(Availability {
            available: backend().is_ok(),
            core_revision: core_revision(),
        })
    })
    .await
}

#[derive(Clone, PartialEq)]
struct ColorKey {
    film: Option<String>,
    strength: f32,
    exposure: f32,
    wb: [f32; 3],
}
fn color_key(recipe: &EditRecipe) -> Result<ColorKey, String> {
    let pipeline = recipe
        .color_pipeline
        .as_ref()
        .ok_or("未选择 RawLab 色彩链")?;
    let film = pipeline
        .film
        .as_ref()
        .filter(|f| f.enabled && f.amount > 0.0);
    let a = recipe.advanced.clone().unwrap_or_default();
    Ok(ColorKey {
        film: film.map(|f| f.id.clone()),
        strength: film.map_or(0.0, |f| f.amount as f32 / 100.0),
        exposure: a.exposure as f32,
        wb: photocraft_algo::camera_raw::white_balance_gains(a.temperature as f32, a.tint as f32),
    })
}
struct Linear {
    id: DocId,
    size: [u32; 2],
    rgb: Arc<Vec<f32>>,
    alpha: Option<Vec<f32>>,
}
fn linear(base: &Document) -> Result<Arc<Linear>, String> {
    if base.mode != photocraft_engine::doc::ColorMode::Rgb {
        return Err("RawLab 色彩链需要 RGB 来源".into());
    }
    let profile = photocraft_engine::color_cmds::document_profile(base);
    let target = photocraft_cms::Builtin::LinearSrgb.profile();
    let pixels = if base.depth == photocraft_engine::doc::SampleType::F32
        && profile.to_bytes() == target.to_bytes()
    {
        // Read source tiles directly: display compositing must not clamp RAW
        // negative/superwhite values before the upstream Log/film stage.
        let [layer] = base.layers.as_slice() else {
            return Err("线性 RAW 来源必须为单个源图层".into());
        };
        let photocraft_engine::doc::LayerContent::Raster(surface) = &layer.content else {
            return Err("线性 RAW 来源无像素数据".into());
        };
        let mut pixels = vec![[0.0; 4]; base.size.width as usize * base.size.height as usize];
        surface.read_rgba_into(base.bounds(), &mut pixels);
        pixels
    } else {
        photocraft_compose::flatten(base).px
    };
    let alpha = pixels
        .iter()
        .any(|p| p[3] != 1.0)
        .then(|| pixels.iter().map(|p| p[3]).collect());
    let mut rgb: Vec<f32> = pixels.iter().flat_map(|p| p[..3].iter().copied()).collect();
    if profile.to_bytes() != target.to_bytes() {
        let transform = photocraft_cms::Transform::new(
            &profile,
            &target,
            photocraft_cms::Intent::RelativeColorimetric,
            false,
        )
        .map_err(|e| e.to_string())?;
        transform.apply(&mut rgb, 3);
    }
    Ok(Arc::new(Linear {
        id: base.id,
        size: [base.size.width, base.size.height],
        rgb: Arc::new(rgb),
        alpha,
    }))
}
struct Job {
    source: Arc<Linear>,
    key: ColorKey,
    reply: mpsc::Sender<Result<Vec<f32>, String>>,
}
fn worker() -> Result<&'static mpsc::SyncSender<Job>, String> {
    static WORKER: OnceLock<Result<mpsc::SyncSender<Job>, String>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let backend = backend()?;
            let (sender, receiver) = mpsc::sync_channel::<Job>(2);
            std::thread::Builder::new()
                .name("rawlab-color".into())
                .spawn(move || {
                    // Contexts are created, used and destroyed here; none cross threads.
                    let mut sessions: HashMap<DocId, (ColorSession<'_>, usize, Instant)> =
                        HashMap::new();
                    loop {
                        sessions.retain(|_, (_, _, t)| t.elapsed() < Duration::from_secs(30));
                        let job = match receiver.recv_timeout(Duration::from_secs(2)) {
                            Ok(job) => job,
                            Err(mpsc::RecvTimeoutError::Timeout) => continue,
                            Err(_) => break,
                        };
                        let result = render_job(&backend, &mut sessions, &job);
                        let _ = job.reply.send(result);
                    }
                })
                .map_err(|e| e.to_string())?;
            Ok(sender)
        })
        .as_ref()
        .map_err(Clone::clone)
}
fn render_job<'a>(
    backend: &'a Backend,
    sessions: &mut HashMap<DocId, (ColorSession<'a>, usize, Instant)>,
    job: &Job,
) -> Result<Vec<f32>, String> {
    // Approximate native source/upload residency; oversized exports bypass cache.
    const LIMIT: usize = 128 * 1024 * 1024;
    let bytes = job.source.rgb.len().saturating_mul(8);
    let film = job
        .key
        .film
        .as_ref()
        .map(|id| super::film::path(id))
        .transpose()?;
    let path = film
        .as_ref()
        .map(|p| p.to_str().ok_or("胶片资源路径不是 UTF-8"))
        .transpose()?;
    let options = ColorOptions {
        lut_path: path,
        strength: job.key.strength,
        gpu: GpuMode::Auto,
        exposure_ev: job.key.exposure,
        relative_white_balance: job.key.wb,
    };
    let cancel = AtomicBool::new(false);
    if bytes > LIMIT {
        let mut session = backend.color_session(&job.source.rgb, job.source.size)?;
        return session
            .render(options, &cancel)
            .map(|image| image.rgb().to_vec());
    }
    if !sessions.contains_key(&job.source.id) {
        while sessions.len() >= 2
            || sessions
                .values()
                .map(|(_, b, _)| *b)
                .sum::<usize>()
                .saturating_add(bytes)
                > LIMIT
        {
            if let Some(id) = sessions
                .iter()
                .min_by_key(|(_, (_, _, t))| *t)
                .map(|(id, _)| *id)
            {
                sessions.remove(&id);
            } else {
                break;
            }
        }
        sessions.insert(
            job.source.id,
            (
                backend.color_session(&job.source.rgb, job.source.size)?,
                bytes,
                Instant::now(),
            ),
        );
    }
    let (session, _, t) = sessions.get_mut(&job.source.id).ok_or("色彩会话丢失")?;
    *t = Instant::now();
    session
        .render(options, &cancel)
        .map(|image| image.rgb().to_vec())
}
#[derive(Default)]
pub(super) struct ColorCache {
    input: Option<Arc<Linear>>,
    output: Option<(DocId, ColorKey, Document)>,
}
impl ColorCache {
    pub fn apply(
        &mut self,
        base: &Document,
        recipe: &EditRecipe,
    ) -> Result<(Document, EditRecipe), String> {
        if recipe.color_pipeline.is_none() {
            return Ok((base.clone(), recipe.clone()));
        }
        let key = color_key(recipe)?;
        if self.input.as_ref().is_none_or(|p| p.id != base.id) {
            self.input = Some(linear(base)?);
            self.output = None;
        }
        let document = if let Some((_, _, doc)) = self
            .output
            .as_ref()
            .filter(|(id, k, _)| *id == base.id && *k == key)
        {
            doc.clone()
        } else {
            let (reply, result) = mpsc::channel();
            let input = self.input.clone().ok_or("色彩来源丢失")?;
            let job = Job {
                source: input,
                key: key.clone(),
                reply,
            };
            let rgb = if job.source.rgb.len().saturating_mul(8) > 128 * 1024 * 1024 {
                // Export already runs on a blocking host thread, with its own
                // context and resource lease; preview GPU sessions stay responsive.
                let backend = backend()?;
                // Sessions borrow the backend and must be dropped before it.
                let mut sessions = HashMap::new();
                render_job(&backend, &mut sessions, &job)?
            } else {
                worker()?.send(job).map_err(|_| "色彩工作线程已退出")?;
                result.recv().map_err(|_| "色彩工作线程已退出")??
            };
            let alpha = self.input.as_ref().and_then(|input| input.alpha.as_deref());
            let mut doc = display_document(&rgb, [base.size.width, base.size.height], alpha)?;
            doc.metadata = base.metadata.clone();
            doc.resolution_dpi = base.resolution_dpi;
            self.output = Some((base.id, key, doc.clone()));
            doc
        };
        let downstream = downstream_recipe(recipe);
        Ok((document, downstream))
    }
}
fn downstream_recipe(recipe: &EditRecipe) -> EditRecipe {
    let mut values = recipe.clone();
    values.color_pipeline = None;
    if let Some(a) = values.advanced.as_mut() {
        a.exposure = 0.0;
        a.temperature = 0.0;
        a.tint = 0.0;
    }
    values
}
fn display_document(
    rgb: &[f32],
    size: [u32; 2],
    alpha: Option<&[f32]>,
) -> Result<Document, String> {
    use photocraft_engine::doc::{
        ColorMode, Layer, LayerContent, PixelFormat, SampleType, Size, Surface,
    };
    if rgb.len() != size[0] as usize * size[1] as usize * 3
        || alpha.is_some_and(|a| a.len() != rgb.len() / 3)
    {
        return Err("显示像素尺寸无效".into());
    }
    // Pack once straight into tiles; avoid float->codec->RGBA duplicate buffers.
    let mut bytes = Vec::with_capacity(rgb.len() / 3 * 8);
    for (i, p) in rgb.chunks_exact(3).enumerate() {
        for v in [p[0], p[1], p[2], alpha.map_or(1.0, |a| a[i])] {
            if !v.is_finite() {
                return Err("色彩结果包含无效数值".into());
            }
            bytes.extend(((v.clamp(0.0, 1.0) * 65535.0).round() as u16).to_ne_bytes());
        }
    }
    let mut doc = Document::new(
        "RawLab display",
        Size::new(size[0], size[1]),
        ColorMode::Rgb,
        SampleType::U16,
    );
    let mut surface = Surface::new(PixelFormat::new(ColorMode::Rgb, SampleType::U16, true));
    surface.write_interleaved(doc.bounds(), &bytes);
    doc.layers
        .push(Layer::new("Photo", LayerContent::Raster(surface)));
    doc.icc_profile = Some(photocraft_cms::Builtin::Srgb.profile().to_bytes());
    Ok(doc)
}
pub(super) fn display(
    base: &Document,
    recipe: &EditRecipe,
) -> Result<(Document, EditRecipe), String> {
    ColorCache::default().apply(base, recipe)
}

/// Full LibRaw source at export, bounded proxy at preparation. No source writes.
pub(super) fn open(
    path: &Path,
    edge: Option<u32>,
    cancel: &AtomicBool,
) -> Result<source::Opened, String> {
    let _cpu =
        crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image).acquire(1);
    let _memory = crate::tasks::index_budget::memory_budget().acquire(512);
    if cancel.load(Ordering::Acquire) {
        return Err("RAW 准备已取消".into());
    }
    open_inner(path, edge, cancel)
}
// Direct test callers retain Auto; isolated application workers select Force/Off.
static EXPORT_GPU: AtomicU8 = AtomicU8::new(1);
pub(super) fn configure_export_gpu(cpu_only: bool) {
    EXPORT_GPU.store(if cpu_only { 0 } else { 2 }, Ordering::Release);
}
pub(super) fn export_source(path: &Path, recipe: &EditRecipe) -> Result<(Document, EditRecipe), String> {
    let cancel = &AtomicBool::new(false);
    let _cpu = crate::tasks::index_budget::budget(crate::tasks::index_budget::Domain::Image).acquire(1);
    let _memory = crate::tasks::index_budget::memory_budget().acquire(512);
    let backend = backend()?;
    if !is_raw_path(path) {
        let opened = source::open(path)?;
        let input = linear(&opened.document)?;
        let rendered = export_color(&backend, &input.rgb, input.size, recipe, cancel)?;
        let mut document = display_document(rendered.rgb(), input.size, input.alpha.as_deref())?;
        document.metadata = opened.document.metadata.clone();
        document.resolution_dpi = opened.document.resolution_dpi;
        return Ok((document, downstream_recipe(recipe)));
    }
    let linear = backend.develop(path.to_str().ok_or("RAW 路径不是 UTF-8")?, DevelopOptions::default(), cancel)?;
    let size = linear.dimensions();
    let rendered = export_color(&backend, linear.rgb(), size, recipe, cancel)?;
    drop(linear);
    Ok((display_document(rendered.rgb(), size, None)?, downstream_recipe(recipe)))
}
fn export_color<'a>(backend: &'a Backend, rgb: &[f32], size: [u32;2], recipe: &EditRecipe,
    cancel: &AtomicBool) -> Result<photographer_media::rawlab::Image<'a>, String> {
    let key = color_key(recipe)?;
    let film = key.film.as_ref().map(|id| super::film::path(id)).transpose()?;
    let film_path = film.as_ref().map(|p| p.to_str().ok_or("胶片路径不是 UTF-8")).transpose()?;
    let gpu = match EXPORT_GPU.load(Ordering::Acquire) { 0 => GpuMode::Off, 2 => GpuMode::Force, _ => GpuMode::Auto };
    if !matches!(gpu, GpuMode::Off) { super::export_worker::begin_gpu()?; }
    let mut session = backend.color_session(rgb, size)?;
    let rendered = session.render(ColorOptions { lut_path: film_path, strength: key.strength,
        gpu, exposure_ev: key.exposure, relative_white_balance: key.wb }, cancel)?;
    drop(session);
    if !matches!(gpu, GpuMode::Off) { super::export_worker::end_gpu()?; }
    Ok(rendered)
}

fn open_inner(
    path: &Path,
    edge: Option<u32>,
    cancel: &AtomicBool,
) -> Result<source::Opened, String> {
    if cancel.load(Ordering::Acquire) {
        return Err("RAW 准备已取消".into());
    }
    let cached = edge.and_then(|edge| super::linear_cache::path(CACHE_ROOT.get()?, path, edge));
    if let Some((size, original, rgb)) = cached
        .as_ref()
        .and_then(|p| super::linear_cache::load(p, edge.unwrap_or(1600)))
    {
        return Ok(source::Opened {
            original_size: original,
            document: photocraft::linear_rgb_document("LibRaw proxy", size[0], size[1], &rgb)?,
            raw: None,
            warnings: vec![],
        });
    }
    let backend = backend()?;
    let image = backend.develop(
        path.to_str().ok_or("RAW 路径不是 UTF-8")?,
        DevelopOptions::default(),
        cancel,
    )?;
    let original_size = image.dimensions();
    let (size, rgb) =
        if let Some(edge) = edge.filter(|e| original_size[0].max(original_size[1]) > *e) {
            let long = original_size[0].max(original_size[1]);
            let w = (u64::from(original_size[0]) * u64::from(edge) / u64::from(long)).max(1) as u32;
            let h = (u64::from(original_size[1]) * u64::from(edge) / u64::from(long)).max(1) as u32;
            let buffer = image::ImageBuffer::<image::Rgb<f32>, Vec<f32>>::from_raw(
                original_size[0],
                original_size[1],
                image.rgb().to_vec(),
            )
            .ok_or("RAW 缓冲无效")?;
            (
                [w, h],
                image::imageops::resize(&buffer, w, h, image::imageops::FilterType::Triangle)
                    .into_raw(),
            )
        } else {
            (original_size, image.rgb().to_vec())
        };
    drop(image);
    if !cancel.load(Ordering::Acquire) {
        if let Some(path) = cached.as_ref() {
            let _ = super::linear_cache::save(path, size, original_size, &rgb);
        }
    }
    if cancel.load(Ordering::Acquire) {
        return Err("RAW 准备已取消".into());
    }
    Ok(source::Opened {
        original_size,
        document: photocraft::linear_rgb_document("LibRaw", size[0], size[1], &rgb)?,
        raw: None,
        warnings: vec![],
    })
}

pub(super) fn is_raw_path(path: &Path) -> bool {
    path.extension()
        .and_then(|s| s.to_str())
        .is_some_and(|ext| crate::thumbs::RAW_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_source_transfer_preserves_hdr_range_before_display() {
        let values = [-0.1, 0.18, 2.0];
        let base = photocraft::linear_rgb_document("Range", 1, 1, &values).unwrap();
        let input = linear(&base).unwrap();
        assert_eq!(input.rgb.as_slice(), values);
    }
    #[test]
    fn historical_recipes_do_not_enter_native_color_stage() {
        let recipe = super::super::recipe::parse_recipe(
            &serde_json::json!({"version":1,"renderer":"photocraft"}),
        )
        .unwrap();
        assert!(recipe.color_pipeline.is_none());
        let base = photocraft::linear_rgb_document("No stage", 1, 1, &[0.1; 3]).unwrap();
        let (same, values) = ColorCache::default().apply(&base, &recipe).unwrap();
        assert_eq!(same.id, base.id);
        assert_eq!(values, recipe);
        assert!(super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","colorPipeline":{"version":1,"backend":"rawlab","coreRevision":"unmatched"}})).is_err());
    }
}

#[cfg(test)]
#[path = "tests/rawlab_integration.rs"]
mod rawlab_host_tests;
