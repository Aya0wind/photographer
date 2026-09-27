//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。
//! settings_set 附加两件事（2026-09-20）：① AI 索引参数投影到推理层
//! （embed 输入档位/人脸阈值即时生效）；② 参数指纹比对——变了就后台
//! 重建对应通道（改参数即自动重建，设置页手动按钮是兜底入口）。

use tauri::{AppHandle, Emitter, State};

use super::{run_blocking, SharedState};
use crate::settings::{Settings, SettingsManager};

/// 删除库的物理结果（camelCase）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDeleteResult {
    pub db_deleted: bool,
    pub photo_root_deleted: bool,
}

/// 删除库核：db_dir 必删（数据库/缩略图/向量/标记），photo_root 可选连删。
/// 三道安全闸：①目录必须形如库数据目录（存在 library.db）；②不得为当前
/// 活跃库（前端删除前先清 activeLibraryId，这里双保险）；③照片目录拒绝
/// 盘根/与 db_dir 相同。前端的「输入库名确认」对话框是第一道用户侧闸。
pub fn fetch_library_delete(
    state: &super::AppState,
    db_dir: &str,
    photo_root: Option<&str>,
) -> Result<LibraryDeleteResult, String> {
    let db_path = std::path::PathBuf::from(db_dir);
    if !db_path.join("library.db").is_file() {
        return Err(format!(
            "目录不像库数据目录（未找到 library.db），拒绝删除：{db_dir}"
        ));
    }
    {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        if let Some(active) = settings.active_library() {
            if active.db_dir == db_dir {
                return Err("该库为当前活跃库，请先切换/清除激活再删除".into());
            }
        }
    }
    // 先全量校验再动手（避免库数据已删、照片目录校验又失败的单边半删态）
    if let Some(root) = photo_root {
        let root_path = std::path::PathBuf::from(root);
        if root_path == db_path {
            return Err("照片目录与库数据目录相同，拒绝删除".into());
        }
        // 盘根（如 Y:\、C:\）parent 为空——防误删整盘；相对单段路径同样拒
        let root_parent_empty = root_path
            .parent()
            .map(|p| p.as_os_str().is_empty())
            .unwrap_or(true);
        if root_parent_empty {
            return Err(format!("拒绝删除盘根/裸目录：{}", root_path.display()));
        }
    }
    let mut result = LibraryDeleteResult {
        db_deleted: false,
        photo_root_deleted: false,
    };
    std::fs::remove_dir_all(&db_path)
        .map_err(|e| format!("删除库数据目录失败（{}）：{e}", db_path.display()))?;
    result.db_deleted = true;
    if let Some(root) = photo_root {
        let root_path = std::path::PathBuf::from(root);
        if root_path.is_dir() {
            std::fs::remove_dir_all(&root_path)
                .map_err(|e| format!("删除照片目录失败（{}）：{e}", root_path.display()))?;
            result.photo_root_deleted = true;
        }
    }
    Ok(result)
}

/// 删除库（物理）：库数据目录必删；勾选连照片目录一起删。注册表移除由
/// 前端在调用前后清（先清 activeLibraryId 再删，再从 libraries 摘除）。
#[tauri::command]
pub async fn library_delete(
    state: State<'_, SharedState>,
    db_dir: String,
    photo_root: Option<String>,
) -> Result<LibraryDeleteResult, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_library_delete(&state, &db_dir, photo_root.as_deref())
    })
    .await
}

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
    // AI 索引参数投影（推理层即时读新值；use_gpu 关掉时新会话回落纯 CPU——
    // 存量会话重启后生效，v1 不做会话驱逐）
    state.ai.set_ai_params(crate::ai::AiIndexParams {
        embed_input_size: settings.ai.embed_input_size,
        face_detect_threshold: settings.ai.face_detect_threshold,
        face_cluster_threshold: settings.ai.face_cluster_threshold,
        use_gpu: settings.ai.use_gpu,
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
