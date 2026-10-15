//! Killable render/encode boundary. Only the parent publishes output and touches DB.
use super::{
    export::{self, ValidatedOptions},
    recipe::EditRecipe,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Serialize, Deserialize)]
struct Environment {
    resources: PathBuf,
    config: PathBuf,
}
const GPU_TIMEOUT: Duration = Duration::from_secs(30);
static WORK_DIRECTORY: OnceLock<PathBuf> = OnceLock::new();
static STOPPING: AtomicBool = AtomicBool::new(false);
static ENVIRONMENT: OnceLock<Environment> = OnceLock::new();
pub(crate) fn init(resources: &Path, config: &Path) {
    let _ = ENVIRONMENT.set(Environment {
        resources: resources.into(),
        config: config.into(),
    });
}
#[derive(Serialize, Deserialize)]
struct Request {
    environment: Environment,
    source: PathBuf,
    recipe: EditRecipe,
    options: ValidatedOptions,
    cpu_only: bool,
}
#[derive(Serialize, Deserialize)]
struct Output {
    width: u32,
    height: u32,
}
fn children() -> &'static Mutex<HashMap<u32, Child>> {
    static CHILDREN: OnceLock<Mutex<HashMap<u32, Child>>> = OnceLock::new();
    CHILDREN.get_or_init(|| Mutex::new(HashMap::new()))
}
struct ChildGuard(u32);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let child = children()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.0);
        if let Some(mut child) = child {
            let _ = child.kill();
            let started = std::time::Instant::now();
            while matches!(child.try_wait(), Ok(None)) && started.elapsed() < Duration::from_secs(2)
            {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}
pub(crate) fn kill_all() {
    STOPPING.store(true, Ordering::Release);
    let mut children = children().lock().unwrap_or_else(|e| e.into_inner());
    for child in children.values_mut() {
        let _ = child.kill();
    }
    // Emergency exit must never wait for a wedged driver/native teardown.
    children.clear();
}

pub(super) fn render(
    source: &Path,
    recipe: &EditRecipe,
    options: &ValidatedOptions,
    progress: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<u8>, u32, u32), String> {
    // Unit/integration test binaries have no application worker entry point.
    if cfg!(test) {
        return export::encode_output(source, recipe, options, progress);
    }
    let environment = ENVIRONMENT.get().ok_or("导出资源目录未初始化")?.clone();
    let directory = tempfile::Builder::new()
        .prefix("photographer-export-")
        .tempdir()
        .map_err(|e| e.to_string())?;
    let root = directory.path();
    let mut request = Request {
        environment,
        source: source.into(),
        recipe: recipe.clone(),
        options: options.clone(),
        cpu_only: false,
    };
    gpu_first(
        |cpu_only| {
            request.cpu_only = cpu_only;
            attempt(root, &request, progress, cancelled)
        },
        progress,
        cancelled,
    )
}

#[derive(Debug)]
struct AttemptFailure {
    message: String,
    gpu: bool,
}
impl From<String> for AttemptFailure {
    fn from(message: String) -> Self {
        Self {
            message,
            gpu: false,
        }
    }
}
fn gpu_first<T>(
    mut attempt: impl FnMut(bool) -> Result<T, AttemptFailure>,
    progress: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<T, String> {
    let result = attempt(false);
    if cancelled() {
        return Err("导出已取消".into());
    }
    match result {
        Ok(value) => Ok(value),
        Err(error) if error.gpu && !cancelled() && !STOPPING.load(Ordering::Acquire) => {
            log::warn!(
                "RawLab GPU export failed; retrying on CPU: {}",
                error.message
            );
            progress("cpuFallback");
            let result = attempt(true);
            if cancelled() {
                return Err("导出已取消".into());
            }
            result.map_err(|cpu| format!("GPU: {}; CPU: {}", error.message, cpu.message))
        }
        Err(error) => Err(error.message),
    }
}
fn attempt(
    root: &Path,
    request: &Request,
    progress: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
) -> Result<(Vec<u8>, u32, u32), AttemptFailure> {
    for file in ["phase", "error", "gpu-active", "image.bin", "result.json"] {
        match std::fs::remove_file(root.join(file)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.to_string().into()),
        }
    }
    std::fs::write(
        root.join("request.json"),
        serde_json::to_vec(request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if !request.cpu_only {
        progress("queued");
    }
    let child = spawn(root, cancelled)?;
    monitor(child.0, root, progress, cancelled, GPU_TIMEOUT)?;
    if cancelled() {
        return Err("导出已取消".to_string().into());
    }
    let output: Output = serde_json::from_slice(
        &std::fs::read(root.join("result.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let bytes = std::fs::read(root.join("image.bin")).map_err(|e| e.to_string())?;
    Ok((bytes, output.width, output.height))
}

fn spawn(root: &Path, cancelled: &dyn Fn() -> bool) -> Result<ChildGuard, String> {
    let mut command = Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.arg("--photographer-export-worker").arg(root);
    let threads = std::thread::available_parallelism()
        .map(|n| (n.get() / 2).max(1))
        .unwrap_or(1);
    command
        .env("RAYON_NUM_THREADS", threads.to_string())
        .env("OMP_NUM_THREADS", threads.to_string());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    loop {
        if cancelled() {
            return Err("导出已取消".into());
        }
        let mut running = children().lock().map_err(|_| "导出进程锁不可用")?;
        if STOPPING.load(Ordering::Acquire) {
            return Err("应用正在退出".into());
        }
        if running.is_empty() {
            let child = command
                .spawn()
                .map_err(|e| format!("启动导出进程失败: {e}"))?;
            let id = child.id();
            running.insert(id, child);
            return Ok(ChildGuard(id));
        }
        drop(running);
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn monitor(
    id: u32,
    root: &Path,
    progress: &dyn Fn(&str),
    cancelled: &dyn Fn() -> bool,
    gpu_timeout: Duration,
) -> Result<(), AttemptFailure> {
    let mut gpu_started: Option<Instant> = None;
    let mut phase = String::new();
    loop {
        if cancelled() {
            return Err("导出已取消".to_string().into());
        }
        if let Ok(next) = std::fs::read_to_string(root.join("phase")) {
            if !next.is_empty() && next != phase {
                progress(&next);
                phase = next;
            }
        }
        let status = children()
            .lock()
            .map_err(|_| "导出进程锁不可用".to_string())?
            .get_mut(&id)
            .ok_or_else(|| "导出进程已退出".to_string())?
            .try_wait()
            .map_err(|e| e.to_string())?;
        if let Some(status) = status {
            if !status.success() {
                return Err(AttemptFailure {
                    message: std::fs::read_to_string(root.join("error"))
                        .unwrap_or_else(|_| format!("导出进程异常退出: {status}")),
                    gpu: root.join("gpu-active").exists(),
                });
            }
            return Ok(());
        }
        check_gpu_deadline(root, &mut gpu_started, gpu_timeout)?;
        std::thread::sleep(Duration::from_millis(100));
    }
}

// Count only the marked native GPU interval; decoding and encoding have no deadline.
fn check_gpu_deadline(
    root: &Path,
    started: &mut Option<Instant>,
    timeout: Duration,
) -> Result<(), AttemptFailure> {
    if !root.join("gpu-active").exists() {
        *started = None;
        return Ok(());
    }
    if started.get_or_insert_with(Instant::now).elapsed() >= timeout {
        return Err(AttemptFailure {
            message: format!("RawLab GPU 处理超过 {} 秒", timeout.as_secs()),
            gpu: true,
        });
    }
    Ok(())
}

/// Called before Tauri, plugins, single-instance registration or background pools.
pub(crate) fn run_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--photographer-export-worker")) {
        return false;
    }
    let Some(root) = args.next().map(PathBuf::from) else {
        std::process::exit(2);
    };
    let result = run(&root);
    if let Err(error) = &result {
        let _ = std::fs::write(root.join("error"), error);
    }
    // Exit without waiting for native GPU helper threads or async-runtime teardown.
    std::process::exit(if result.is_ok() { 0 } else { 1 });
}
/// Marker covers native session allocation/upload/render/readback, never decode/encode.
pub(super) fn begin_gpu() -> Result<(), String> {
    if let Some(root) = WORK_DIRECTORY.get() {
        std::fs::write(root.join("gpu-active"), b"active").map_err(|e| e.to_string())?;
        std::fs::write(root.join("phase"), "gpu").map_err(|e| e.to_string())?;
    }
    Ok(())
}
pub(super) fn end_gpu() -> Result<(), String> {
    if let Some(root) = WORK_DIRECTORY.get() {
        std::fs::remove_file(root.join("gpu-active")).map_err(|e| e.to_string())?;
    }
    Ok(())
}
fn run(root: &Path) -> Result<(), String> {
    let request: Request = serde_json::from_slice(
        &std::fs::read(root.join("request.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let _ = WORK_DIRECTORY.set(root.to_path_buf());
    super::rawlab::configure_export_gpu(request.cpu_only);
    super::lut::init(&request.environment.config);
    super::film::init(&request.environment.config);
    super::film::init_builtins(&request.environment.resources);
    super::rawlab::init(&request.environment.resources, &request.environment.config);
    let progress = |phase: &str| {
        let _ = std::fs::write(root.join("phase"), phase);
    };
    let (bytes, width, height) = export::encode_output(
        &request.source,
        &request.recipe,
        &request.options,
        &progress,
    )?;
    std::fs::write(root.join("image.bin"), bytes).map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("result.json"),
        serde_json::to_vec(&Output { width, height }).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gpu_failure_retries_cpu_once_and_other_failures_do_not_retry() {
        let mut modes = Vec::new();
        let result = gpu_first(
            |cpu| {
                modes.push(cpu);
                if cpu {
                    Ok("done")
                } else {
                    Err(AttemptFailure {
                        message: "GPU OOM".into(),
                        gpu: true,
                    })
                }
            },
            &|_| {},
            &|| false,
        );
        assert_eq!(result.unwrap(), "done");
        assert_eq!(modes, [false, true]);
        let mut calls = 0;
        let result: Result<(), _> = gpu_first(
            |_| {
                calls += 1;
                Err("decode failed".to_string().into())
            },
            &|_| {},
            &|| false,
        );
        assert!(result.unwrap_err().contains("decode failed"));
        assert_eq!(calls, 1);
    }
    #[test]
    fn gpu_success_is_not_retried_and_cancellation_prevents_cpu_fallback() {
        let mut modes = Vec::new();
        let result = gpu_first(
            |cpu| {
                modes.push(cpu);
                Ok("GPU")
            },
            &|_| {},
            &|| false,
        );
        assert_eq!(result.unwrap(), "GPU");
        assert_eq!(modes, [false]);
        let mut modes = Vec::new();
        let result: Result<(), _> = gpu_first(
            |cpu| {
                modes.push(cpu);
                Err(AttemptFailure {
                    message: "GPU failed".into(),
                    gpu: true,
                })
            },
            &|_| {},
            &|| true,
        );
        assert_eq!(result.unwrap_err(), "导出已取消");
        assert_eq!(modes, [false]);
    }
    #[test]
    fn cpu_failure_ends_the_job_without_another_retry() {
        let mut modes = Vec::new();
        let result: Result<(), _> = gpu_first(
            |cpu| {
                modes.push(cpu);
                Err(AttemptFailure {
                    message: "failed".into(),
                    gpu: !cpu,
                })
            },
            &|_| {},
            &|| false,
        );
        assert!(result.unwrap_err().contains("CPU"));
        assert_eq!(modes, [false, true]);
    }
    #[test]
    fn cancellation_during_fallback_keeps_the_cancelled_terminal_reason() {
        let cancelled = std::cell::Cell::new(false);
        let result: Result<(), _> = gpu_first(
            |cpu| {
                cancelled.set(cpu);
                Err(AttemptFailure {
                    message: "GPU failed".into(),
                    gpu: true,
                })
            },
            &|_| {},
            &|| cancelled.get(),
        );
        assert_eq!(result.unwrap_err(), "导出已取消");
    }
    #[test]
    #[ignore = "subprocess fixture, launched explicitly by blocked_child_is_reaped"]
    fn blocked_process() {
        std::fs::write(
            std::env::var_os("PHOTOGRAPHER_EXPORT_TEST_READY").unwrap(),
            b"ready",
        )
        .unwrap();
        std::thread::sleep(Duration::from_secs(30));
    }
    #[test]
    fn blocked_child_is_reaped() {
        let root = tempfile::tempdir().unwrap();
        let ready = root.path().join("ready");
        let module = module_path!().split_once("::").unwrap().1;
        let child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                &format!("{module}::blocked_process"),
            ])
            .env("PHOTOGRAPHER_EXPORT_TEST_READY", &ready)
            .spawn()
            .unwrap();
        let id = child.id();
        children().lock().unwrap().insert(id, child);
        let guard = ChildGuard(id);
        let start = std::time::Instant::now();
        while !ready.exists() && start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(ready.exists(), "child did not enter the blocked stage");
        // No marker: decoding is not governed by the GPU deadline.
        let start = Instant::now();
        let error = monitor(
            id,
            root.path(),
            &|_| {},
            &|| start.elapsed() > Duration::from_millis(120),
            Duration::ZERO,
        )
        .unwrap_err();
        assert!(!error.gpu);
        assert_eq!(error.message, "导出已取消");
        std::fs::write(root.path().join("gpu-active"), b"active").unwrap();
        let error = monitor(
            id,
            root.path(),
            &|_| {},
            &|| false,
            Duration::from_millis(10),
        )
        .unwrap_err();
        assert!(error.gpu);
        let start = std::time::Instant::now();
        drop(guard);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!children().lock().unwrap().contains_key(&id));
    }
}
