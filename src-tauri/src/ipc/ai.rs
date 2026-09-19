//! ai 命令（M4 前置）：模型清单状态 / 下载 / 取消 / 删除。
//!
//! 状态读文件元数据、删除落盘 → 后台线程；下载为登记 + 派发（快），
//! 主体在 TaskSupervisor 线程，进度/结果经事件回报。

use tauri::State;

use super::{run_blocking, SharedState};
use crate::ai::ModelStatusDto;

/// 内置模型清单状态（installed/state/downloadedBytes…）。
#[tauri::command]
pub async fn ai_models_status(
    state: State<'_, SharedState>,
) -> Result<Vec<ModelStatusDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| Ok(state.ai.status_all())).await
}

/// 发起模型下载（同模型去重；断点续传 + SHA256 校验 + 镜像回退，结果经
/// aiModelDownloadFinished 事件）。
#[tauri::command]
pub async fn ai_model_download(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let entry = crate::ai::catalog()
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("未知模型: {id}"))?;
        state.ai.download(entry)
    })
    .await
}

/// 取消下载（清 .part）。
#[tauri::command]
pub async fn ai_model_cancel(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| state.ai.cancel(&id)).await
}

/// 删除已下载模型文件（释放磁盘，installed 翻 false）。
#[tauri::command]
pub async fn ai_model_delete(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| state.ai.delete(&id)).await
}
