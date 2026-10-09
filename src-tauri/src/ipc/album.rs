//! album 命令（M9 相册：纯引用照片组）。相册只引用全局图库资产（多对多，
//! 同一照片可入多相册），一切相册操作只改 album_item 引用——绝不动物理
//! 文件，相册/子组无物理足迹（2026-10-09 单数据库多照片库 §一：变更归属
//! 零文件操作；旧物理目录化/归册挪移/子组物理段随纯时间布局退役）。
//! 全部走 app_database_db + run_blocking（铁律：DB 查询不上主线程）；
//! 载荷 camelCase（AlbumRow 即 IPC DTO，PersonRow 同款复用）。相册内时间线
//! 复用 assets_page 的 keyset 机制与 AssetFilters（album_assets_page 把
//! filters.album_id 覆写为目标相册后透传，与全局分页同一 DTO/别名归一）。

use tauri::State;

use super::{run_blocking, SharedState};
pub use crate::db::AlbumRow;
use crate::db::AssetFilters;

/// AlbumDto 的 Rust 面别名（serde 输出 `{id, name, coverAssetId,
/// itemCount, createdAt}`）。
pub type AlbumDto = AlbumRow;

/// 相册名归一：trim 后空串拒绝（错误文案直接面向用户）。
fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("相册名不能为空".into());
    }
    Ok(name.to_string())
}

/// 相册写错误归一：同名唯一约束 → 重名文案；行不存在 → 相册不存在文案；
/// 其余透传。
fn map_album_write_error(error: rusqlite::Error, name: &str) -> String {
    if matches!(
        &error,
        rusqlite::Error::SqliteFailure(ffi_err, _)
            if ffi_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
    ) {
        format!("同名相册已存在：{name}")
    } else {
        map_missing(error)
    }
}

/// QueryReturnedNoRows（相册不存在）转文案；其余透传。
fn map_missing(error: rusqlite::Error) -> String {
    match error {
        rusqlite::Error::QueryReturnedNoRows => "相册不存在".into(),
        other => other.to_string(),
    }
}

/// 相册列表核（createdAt DESC；空相册合法返回）。
pub fn fetch_album_list(state: &super::AppState) -> Result<Vec<AlbumDto>, String> {
    let db = super::app_database_db(state)?;
    db.album_list().map_err(|e| e.to_string())
}

/// 资产所属相册反查核（查看器详情「所属相册」行；未入册返回空）。
pub fn fetch_asset_albums(state: &super::AppState, asset_id: i64) -> Result<Vec<AlbumDto>, String> {
    let db = super::app_database_db(state)?;
    db.asset_albums(asset_id).map_err(|e| e.to_string())
}

/// 建相册核：trim / 空拒绝；重名报错。
pub fn fetch_album_create(state: &super::AppState, name: &str) -> Result<AlbumDto, String> {
    let name = validate_name(name)?;
    let db = super::app_database_db(state)?;
    db.album_create(&name)
        .map_err(|e| map_album_write_error(e, &name))
}

/// 相册重命名核：trim / 空拒绝；重名报错；相册不存在报错。
pub fn fetch_album_rename(state: &super::AppState, id: i64, name: &str) -> Result<(), String> {
    let name = validate_name(name)?;
    let db = super::app_database_db(state)?;
    db.album_rename(id, &name).map_err(|e| match e {
        rusqlite::Error::InvalidParameterName(msg) => msg,
        other => map_album_write_error(other, &name),
    })
}

/// 删相册核：一照一册模型下相册是照片的唯一归属——删册前先把全部成员
/// 移入回收站（软删；原册已不存在时，还原会落入默认相册「未分组」），
/// 再删相册行（album_item 引用级联消失）。相册不存在报错（前端列表刷新
/// 前的竞态显式暴露）；默认相册「未分组」拒删（db 层守卫，此处转友好文案）。
pub fn fetch_album_delete(state: &super::AppState, id: i64) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    let members: Vec<i64> = {
        let mut stmt =
            db.0.prepare("SELECT asset_id FROM album_item WHERE album_id = ?1")
                .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([id], |r| r.get::<_, i64>(0))
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };
    if !members.is_empty() {
        db.assets_trash_move(&members).map_err(|e| e.to_string())?;
    }
    db.album_delete(id).map_err(|e| match e {
        rusqlite::Error::InvalidParameterName(msg) => msg,
        other => map_missing(other),
    })
}

/// 设/清封面核：asset_id=None 清回默认；资产不存在明确报错（FK 兜底之上的
/// 友好文案）；相册不存在报错。封面只是引用，不要求资产在册相册。
pub fn fetch_album_cover_set(
    state: &super::AppState,
    id: i64,
    asset_id: Option<i64>,
) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    if let Some(asset) = asset_id {
        let exists = db.asset_by_id(asset).map_err(|e| e.to_string())?.is_some();
        if !exists {
            return Err(format!("封面资产不存在：{asset}"));
        }
    }
    db.album_cover_set(id, asset_id).map_err(map_missing)
}

/// 批量入册核：幂等（已存在仅在显式给子分组时改写归属），返回实际新增数
/// （重复/失效 id 不计）。`subgroup` = 0019 子分组命名层（None = 相册根）。
pub fn fetch_album_add_assets(
    state: &super::AppState,
    id: i64,
    asset_ids: &[i64],
    subgroup: Option<&str>,
) -> Result<u64, String> {
    let subgroup = subgroup.map(str::trim).filter(|s| !s.is_empty());
    let db = super::app_database_db(state)?;
    db.album_add_assets(id, asset_ids, subgroup)
        .map_err(map_missing)
}

/// 相册子分组清单核（0019）：DISTINCT subgroup + 计数，name 升序。
pub fn fetch_album_subgroups(
    state: &super::AppState,
    id: i64,
) -> Result<Vec<AlbumSubgroupDto>, String> {
    let db = super::app_database_db(state)?;
    if !db.album_exists(id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    Ok(db
        .album_subgroups(id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(name, item_count)| AlbumSubgroupDto { name, item_count })
        .collect())
}

/// 相册内挪子分组核（0019；2026-10-09 逻辑化）：**纯引用改写**——只改
/// `album_item.subgroup`（None = 挪回相册根），零文件操作（§一：相册/
/// 子组无物理足迹）。不在该相册的 id 自然不命中（0 行）。返回改写行数。
pub fn fetch_album_item_move_subgroup(
    state: &super::AppState,
    id: i64,
    asset_ids: &[i64],
    subgroup: Option<&str>,
) -> Result<u64, String> {
    let subgroup = subgroup.map(str::trim).filter(|s| !s.is_empty());
    let db = super::app_database_db(state)?;
    db.album_item_move_subgroup(id, asset_ids, subgroup)
        .map_err(map_missing)
}

/// 相册内时间线核：按 captured_at 复用画廊 keyset 分页（NULL 最先 + 时间
/// 降序 + id tiebreak）；`filters` 透传现有 AssetFilters 与相册集合求交——
/// 实现为覆写 filters.album_id 后走全局 fetch_assets_page（DTO/别名归一、
/// 日期过滤归一、burstCount 装配全共享）。相册不存在报错。
pub fn fetch_album_assets_page(
    state: &super::AppState,
    id: i64,
    after_id: i64,
    limit: u32,
    filters: Option<AssetFilters>,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::app_database_db(state)?;
    if !db.album_exists(id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    drop(db);
    let mut filters = filters.unwrap_or_default();
    filters.album_id = Some(id);
    super::assets::fetch_assets_page(state, after_id, limit, filters)
}

/// 相册列表（createdAt DESC）。
#[tauri::command]
pub async fn album_list(state: State<'_, SharedState>) -> Result<Vec<AlbumDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_album_list).await
}

/// 资产所属相册反查（查看器详情「所属相册」行）。
#[tauri::command]
pub async fn asset_albums(
    state: State<'_, SharedState>,
    asset_id: i64,
) -> Result<Vec<AlbumDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_asset_albums(state, asset_id)).await
}

/// 建相册（重名/空名报错），返回新相册。
#[tauri::command]
pub async fn album_create(state: State<'_, SharedState>, name: String) -> Result<AlbumDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_album_create(state, &name)).await
}

/// 相册重命名（trim / 空拒绝；重名报错）。
#[tauri::command]
pub async fn album_rename(
    state: State<'_, SharedState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_album_rename(state, id, &name)).await
}

/// 删相册（只删引用，绝不动物理文件）。
#[tauri::command]
pub async fn album_delete(state: State<'_, SharedState>, id: i64) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_album_delete(state, id)).await
}

/// 设/清相册封面（asset_id=null 清回默认）。
#[tauri::command]
pub async fn album_cover_set(
    state: State<'_, SharedState>,
    id: i64,
    asset_id: Option<i64>,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_cover_set(state, id, asset_id)
    })
    .await
}

/// 批量入册（幂等），返回实际新增数。
#[tauri::command]
pub async fn album_add_assets(
    state: State<'_, SharedState>,
    id: i64,
    asset_ids: Vec<i64>,
    subgroup: Option<String>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_add_assets(state, id, &asset_ids, subgroup.as_deref())
    })
    .await
}

/// 相册子分组清单（DISTINCT + 计数，name 升序）。
#[tauri::command]
pub async fn album_subgroups(
    state: State<'_, SharedState>,
    id: i64,
) -> Result<Vec<AlbumSubgroupDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_album_subgroups(state, id)).await
}

/// 相册内挪子分组（None = 挪回根），返回改写行数。
#[tauri::command]
pub async fn album_item_move_subgroup(
    state: State<'_, SharedState>,
    id: i64,
    asset_ids: Vec<i64>,
    subgroup: Option<String>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_item_move_subgroup(state, id, &asset_ids, subgroup.as_deref())
    })
    .await
}

/// 子分组条目 DTO（0019，camelCase）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSubgroupDto {
    pub name: String,
    pub item_count: u64,
}

/// 相册内时间线（captured_at keyset 分页 + filters 求交）。
#[tauri::command]
pub async fn album_assets_page(
    state: State<'_, SharedState>,
    id: i64,
    after_id: i64,
    limit: u32,
    filters: Option<AssetFilters>,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_assets_page(state, id, after_id, limit, filters)
    })
    .await
}
