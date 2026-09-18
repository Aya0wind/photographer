//! settings 命令：`settings_get` / `settings_set`。

use tauri::{AppHandle, Emitter, State};

use super::AppState;
use crate::settings::{Settings, SettingsManager};

/// 读取当前设置（内存快照）。
#[tauri::command]
pub fn settings_get(state: State<AppState>) -> Settings {
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
    state: State<AppState>,
    settings: Settings,
) -> Result<(), String> {
    SettingsManager::save(&settings, &state.config_dir).map_err(|err| err.to_string())?;
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    app.emit("settings://changed", &settings)
        .map_err(|err| err.to_string())?;
    Ok(())
}
