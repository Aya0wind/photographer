//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。
//! settings_set 附加两件事（2026-09-20）：① AI 索引参数投影到推理层
//! （embed 输入档位/人脸阈值即时生效）；② 参数指纹比对——变了就后台
//! 重建对应通道（改参数即自动重建，设置页手动按钮是兜底入口）。

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
    // 缩略图缓存上限即时生效（M8-③）
    crate::thumbs::set_thumb_cache_cap_bytes(
        u64::from(settings.storage.thumb_cache_max_gb) * 1024 * 1024 * 1024,
    );
    // AI 索引参数投影（推理层即时读新值）
    state.ai.set_ai_params(crate::ai::AiIndexParams {
        embed_input_size: settings.ai.embed_input_size,
        face_detect_threshold: settings.ai.face_detect_threshold,
        face_cluster_threshold: settings.ai.face_cluster_threshold,
    });
    // 参数指纹比对：变更通道后台自动重建（无库/无变更为 no-op）
    let ai_snapshot = settings.ai.clone();
    if let Some(library) = settings.active_library() {
        let db_dir = std::path::PathBuf::from(&library.db_dir);
        let shared = state.inner().clone();
        state
            .supervisor
            .spawn("index", "params-fingerprint-check".into(), move |_| {
                super::indexing::check_params_and_rebuild(&shared, &db_dir, &ai_snapshot);
            });
    }
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    app.emit("settings://changed", &settings)
        .map_err(|err| err.to_string())?;
    Ok(())
}
