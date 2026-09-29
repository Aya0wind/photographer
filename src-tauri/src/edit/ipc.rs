//! 基础编辑与导出命令（阶段 D）：配方存取三命令 + 导出启动一命令。
//!
//! 命令实现放 `edit::ipc`（而非 `ipc::edit`）：tests/common 脚手架按
//! `#[path]` 把 `ipc` 编进各测试 crate，若 ipc 反向依赖 `crate::edit`，
//! 所有既有测试文件的根部再导出清单都得补 `edit`——命令壳放被依赖方
//! （edit）一侧，依赖方向保持 ipc ← edit 单向。写法与 ipc/* 完全同构
//! （async + run_blocking + camelCase DTO）。
//!
//! 契约（与前端 lane 共同遵守，字段名不得偏移）：
//! - `edit_recipe_get(assetId)` → `{ recipe: Value|null, updatedAt: String|null }`；
//! - `edit_recipe_save(assetId, recipe)`：服务端校验（version==1、数值夹取
//!   0..1、非法报错）后存**归一化 JSON 文本**，回显同 DTO；
//! - `edit_recipe_delete(assetId)` → `()`；
//! - `export_run(assetId, recipe, options)` → `ExportTaskDto`：建库内任务
//!   （queued）后台执行，立刻返回；进度/收尾走 EventBus 既有任务事件通道
//!   （app://event：exportTaskProgress / exportTaskFinished）。
//!
//! 全部 async + run_blocking（铁律：DB/IO 不上主线程）；asset_id 以字符串
//! 传入（前端契约），服务端解析为 i64。

use serde::{Deserialize, Serialize};
use tauri::State;

use super::export::{self, ExportJobRequest, ExportOptions, ExportTaskDto};
use super::recipe;

use crate::ipc::{run_blocking, SharedState};

/// 配方状态 DTO（get/save 共用返回）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditRecipeStateDto {
    /// 归一化配方（get 无配方时 null）。
    pub recipe: Option<serde_json::Value>,
    /// RFC3339（get 无配方时 null）。
    pub updated_at: Option<String>,
}

/// 字符串 asset_id → i64（契约：前端传字符串 id）。
fn parse_asset_id(asset_id: &str) -> Result<i64, String> {
    asset_id
        .trim()
        .parse::<i64>()
        .map_err(|_| format!("资产 id 非法: {asset_id}"))
}

/// Unix 毫秒 → RFC3339。
fn millis_to_rfc3339(millis: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis)
        .unwrap_or_else(chrono::Utc::now)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 配方读取核（返回归一化后的 JSON + 更新时间）。
pub fn fetch_edit_recipe(db: &crate::db::Db, asset_id: i64) -> Result<EditRecipeStateDto, String> {
    match db.edit_recipe_get(asset_id).map_err(|e| e.to_string())? {
        Some((text, updated_at)) => {
            let value = serde_json::from_str(&text).map_err(|e| format!("存库配方损坏: {e}"))?;
            Ok(EditRecipeStateDto {
                recipe: Some(value),
                updated_at: Some(millis_to_rfc3339(updated_at)),
            })
        }
        None => Ok(EditRecipeStateDto {
            recipe: None,
            updated_at: None,
        }),
    }
}

/// 配方保存核：校验/夹取 → 归一化文本落库 → 回显。
pub fn fetch_edit_recipe_save(
    db: &crate::db::Db,
    asset_id: i64,
    value: &serde_json::Value,
) -> Result<EditRecipeStateDto, String> {
    if db
        .asset_by_id(asset_id)
        .map_err(|e| e.to_string())?
        .is_none()
    {
        return Err(format!("资产 {asset_id} 不存在"));
    }
    let normalized = recipe::parse_recipe(value)?;
    let text = serde_json::to_string(&normalized).map_err(|e| format!("配方序列化失败: {e}"))?;
    let updated_at = chrono::Utc::now().timestamp_millis();
    db.edit_recipe_upsert(asset_id, &text, updated_at)
        .map_err(|e| e.to_string())?;
    Ok(EditRecipeStateDto {
        recipe: Some(serde_json::to_value(&normalized).map_err(|e| e.to_string())?),
        updated_at: Some(millis_to_rfc3339(updated_at)),
    })
}

/// 配方删除核（幂等）。
pub fn fetch_edit_recipe_delete(db: &crate::db::Db, asset_id: i64) -> Result<(), String> {
    db.edit_recipe_delete(asset_id).map_err(|e| e.to_string())?;
    Ok(())
}

/// 导出启动核：孤儿收尸 → 校验 → 建 queued 任务 → supervisor 后台执行。
pub fn fetch_export_run(
    state: &crate::ipc::AppState,
    asset_id: i64,
    value: &serde_json::Value,
    options: &ExportOptions,
) -> Result<ExportTaskDto, String> {
    let db = crate::ipc::active_library_db(state)?;
    export::reap_orphan_jobs(&db);
    let asset = db
        .asset_by_id(asset_id)
        .map_err(|e| e.to_string())?
        .ok_or(format!("资产 {asset_id} 不存在"))?;
    let recipe = recipe::parse_recipe(value)?;
    let validated = export::validate_options(&db, options)?;
    let job_id = db
        .export_job_create(asset_id, validated.mode.as_str())
        .map_err(|e| e.to_string())?;

    // worker 自带库连接 + 事件总线（后台任务资源所有权规则：线程内获取释放）
    let (db_dir, photo_root) = {
        let library = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .cloned()
            .ok_or("尚未创建库")?;
        (
            std::path::PathBuf::from(&library.db_dir),
            std::path::PathBuf::from(&library.photo_root),
        )
    };
    let bus = state.bus.clone();
    let request = ExportJobRequest {
        job_id,
        asset_id,
        asset,
        recipe,
        options: validated,
        photo_root,
    };
    state
        .supervisor
        .spawn(
            "export",
            format!("job-{job_id}"),
            move |_| match crate::ipc::open_library_db(&db_dir) {
                Ok(worker_db) => export::run_export_job(worker_db, &bus, request),
                Err(error) => eprintln!("导出任务 {job_id} 无法打开库: {error}"),
            },
        );
    db.export_job_get(job_id)
        .map_err(|e| e.to_string())?
        .map(ExportTaskDto::from)
        .ok_or_else(|| "导出任务建档后无法读回".to_string())
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + spawn_blocking）
// ---------------------------------------------------------------------------

/// 读取资产编辑配方。
#[tauri::command]
pub async fn edit_recipe_get(
    state: State<'_, SharedState>,
    asset_id: String,
) -> Result<EditRecipeStateDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let id = parse_asset_id(&asset_id)?;
        let db = crate::ipc::active_library_db(state)?;
        fetch_edit_recipe(&db, id)
    })
    .await
}

/// 保存（校验/夹取后）资产编辑配方，返回归一化结果。
#[tauri::command]
pub async fn edit_recipe_save(
    state: State<'_, SharedState>,
    asset_id: String,
    recipe: serde_json::Value,
) -> Result<EditRecipeStateDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let id = parse_asset_id(&asset_id)?;
        let db = crate::ipc::active_library_db(state)?;
        fetch_edit_recipe_save(&db, id, &recipe)
    })
    .await
}

/// 删除资产编辑配方（幂等）。
#[tauri::command]
pub async fn edit_recipe_delete(
    state: State<'_, SharedState>,
    asset_id: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let id = parse_asset_id(&asset_id)?;
        let db = crate::ipc::active_library_db(state)?;
        fetch_edit_recipe_delete(&db, id)
    })
    .await
}

/// 启动导出（建库内任务后台执行，立刻返回任务 DTO）。
#[tauri::command]
pub async fn export_run(
    state: State<'_, SharedState>,
    asset_id: String,
    recipe: serde_json::Value,
    options: ExportOptions,
) -> Result<ExportTaskDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let id = parse_asset_id(&asset_id)?;
        fetch_export_run(state, id, &recipe, &options)
    })
    .await
}
