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
    time::Duration,
};

#[derive(Clone, Serialize, Deserialize)]
struct Environment {
    resources: PathBuf,
    config: PathBuf,
}
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
    let request = Request {
        environment,
        source: source.into(),
        recipe: recipe.clone(),
        options: options.clone(),
    };
    std::fs::write(
        root.join("request.json"),
        serde_json::to_vec(&request).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    progress("queued");
    let child = spawn(root, cancelled)?;
    monitor(child.0, root, progress, cancelled)?;
    if cancelled() {
        return Err("导出已取消".into());
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
) -> Result<(), String> {
    let mut phase = String::new();
    loop {
        if cancelled() {
            return Err("导出已取消".into());
        }
        if let Ok(next) = std::fs::read_to_string(root.join("phase")) {
            if !next.is_empty() && next != phase {
                progress(&next);
                phase = next;
            }
        }
        let status = children()
            .lock()
            .map_err(|_| "导出进程锁不可用")?
            .get_mut(&id)
            .ok_or("导出进程已退出")?
            .try_wait()
            .map_err(|e| e.to_string())?;
        if let Some(status) = status {
            if !status.success() {
                return Err(std::fs::read_to_string(root.join("error"))
                    .unwrap_or_else(|_| format!("导出进程异常退出: {status}")));
            }
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
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
fn run(root: &Path) -> Result<(), String> {
    let request: Request = serde_json::from_slice(
        &std::fs::read(root.join("request.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
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
        let start = std::time::Instant::now();
        drop(guard);
        assert!(start.elapsed() < Duration::from_secs(3));
        assert!(!children().lock().unwrap().contains_key(&id));
    }
}
