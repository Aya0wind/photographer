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
    /// 显示分数 [0,1]：原始 cos 经 calibrated_display_score 线性拉伸
    /// （SigLIP2 窄带 0.04-0.125 → 0-1）。阈值过滤仍用原始分数。
    pub score: f32,
}

/// 语义检索阈值合成：显式参数优先，未传回落设置项（ai.semantic_min_score；
/// null = auto——按当前画质档位的语义模型变体取默认：int8 = 0.09、
/// fp16 = 见 ai::semantic_default_min_score）。
/// 不设阈值时任何查询都返回 top-N≈全库（119 张库 limit=100 →「进哪个智能
/// 相册都是全部照片」真机实测复现）。设置 0 = 不过滤。
pub fn effective_min_score(
    explicit: Option<f32>,
    settings_value: Option<f64>,
    tier: crate::ai::QualityTier,
) -> Option<f32> {
    let from_settings = settings_value
        .map(|v| v as f32)
        .unwrap_or_else(|| crate::ai::semantic::semantic_default_min_score(tier));
    explicit.or(Some(from_settings))
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
        let [visual, text] = crate::ai::semantic_model_ids(state.ai.ai_params().quality_tier);
        return Err(format!(
            "语义检索模型未下载（{visual} / {text} / siglip2-tokenizer，             请先在设置页下载）"
        ));
    }
    let db_dir = super::app_database_dir(state)?;
    let db = super::open_library_db(&db_dir)?;
    let (settings_value, tier) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        (
            settings.ai.semantic_min_score,
            crate::ai::QualityTier::from_setting(&settings.ai.quality_tier).unwrap_or_default(),
        )
    };
    let hits = crate::ai::semantic::search(
        &db_dir,
        &db,
        &state.ai,
        query,
        limit.clamp(1, 100),
        effective_min_score(min_score, settings_value, tier),
    )?;
    Ok(hits
        .into_iter()
        .map(|(asset_id, score)| SearchHitDto {
            asset_id,
            score: crate::ai::semantic::calibrated_display_score(score, tier),
        })
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

/// 模型就绪后读取当前库及设置，触发回填并补跑参数指纹比对。
/// 三类模型共用 20 分钟等待窗口；各入口保留模型门槛和开关语义。

/// 模型下载完成后的语义索引自动触发：轮询三件套就绪（后台下载完成）→
/// enable_clip 时派语义回填 + 补跑参数指纹比对。轮询上限 20 分钟。
/// 指纹补跑（2026-09-28 三档画质）：切到 accurate 时若 fp16 件未装，
/// check_params_and_rebuild 会被 ready 门槛挡下且**不写 marker**——件装好
/// 后这里补跑一次，让「切档 → 提示下载 → 装好 → 自动重建+回填」闭环
/// （marker 未动，补跑时指纹仍是旧值 → 正常触发重建；无变更则 no-op）。

/// 内置模型清单状态（installed/state/downloadedBytes…）。
#[tauri::command]
pub async fn ai_models_status(
    state: State<'_, SharedState>,
) -> Result<Vec<ModelStatusDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| Ok(state.ai.status_all())).await
}

/// 发起模型下载（同模型去重；断点续传 + SHA256 校验 + 镜像回退，结果经
/// aiModelDownloadFinished 事件）。三档画质件（scrfd-10g / siglip2-*-fp16）
/// 沿 feature 既有钩子：face 件装好触发人脸回填，语义件装好触发语义回填。
#[tauri::command]
pub async fn ai_model_download(state: State<'_, SharedState>, id: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared.clone(), move |state| {
        let entry = crate::ai::catalog()
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("未知模型: {id}"))?;
        state.ai.download(entry)
    })
    .await?;
    crate::index::scheduler::models_changed();
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
    let result = run_blocking(shared, move |state| state.ai.delete(&id)).await;
    crate::index::scheduler::models_changed();
    result
}

/// 一键清除人脸数据核：faces + people 清空、face 通道任务清空、
/// assets.face_indexed_at 复位（可重新回填）+ 簇心缓存失效。
pub fn fetch_face_data_clear(state: &super::AppState) -> Result<bool, String> {
    let db_dir = super::app_database_dir(state)?;
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
