//! assets 命令（M3 画廊数据源）：keyset 分页 / 日期分组 / 资产详情。
//!
//! 排序契约（真机核对 2026-09-19）：NULL captured_at 最先，随后拍摄时间
//! 降序、id 倒序 tiebreak——db 层用 COALESCE 高哨兵归一成单键。DB 查询
//! 走 run_blocking 后台线程（铁律：大结果集不上主线程）。

use tauri::State;

use super::{run_blocking, SharedState};
pub use crate::db::AssetFilters;
use crate::events::AssetKind;

/// 画廊网格条目 DTO（camelCase）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDto {
    pub id: i64,
    /// 绝对路径。
    pub path: String,
    /// 文件名。
    pub name: String,
    /// photo | raw | video | other（`AssetKind` camelCase 序列化）。
    pub kind: AssetKind,
    /// RFC3339；未知为 null（排序时排最前）。
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub size_bytes: u64,
}

/// 日期分组 DTO（画廊吸顶 + 跳转；date 为本地时区 `YYYY-MM-DD`，NULL 归
/// "unknown" 且置顶）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateGroupDto {
    pub date: String,
    pub count: u64,
    pub cover_asset_id: i64,
}

/// 资产详情 DTO：AssetRow 全字段（flatten）+ id + 库内同指纹重复计数
/// （(size, xxh) 相同的**其他**资产数，不含自身）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AssetDetailDto {
    pub id: i64,
    #[serde(flatten)]
    pub asset: crate::db::AssetRow,
    pub duplicate_count: u64,
}

/// RFC3339 过滤值归一：解析后转与库内 captured_at 同构的定宽 UTC 字符串
/// （库内为 `to_rfc3339_opts(Millis, true)`；带时区偏移的入参字典序不可比）。
fn normalize_rfc3339(value: &str, field: &str) -> Result<String, String> {
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .map_err(|_| format!("无效的{field}日期过滤（需 RFC3339）: {value}"))
}

/// keyset 分页：after_id = 上一页末行 id（0 = 第一页；行已删按第一页）。
/// 过滤条件全参数化；任一日期过滤出现时 NULL captured_at 被排除。
pub fn fetch_assets_page(
    state: &super::AppState,
    after_id: i64,
    limit: u32,
    filters: AssetFilters,
) -> Result<Vec<AssetDto>, String> {
    let mut filters = filters;
    if let Some(after) = filters.captured_after.take() {
        filters.captured_after = Some(normalize_rfc3339(&after, "起始")?);
    }
    if let Some(before) = filters.captured_before.take() {
        filters.captured_before = Some(normalize_rfc3339(&before, "结束")?);
    }
    let db = super::active_library_db(state)?;
    let rows = db
        .assets_page(after_id, limit.clamp(1, 200), &filters)
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| AssetDto {
            id: r.id,
            path: r.path,
            name: r.filename,
            kind: r.kind,
            captured_at: r.captured_at,
            camera: r.camera,
            size_bytes: r.size,
        })
        .collect())
}

/// 本地时区日期分组（降序；unknown 组置顶）。
pub fn fetch_asset_group_dates(state: &super::AppState) -> Result<Vec<DateGroupDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.asset_group_dates().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| DateGroupDto {
            date: r.date,
            count: r.count,
            cover_asset_id: r.cover_asset_id,
        })
        .collect())
}

/// 资产详情：全字段 + 同指纹重复计数；不存在返回 None。
pub fn fetch_asset_detail(
    state: &super::AppState,
    id: i64,
) -> Result<Option<AssetDetailDto>, String> {
    let db = super::active_library_db(state)?;
    let Some(asset) = db.asset_by_id(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let duplicate_count = db
        .asset_duplicate_count(id, asset.size, asset.xxhash)
        .map_err(|e| e.to_string())?;
    Ok(Some(AssetDetailDto {
        id,
        asset,
        duplicate_count,
    }))
}

/// 画廊分页（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn assets_page(
    state: State<'_, SharedState>,
    after_id: i64,
    limit: u32,
    filters: AssetFilters,
) -> Result<Vec<AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_assets_page(state, after_id, limit, filters)
    })
    .await
}

/// 日期分组（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn asset_group_dates(state: State<'_, SharedState>) -> Result<Vec<DateGroupDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_asset_group_dates).await
}

/// 资产详情（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn asset_detail(
    state: State<'_, SharedState>,
    id: i64,
) -> Result<Option<AssetDetailDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_asset_detail(state, id)).await
}
