//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。

use tauri::{AppHandle, Emitter, State};

use super::SharedState;
use crate::settings::{Settings, SettingsManager};

/// 读取当前设置（内存快照）。
#[tauri::command]
pub fn settings_get(state: State<SharedState>) -> Settings {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone()
}

/// 保存设置到磁盘，更新内存快照，并广播 `settings://changed`。
#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    state: State<SharedState>,
    settings: Settings,
) -> Result<(), String> {
    SettingsManager::save(&settings, &state.config_dir).map_err(|err| err.to_string())?;
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    app.emit("settings://changed", &settings)
        .map_err(|err| err.to_string())?;
    Ok(())
}
