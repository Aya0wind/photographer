//! index 命令（真机修复 2026-09-19：语义回填触发链缺口）：手动触发索引与
//! 通道状态查询。根因：kick_semantic_if_ready 只挂在「下载完成 watcher /
//! 导入收尾」，存量资产在模型就位前导入则无人补触发——本模块提供 UI 主动
//! 触发入口（index_kick_now）与进度可视化数据（index_status）。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 单通道任务状态计数（camelCase）。total = 可索引资产数（photo/raw，
/// 供 UI 显示 done/total 进度条；thumb/face 通道同口径）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexKindStatus {
    pub pending: u64,
    pub running: u64,
    pub done: u64,
    pub failed: u64,
    pub total: u64,
}

/// 全通道状态（index_status 载荷）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexStatusDto {
    pub thumb: IndexKindStatus,
    pub exif: IndexKindStatus,
    pub ai: IndexKindStatus,
    pub face: IndexKindStatus,
}

const KINDS: [&str; 4] = ["thumb", "exif", "ai", "face"];

/// 状态聚合核：index_tasks 按 (kind, state) 计数 + assets 可索引总数。
pub fn fetch_index_status(state: &super::AppState) -> Result<IndexStatusDto, String> {
    let db = super::active_library_db(state)?;
    let counts = db.index_task_state_counts().map_err(|e| e.to_string())?;
    let total_assets =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE kind IN ('photo', 'raw')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map_err(|e| e.to_string())? as u64;
    let mut by_kind: HashMap<String, [u64; 4]> = HashMap::new();
    for (kind, state_name, count) in counts {
        let slot = match state_name.as_str() {
            "pending" => 0,
            "running" => 1,
            "done" => 2,
            _ => 3,
        };
        by_kind.entry(kind).or_default()[slot] += count;
    }
    let mut dto = IndexStatusDto::default();
    for (field, kind) in [
        (&mut dto.thumb, "thumb"),
        (&mut dto.exif, "exif"),
        (&mut dto.ai, "ai"),
        (&mut dto.face, "face"),
    ] {
        let slots = by_kind.get(kind).copied().unwrap_or_default();
        *field = IndexKindStatus {
            pending: slots[0],
            running: slots[1],
            done: slots[2],
            failed: slots[3],
            total: total_assets,
        };
    }
    Ok(dto)
}

/// 手动触发核：thumb/exif → index::kick（全核跑待办）；ai → 语义回填
/// （模型未就绪 → 明确错误）；face → 人脸回填（同门槛）。幂等：任务生成
/// 侧自带去重（NOT EXISTS pending/running + ai_indexed_at/face_indexed_at
/// 时间账）。
pub fn fetch_index_kick_now(state: &super::AppState, kind: &str) -> Result<(), String> {
    let (enable_clip, enable_face) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        (settings.ai.enable_clip, settings.ai.enable_face)
    };
    let library = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?;
    let db_dir = std::path::PathBuf::from(&library.db_dir);
    let db = super::open_library_db(&db_dir)?;
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    match kind {
        "thumb" | "exif" => {
            db.retry_failed_index_tasks(kind)
                .map_err(|e| format!("重试失败索引任务失败: {e}"))?;
            crate::index::kick(db_dir, &supervisor);
            Ok(())
        }
        "ai" => {
            if !state.ai.semantic_ready() {
                return Err("语义检索模型未下载，请先在设置中下载模型（siglip2-visual / siglip2-text / siglip2-tokenizer）".into());
            }
            if !enable_clip {
                return Err("语义索引未开启（设置 → AI → 语义检索）".into());
            }
            db.retry_failed_index_tasks("ai")
                .map_err(|e| format!("重试失败语义任务失败: {e}"))?;
            crate::ai::semantic::kick_semantic_if_ready(db_dir, &state.ai, &state.bus, &supervisor);
            Ok(())
        }
        "face" => {
            if !state.ai.face_models_ready() {
                return Err("人脸识别模型未下载，请先在设置中下载模型（scrfd / arcface）".into());
            }
            if !enable_face {
                return Err("人脸识别未开启（设置 → AI → 人脸识别）".into());
            }
            db.retry_failed_index_tasks("face")
                .map_err(|e| format!("重试失败人脸任务失败: {e}"))?;
            crate::ai::face::kick_face_if_ready(db_dir, &state.ai, &state.bus, &supervisor);
            Ok(())
        }
        other => Err(format!("未知索引通道: {other}（可选 {KINDS:?}）")),
    }
}

/// 手动触发索引（kind = "thumb" | "exif" | "ai" | "face"；幂等）。
#[tauri::command]
pub async fn index_kick_now(state: State<'_, SharedState>, kind: String) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_index_kick_now(state, &kind)).await
}

/// 索引通道状态（各通道 pending/running/done/failed + 可索引资产总数）。
#[tauri::command]
pub async fn index_status(state: State<'_, SharedState>) -> Result<IndexStatusDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_index_status).await
}
