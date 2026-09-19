//! ai 命令（M4 前置）：模型清单状态 / 下载 / 取消 / 删除。
//!
//! 状态读文件元数据、删除落盘 → 后台线程；下载为登记 + 派发（快），
//! 主体在 TaskSupervisor 线程，进度/结果经事件回报。

use tauri::State;

use super::{run_blocking, SharedState};
use crate::ai::ModelStatusDto;

/// 语义检索命中（camelCase）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHitDto {
    pub asset_id: i64,
    /// cos 相似度（归一化嵌入点积，[-1,1]）。
    pub score: f32,
}

/// 语义检索核（模型未齐 → 明确错误）。
pub fn fetch_search_semantic(
    state: &super::AppState,
    query: &str,
    limit: u32,
    min_score: Option<f32>,
) -> Result<Vec<SearchHitDto>, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("查询不能为空".into());
    }
    if !state.ai.semantic_ready() {
        return Err(
            "语义检索模型未下载（siglip2-visual / siglip2-text / siglip2-tokenizer，             请先在设置页下载）"
                .into(),
        );
    }
    let library = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?;
    let db_dir = std::path::PathBuf::from(&library.db_dir);
    let db = super::open_library_db(&db_dir)?;
    let hits = crate::ai::semantic::search(
        &db_dir,
        &db,
        &state.ai,
        query,
        limit.clamp(1, 100),
        min_score,
    )?;
    Ok(hits
        .into_iter()
        .map(|(asset_id, score)| SearchHitDto { asset_id, score })
        .collect())
}

/// 语义检索（文本查询 → 768 维嵌入 → HNSW KNN → 资产 join）。
#[tauri::command]
pub async fn search_semantic(
    state: State<'_, SharedState>,
    query: String,
    limit: u32,
    min_score: Option<f32>,
) -> Result<Vec<SearchHitDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_search_semantic(state, &query, limit, min_score)
    })
    .await
}

/// 模型下载完成后的语义索引自动触发：轮询三件套就绪（后台下载完成）→
/// enable_clip 时派语义回填。轮询上限 20 分钟。
fn spawn_post_install_watch(state: &SharedState) {
    let shared = std::sync::Arc::clone(state);
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    supervisor.spawn("ai-postinstall", "semantic-watch".into(), move |_| {
        for _ in 0..600 {
            if shared.ai.semantic_ready() {
                let enable_clip = shared
                    .settings
                    .lock()
                    .expect("settings mutex poisoned")
                    .ai
                    .enable_clip;
                if enable_clip {
                    let library = shared
                        .settings
                        .lock()
                        .expect("settings mutex poisoned")
                        .active_library()
                        .cloned();
                    if let Some(library) = library {
                        let db_dir = std::path::PathBuf::from(&library.db_dir);
                        crate::ai::semantic::kick_semantic_if_ready(
                            db_dir,
                            &shared.ai,
                            &shared.bus,
                            &shared.supervisor,
                        );
                    }
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

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
    let is_semantic = id.starts_with("siglip2");
    let is_face = id == "scrfd" || id == "arcface";
    run_blocking(shared.clone(), move |state| {
        let entry = crate::ai::catalog()
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("未知模型: {id}"))?;
        state.ai.download(entry)
    })
    .await?;
    if is_semantic {
        spawn_post_install_watch(&shared);
    }
    if is_face {
        spawn_face_post_install_watch(&shared);
    }
    Ok(())
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

/// 一键清除人脸数据核：faces + people 清空、face 通道任务清空、
/// assets.face_indexed_at 复位（可重新回填）+ 簇心缓存失效。
pub fn fetch_face_data_clear(state: &super::AppState) -> Result<bool, String> {
    let library = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?;
    let db_dir = std::path::PathBuf::from(&library.db_dir);
    let db = super::open_library_db(&db_dir)?;
    db.clear_face_data().map_err(|e| e.to_string())?;
    crate::ai::face::invalidate_cluster_cache(&db_dir);
    Ok(true)
}

/// 一键清除人脸数据（设置页两步强确认后调用；前端 aiFaceDataClear）。
#[tauri::command]
pub async fn ai_face_data_clear(state: State<'_, SharedState>) -> Result<bool, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_face_data_clear).await
}

/// scrfd/arcface 下载完成后的自动触发：轮询两件套就绪 → enable_face 时派
/// 人脸回填。轮询上限 20 分钟。
fn spawn_face_post_install_watch(state: &SharedState) {
    let shared = std::sync::Arc::clone(state);
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    supervisor.spawn("ai-postinstall", "face-watch".into(), move |_| {
        for _ in 0..600 {
            let enable_face = shared
                .settings
                .lock()
                .expect("settings mutex poisoned")
                .ai
                .enable_face;
            if shared.ai.face_models_ready() {
                let library = shared
                    .settings
                    .lock()
                    .expect("settings mutex poisoned")
                    .active_library()
                    .cloned();
                if enable_face {
                    if let Some(library) = library {
                        let db_dir = std::path::PathBuf::from(&library.db_dir);
                        crate::ai::face::kick_face_if_ready(
                            db_dir,
                            &shared.ai,
                            &shared.bus,
                            &shared.supervisor,
                        );
                    }
                }
                return;
            }
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}
