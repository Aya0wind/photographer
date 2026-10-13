//! 独立、单设备 GPU 合成线程。队列和缓存有界；任何失败由调用方重算 CPU。
use std::{
    collections::VecDeque,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, OnceLock,
    },
    time::{Duration, Instant},
};

use photocraft_engine::doc::{ColorMode, Surface};
use photocraft_engine::doc::{DocId, Document, Rect};
use photocraft_gpu::{Compositor, DeviceHealth};

const MEMORY_BUDGET: u64 = 256 * 1024 * 1024;
const MAX_DOCUMENTS: usize = 4;
const STALL_TIMEOUT: Duration = Duration::from_secs(30);

struct Request {
    doc: Document,
    // None 是行带完成心跳，避免大图总耗时被误认为设备卡死。
    reply: mpsc::Sender<Result<Option<Surface>, String>>,
}

enum Command {
    Render(Request),
    Clear,
}
static WORKER: OnceLock<Worker> = OnceLock::new();

struct Worker {
    sender: mpsc::SyncSender<Command>,
    disabled: Arc<AtomicBool>,
    busy: AtomicBool,
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    health: DeviceHealth,
    compositor: Compositor,
    documents: VecDeque<DocId>,
}

impl Gpu {
    fn new() -> Result<Self, String> {
        let instance = wgpu::Instance::default();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
            ..Default::default()
        }))
        .map_err(|e| format!("无可用 GPU: {e}"))?;
        let info = adapter.get_info();
        if info.device_type == wgpu::DeviceType::Cpu {
            return Err("软件 GPU 不提供硬件加速".into());
        }
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("GPU 初始化失败: {e}"))?;
        let health = DeviceHealth::watch(&device);
        let mut compositor =
            Compositor::try_new_with_format(&device, Compositor::preferred_acc_format(&adapter))
                .map_err(|e| e.0)?;
        compositor.set_health(health.clone());
        compositor.set_texture_limit(device.limits().max_texture_dimension_2d);
        compositor.set_memory_budget(MEMORY_BUDGET);
        log::warn!(
            "[editor GPU] {} ({:?}), cache budget 256 MiB",
            info.name,
            info.backend
        );
        Ok(Self {
            device,
            queue,
            health,
            compositor,
            documents: VecDeque::new(),
        })
    }

    fn render(&mut self, request: &Request) -> Result<Surface, String> {
        // 基础文档和 raster tile 身份稳定，连续调整复用已上传的原片。
        self.documents.retain(|id| *id != request.doc.id);
        self.documents.push_back(request.doc.id);
        while self.documents.len() > MAX_DOCUMENTS {
            if let Some(id) = self.documents.pop_front() {
                self.compositor.forget_doc(id);
            }
        }
        let doc = &request.doc;
        let mut output = Surface::new(doc.pixel_format());
        // 每次 float 读回最多约 16 MiB，行带最多 256 行。
        let rows = (1024 * 1024 / doc.size.width.max(1)).clamp(1, 256);
        for y in (0..doc.size.height).step_by(rows as usize) {
            if request.reply.send(Ok(None)).is_err() {
                return Err("GPU 请求已取消".into());
            }
            let height = rows.min(doc.size.height - y);
            let pixels = photocraft_gpu::render_to_vec(
                &mut self.compositor,
                &self.device,
                &self.queue,
                doc,
                Rect::from_xywh(0, y as i32, doc.size.width, height),
            )
            .map_err(|e| e.0)?;
            if let Some(fault) = self.health.fault() {
                return Err(fault.to_string());
            }
            let flat: Vec<f32> = pixels.into_iter().flatten().collect();
            output.write_region(Rect::from_xywh(0, y as i32, doc.size.width, height), &flat);
        }
        Ok(output)
    }
}

fn process(gpu: &mut Option<Gpu>, request: &Request) -> (Result<Surface, String>, bool) {
    let mut unsupported = false;
    let result = catch_unwind(AssertUnwindSafe(|| {
        if gpu.is_none() {
            *gpu = Some(Gpu::new()?);
        }
        let gpu = gpu.as_mut().ok_or("GPU 未初始化")?;
        if gpu.health.is_ok() {
            if let Err(error) = gpu.compositor.supports(&request.doc) {
                unsupported = true;
                return Err(error.0);
            }
        }
        gpu.render(request)
    }))
    .unwrap_or_else(|_| Err("GPU 驱动或合成器 panic".into()));
    (result, unsupported)
}

fn clear_documents(gpu: &mut Option<Gpu>) {
    if let Some(gpu) = gpu.as_mut() {
        for id in gpu.documents.drain(..) {
            gpu.compositor.forget_doc(id);
        }
    }
}

fn render_request(gpu: &mut Option<Gpu>, request: Request, stopped: &AtomicBool) {
    let (result, unsupported) = process(gpu, &request);
    if let Err(reason) = &result {
        log::warn!("[editor GPU] CPU fallback: {reason}");
        if !unsupported {
            stopped.store(true, Ordering::Release);
        }
    }
    let _ = request.reply.send(result.map(Some));
}

fn run(receiver: mpsc::Receiver<Command>, stopped: Arc<AtomicBool>) {
    let mut gpu = None;
    let mut last_render = Instant::now();
    while !stopped.load(Ordering::Acquire) {
        let timeout = Duration::from_secs(60).saturating_sub(last_render.elapsed());
        match receiver.recv_timeout(timeout) {
            Ok(Command::Render(request)) => {
                render_request(&mut gpu, request, &stopped);
                last_render = Instant::now();
            }
            Ok(Command::Clear) => clear_documents(&mut gpu),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if gpu.is_some() {
                    log::info!("编辑器 GPU 空闲设备已卸载");
                }
                gpu = None;
                last_render = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn start() -> Worker {
    let (sender, receiver) = mpsc::sync_channel::<Command>(8);
    let disabled = Arc::new(AtomicBool::new(false));
    let stopped = disabled.clone();
    let spawned = std::thread::Builder::new()
        .name("editor-gpu".into())
        .spawn(move || run(receiver, stopped));
    if let Err(error) = spawned {
        log::warn!("[editor GPU] CPU fallback: {error}");
        disabled.store(true, Ordering::Release);
    }
    Worker {
        sender,
        disabled,
        busy: AtomicBool::new(false),
    }
}

/// None 代表立即使用既有 CPU 合成；队列满时不阻塞后台任务堆积。
pub(super) fn composite(doc: &Document) -> Option<Surface> {
    if doc.mode != ColorMode::Rgb {
        return None;
    }
    let worker = WORKER.get_or_init(start);
    if worker.disabled.load(Ordering::Acquire) {
        return None;
    }
    // 同时只有一个 GPU 请求；其他导出/会话立即使用 CPU，不排队等待。
    worker
        .busy
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .ok()?;
    struct BusyGuard<'a>(&'a AtomicBool);
    impl Drop for BusyGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Release);
        }
    }
    let _busy = BusyGuard(&worker.busy);
    let (reply, receiver) = mpsc::channel();
    worker
        .sender
        .try_send(Command::Render(Request {
            doc: doc.clone(),
            reply,
        }))
        .ok()?;
    loop {
        match receiver.recv_timeout(STALL_TIMEOUT) {
            Ok(Ok(Some(image))) => return Some(image),
            Ok(Ok(None)) => continue,
            Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                worker.disabled.store(true, Ordering::Release);
                log::warn!(
                    "[editor GPU] CPU fallback: GPU stalled for 30s; disabled for this process"
                );
                return None;
            }
        }
    }
}

/// Nonblocking; queued renders finish before cache ownership is released.
pub(super) fn release_cached_documents() {
    if let Some(worker) = WORKER.get() {
        let _ = worker.sender.try_send(Command::Clear);
    }
}
