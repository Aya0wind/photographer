//! 监视文件夹命令（F4 v1）：add / remove / list。列表存 settings.watch_folders
//! （保存走 SettingsManager 原子写），轮询线程每轮重读——增删即时生效。

use tauri::State;

use std::path::Path;
use std::time::Duration;

use super::{run_blocking, SharedState};
use crate::events::{AppEvent, EventBus};
use crate::import::engine::{ImportMode, ImportPlan};
use crate::settings::{DuplicatePolicy, SettingsManager};

/// 监视文件夹列表（展示形态 = 用户输入的绝对路径）。
pub fn fetch_watch_folders(state: &super::AppState) -> Vec<String> {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .watch_folders
        .clone()
}

/// 添加监视文件夹核：目录存在性校验 + 去重 + 落盘。
pub fn fetch_watch_folder_add(state: &super::AppState, path: &str) -> Result<(), String> {
    let path = path.trim();
    if !std::path::Path::new(path).is_dir() {
        return Err(format!("目录不存在或不可访问: {path}"));
    }
    let mut settings = state.settings.lock().expect("settings mutex poisoned");
    if settings
        .watch_folders
        .iter()
        .any(|p| p.eq_ignore_ascii_case(path))
    {
        return Ok(()); // 幂等：已在监视列表
    }
    settings.watch_folders.push(path.to_string());
    SettingsManager::save(&settings, &state.config_dir).map_err(|e| e.to_string())
}

/// 移除监视文件夹核（不存在幂等返回 Ok）。
pub fn fetch_watch_folder_remove(state: &super::AppState, path: &str) -> Result<(), String> {
    let mut settings = state.settings.lock().expect("settings mutex poisoned");
    let before = settings.watch_folders.len();
    settings
        .watch_folders
        .retain(|p| !p.eq_ignore_ascii_case(path.trim()));
    if settings.watch_folders.len() == before {
        return Ok(()); // 幂等：本就不在列表
    }
    SettingsManager::save(&settings, &state.config_dir).map_err(|e| e.to_string())
}

/// 监视文件夹列表。
#[tauri::command]
pub async fn watch_folders_list(state: State<'_, SharedState>) -> Result<Vec<String>, String> {
    let shared = state.inner().clone();
    // 纯内存读（Mutex 快照）：豁免 run_blocking 铁律（无磁盘 IO）
    Ok(fetch_watch_folders(&shared))
}

/// 添加监视文件夹（即时生效：轮询线程下轮重读）。
#[tauri::command]
pub async fn watch_folder_add(state: State<'_, SharedState>, path: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_watch_folder_add(state, &path)).await
}

/// 移除监视文件夹。
#[tauri::command]
pub async fn watch_folder_remove(
    state: State<'_, SharedState>,
    path: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_watch_folder_remove(state, &path)).await
}

// ---------------------------------------------------------------------------
// 轮询 worker（F4 v1）
// ---------------------------------------------------------------------------

use crate::devices::folder::LocalFolderSource;
use crate::devices::{DeviceSource, SourceKind};

/// 轮询间隔（v1：5 分钟）。
pub const WATCH_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// 单轮扫描 + 自动入册。返回 (文件夹, 新文件数) 列表（本轮真正发起导入的）。
/// 任何单文件夹失败（目录消失/库不可用/Busy）记日志跳过，不影响其他文件夹。
pub fn poll_once(state: &super::AppState) -> Vec<(String, u64)> {
    let folders: Vec<String> = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .watch_folders
        .clone();
    let mut started = Vec::new();
    for folder in folders {
        if !Path::new(&folder).is_dir() {
            eprintln!("[watch] 目录不可用，跳过本轮: {folder}");
            continue;
        }
        let Ok(source) = LocalFolderSource::new(&folder) else {
            eprintln!("[watch] 打开监视目录失败: {folder}");
            continue;
        };
        let source: std::sync::Arc<dyn DeviceSource> = std::sync::Arc::new(source);
        // 注册表幂等登记（start_import 从注册表取源）
        state
            .devices
            .lock()
            .expect("devices mutex poisoned")
            .insert(
                source.id(),
                super::DeviceEntry {
                    scan: super::DeviceScan::Ready,
                    snapshot: crate::devices::orchestrator::DeviceSnapshot {
                        id: source.id(),
                        name: source.name(),
                        kind: SourceKind::Folder,
                        files_by_kind: Default::default(),
                        bytes_total: 0,
                        new_files: 0,
                    },
                    source: std::sync::Arc::clone(&source),
                },
            );
        // 新文件预检：0 新文件不建任务（防空任务刷屏）
        let new_files = match super::active_library_db(state) {
            Ok(db) => {
                let skip_imported = state
                    .settings
                    .lock()
                    .expect("settings mutex poisoned")
                    .import
                    .skip_imported;
                crate::devices::orchestrator::scan_device(&*source, &db, skip_imported)
                    .map(|s| s.new_files)
                    .unwrap_or(0)
            }
            Err(error) => {
                eprintln!("[watch] 库不可用，跳过本轮: {error}");
                continue;
            }
        };
        if new_files == 0 {
            continue;
        }
        let Some(library) = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .cloned()
        else {
            eprintln!("[watch] 无激活库，跳过");
            continue;
        };
        let streams = library.streams.max(1);
        // 0018 导入必落相册：监视入册自动归入默认相册「未分组」（按名幂等）
        let default_db_dir = std::path::PathBuf::from(&library.db_dir);
        let default_album = crate::ipc::open_library_db(&default_db_dir)
            .and_then(|db| db.ensure_default_album().map_err(|e| e.to_string()));
        let album_id = match default_album {
            Ok(id) => Some(id),
            Err(e) => {
                state.bus.publish(AppEvent::AppError {
                    level: "warn".into(),
                    message: format!("监视入册失败（默认相册创建失败）: {e}"),
                    recoverable: true,
                });
                continue;
            }
        };
        let plan = ImportPlan {
            source_id: source.id(),
            target_root: std::path::PathBuf::from(&library.photo_root),
            // 内层模板固定（0018）：album 段由引擎按 dir_name 拼接
            dir_template: "{YYYY}/{MM-DD}".into(),
            name_template: "{原文件名}".into(),
            duplicate_policy: DuplicatePolicy::Skip,
            skip_imported: true,
            streams,
            mode: ImportMode::Copy,
            second_target: None,
            include: None,
            album_id,
            album_subgroup: None,
        };
        match super::start_import(state, plan) {
            Ok(_job_id) => {
                eprintln!("[watch] {folder}: {new_files} 个新文件自动入册");
                started.push((folder, new_files));
            }
            Err(error) => eprintln!("[watch] 自动入册发起失败（下轮重试）: {error}"),
        }
    }
    started
}

/// 后台轮询线程（supervisor 派发）：启动即扫第一轮，随后每
/// [`WATCH_POLL_INTERVAL`] 一轮；每轮把「真正发起新导入」的文件夹经
/// `watchFolderImported` 事件上报。
pub fn spawn_watch_worker(
    state: super::SharedState,
    bus: &EventBus,
    supervisor: &std::sync::Arc<crate::tasks::TaskSupervisor>,
) {
    let bus = bus.clone();
    supervisor.spawn("watch", "watch-folder-poller".into(), move |controls| {
        loop {
            for (folder, files) in poll_once(&state) {
                bus.publish(AppEvent::WatchFolderImported { folder, files });
            }
            // 软退出轮：每秒醒来一次便于取消响应（v1 无取消入口，预留）
            for _ in 0..WATCH_POLL_INTERVAL.as_secs() {
                if controls.is_cancelled() {
                    return;
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        }
    });
}
