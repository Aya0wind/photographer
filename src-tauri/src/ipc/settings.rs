//! settings 命令：`settings_get` / `settings_set`。
//!
//! 铁律（2026-09-18）：settings_set 落盘（原子写 tmp + rename）→ async +
//! spawn_blocking；settings_get 纯内存快照读，保留同步。

use tauri::{AppHandle, Emitter, State};

use super::{run_blocking, SharedState};
use crate::settings::{Settings, SettingsManager};

/// 读取当前设置（内存快照）。纯内存，保留同步。
#[tauri::command]
pub fn settings_get(state: State<SharedState>) -> Settings {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone()
}

/// 保存设置到磁盘，更新内存快照，并广播 `settings://changed`。
/// 磁盘写（含 NAS 配置目录的可能）在后台线程；emit 在 await 之后回到
/// async 上下文执行（AppHandle 仅用于事件，非热路径）。
#[tauri::command]
pub async fn settings_set(
    app: AppHandle,
    state: State<'_, SharedState>,
    settings: Settings,
) -> Result<(), String> {
    let broadcast = settings.clone();
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        SettingsManager::save(&settings, &state.config_dir).map_err(|err| err.to_string())?;
        *state.settings.lock().expect("settings mutex poisoned") = settings;
        Ok(())
    })
    .await?;
    app.emit("settings://changed", &broadcast)
        .map_err(|err| err.to_string())?;
    Ok(())
}
