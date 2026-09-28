//! album 命令（M9 相册：纯引用照片组）。相册只引用全局图库资产（多对多，
//! 同一照片可入多相册），一切相册删除只删 album_item 引用——绝不动物理
//! 文件，也无相册物理目录。全部走 active_library_db + run_blocking（铁律：
//! DB 查询不上主线程）；载荷 camelCase（AlbumRow 即 IPC DTO，PersonRow 同款
//! 复用）。相册内时间线复用 assets_page 的 keyset 机制与 AssetFilters
//! （album_assets_page 把 filters.album_id 覆写为目标相册后透传，与全局
//! 分页同一 DTO/别名归一）。

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
    let db = super::active_library_db(state)?;
    db.album_list().map_err(|e| e.to_string())
}

/// 资产所属相册反查核（查看器详情「所属相册」行；未入册返回空）。
pub fn fetch_asset_albums(state: &super::AppState, asset_id: i64) -> Result<Vec<AlbumDto>, String> {
    let db = super::active_library_db(state)?;
    db.asset_albums(asset_id).map_err(|e| e.to_string())
}

/// 建相册核：trim / 空拒绝；重名报错。
pub fn fetch_album_create(state: &super::AppState, name: &str) -> Result<AlbumDto, String> {
    let name = validate_name(name)?;
    let db = super::active_library_db(state)?;
    db.album_create(&name)
        .map_err(|e| map_album_write_error(e, &name))
}

/// 相册重命名核：trim / 空拒绝；重名报错；相册不存在报错。
pub fn fetch_album_rename(state: &super::AppState, id: i64, name: &str) -> Result<(), String> {
    let name = validate_name(name)?;
    let db = super::active_library_db(state)?;
    db.album_rename(id, &name).map_err(|e| match e {
        rusqlite::Error::InvalidParameterName(msg) => msg,
        other => map_album_write_error(other, &name),
    })
}

/// 删相册核：只删引用（album_item 级联消失），资产与物理文件绝不动；
/// 相册不存在报错（前端列表刷新前的竞态显式暴露）；默认相册「未分组」
/// 拒删（db 层守卫，此处转友好文案）。
pub fn fetch_album_delete(state: &super::AppState, id: i64) -> Result<(), String> {
    let db = super::active_library_db(state)?;
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
    let db = super::active_library_db(state)?;
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
    let db = super::active_library_db(state)?;
    db.album_add_assets(id, asset_ids, subgroup)
        .map_err(map_missing)
}

/// 相册子分组清单核（0019）：DISTINCT subgroup + 计数，name 升序。
pub fn fetch_album_subgroups(
    state: &super::AppState,
    id: i64,
) -> Result<Vec<AlbumSubgroupDto>, String> {
    let db = super::active_library_db(state)?;
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

/// 相册内挪子分组核（0019；0022 物理化）：**物理挪移**——源 = 当前
/// `assets.path`，目标 = [`crate::db::Db::album_item_home_rel`] 公式段
/// （subgroup None = 相册根），同卷 rename / 跨卷 copy+xxhash 校验+删源，
/// XMP 边车随行（与 claim 挪移同款语义，复用其 [`super::claim`] 工具）。
/// 先物理后账本：挪移成功的行才改 `album_item.subgroup` + `assets.path`
/// （filename 随冲突后缀变化）；**失败的行不更新 DB**（整批不回滚）。
/// 已在目标目录（如子组名净化后同段）只补账本；不在该相册的 id 自然
/// 不命中（0 行，与纯 DB 时代语义一致）。返回成功改写行数。
pub fn fetch_album_item_move_subgroup(
    state: &super::AppState,
    id: i64,
    asset_ids: &[i64],
    subgroup: Option<&str>,
) -> Result<u64, String> {
    let subgroup = subgroup.map(str::trim).filter(|s| !s.is_empty());
    let db = super::active_library_db(state)?;
    if !db.album_exists(id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    if asset_ids.is_empty() {
        return Ok(0);
    }
    let home_rel = db
        .album_item_home_rel(id, subgroup)
        .map_err(|e| e.to_string())?
        .ok_or("相册不存在")?;
    let photo_root = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?
        .photo_root;
    // home_rel 段分隔符归一为平台原生（库内两种形态前缀判定均兼容——见
    // claim 的 norm_sep）
    let dst_dir = std::path::Path::new(&photo_root)
        .join(home_rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    let dst_dir_norm = super::claim::norm_sep(&dst_dir.to_string_lossy());

    let mut moved: u64 = 0;
    let mut failures: Vec<String> = Vec::new();
    for asset_id in asset_ids.iter().copied() {
        let fail = |failures: &mut Vec<String>, error: String| {
            failures.push(format!("资产 {asset_id}: {error}"));
        };
        // 只处理本相册在册行（旧纯 DB UPDATE 的命中语义）；顺带取当前
        // 子组判定是否需要物理挪移
        let row: Option<(String, String, i64, i64, u64)> =
            db.0.query_row(
                "SELECT a.path, a.filename, a.in_trash, a.xxhash, a.size \
                 FROM album_item i JOIN assets a ON a.id = i.asset_id \
                 WHERE i.album_id = ?1 AND i.asset_id = ?2",
                rusqlite::params![id, asset_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, i64>(3)?,
                        r.get::<_, i64>(4)? as u64,
                    ))
                },
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(|e| e.to_string())?;
        let Some((path, filename, in_trash, xxhash, size)) = row else {
            continue; // 不在册：不命中（0 行语义），静默跳过
        };
        let src = std::path::PathBuf::from(&path);
        let src_dir_norm = src
            .parent()
            .map(|p| super::claim::norm_sep(&p.to_string_lossy()))
            .unwrap_or_default();
        if src_dir_norm != dst_dir_norm {
            // 物理挪移（根↔子组A↔子组B 任意向）：先物理后账本
            if in_trash != 0 {
                fail(&mut failures, "资产在回收站，不能挪移".into());
                continue;
            }
            if !src.is_file() {
                fail(&mut failures, format!("源文件不在盘：{path}"));
                continue;
            }
            let dst = super::claim::resolve_conflict(&dst_dir, &filename);
            if let Err(e) = super::claim::move_file_with_sidecar(&src, &dst, size, xxhash as u64) {
                fail(&mut failures, e);
                continue; // 物理失败：该行账本不动
            }
            let new_name = dst
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| filename.clone());
            if let Err(e) = db.asset_update_path(asset_id, &dst.to_string_lossy(), &new_name) {
                fail(&mut failures, format!("库路径更新失败: {e}"));
                continue;
            }
        }
        // 账本半边：subgroup 改写（None = 挪回根）
        if let Err(e) = db.album_item_move_subgroup(id, &[asset_id], subgroup) {
            fail(&mut failures, format!("子分组改写失败: {e}"));
            continue;
        }
        moved += 1;
    }
    if !failures.is_empty() {
        let summary = failures.join("；");
        let _ = db.append_log(
            "warn",
            None,
            &format!("子分组挪移部分失败（成功 {moved}）：{summary}"),
        );
        if moved == 0 {
            return Err(format!("子分组挪移全部失败：{summary}"));
        }
    }
    Ok(moved)
}

/// 批量移除引用核（幂等；相册不存在报错）。
pub fn fetch_album_remove_assets(
    state: &super::AppState,
    id: i64,
    asset_ids: &[i64],
) -> Result<(), String> {
    let db = super::active_library_db(state)?;
    db.album_remove_assets(id, asset_ids).map_err(map_missing)
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
    let db = super::active_library_db(state)?;
    if !db.album_exists(id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    drop(db);
    let mut filters = filters.unwrap_or_default();
    filters.album_id = Some(id);
    super::assets::fetch_assets_page(state, after_id, limit, filters)
}

/// 受控改目录核（0018）：只动相册主目录的**最后一段** dir_name——新布局
/// （2026-09-28）父目录 = 创建年月目录（`photoRoot/{创建YYYY}/{创建MM}/`），
/// 物理 rename `…/{old}` → `…/{new}`（同父恒同卷）+ DB 库内资产路径前缀
/// 批量改写 + album.dir_name 更新。
/// - 新目录名经 [`crate::db::sanitize_dir_name`] 净化；与其他相册目录重名
///   拒绝（album.dir_name 全库 UNIQUE，即便不同创建年月父目录不撞也沿用）；
///   目标目录已存在拒绝。
/// - 旧目录不在盘（从未有相册导入/已被外部搬走）→ 只做 DB 改写（记 warn）。
/// - XMP 边车随目录整体移动（同目录文件）；缩略图缓存键 = (path, mtime)，
///   路径变更后自然失效重生成。
pub fn fetch_album_dir_rename(
    state: &super::AppState,
    id: i64,
    new_dir_name: &str,
) -> Result<String, String> {
    let new_dir = crate::db::sanitize_dir_name(new_dir_name);
    let db = super::active_library_db(state)?;
    let old_dir = db
        .album_dir_name(id)
        .map_err(|e| e.to_string())?
        .ok_or("相册不存在")?;
    if old_dir == new_dir {
        return Ok(new_dir);
    }
    let taken: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album WHERE dir_name = ?1 AND id != ?2",
            rusqlite::params![new_dir, id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if taken > 0 {
        return Err(format!("目录名已被其他相册占用：{new_dir}"));
    }
    let photo_root = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?
        .photo_root;
    // 主目录 = photoRoot/{创建YYYY}/{创建MM}/{dir_name}（统一公式；段分隔符
    // 归一为平台原生——路径改写的旧前缀匹配兼容库内 `\`/`/` 双形态）
    let old_path = std::path::Path::new(&photo_root).join(
        db.album_home_rel(id)
            .map_err(|e| e.to_string())?
            .ok_or("相册不存在")?
            .replace('/', std::path::MAIN_SEPARATOR_STR),
    );
    let new_path = old_path.with_file_name(&new_dir);
    if new_path.exists() {
        return Err(format!("目标目录已存在：{}", new_path.display()));
    }
    let old_prefix = format!("{}{}", old_path.display(), std::path::MAIN_SEPARATOR);
    let new_prefix = format!("{}{}", new_path.display(), std::path::MAIN_SEPARATOR);
    if old_path.is_dir() {
        std::fs::rename(&old_path, &new_path).map_err(|e| format!("目录改名失败: {e}"))?;
    } else {
        let _ = db.append_log(
            "warn",
            None,
            &format!(
                "相册改目录：旧目录不在盘，仅改写库（{}）",
                old_path.display()
            ),
        );
    }
    let n = db
        .album_rewrite_paths(id, &new_dir, &old_prefix, &new_prefix)
        .map_err(|e| e.to_string())?;
    let _ = db.append_log(
        "info",
        None,
        &format!("相册改目录：{old_dir} → {new_dir}（改写 {n} 条资产路径）"),
    );
    Ok(new_dir)
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

/// 批量移除引用（幂等；只删引用不动资产）。
#[tauri::command]
pub async fn album_remove_assets(
    state: State<'_, SharedState>,
    id: i64,
    asset_ids: Vec<i64>,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_remove_assets(state, id, &asset_ids)
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

/// 受控改相册物理目录（净化后的目录名返回给前端刷新）。
#[tauri::command]
pub async fn album_dir_rename(
    state: State<'_, SharedState>,
    id: i64,
    new_dir_name: String,
) -> Result<String, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_dir_rename(state, id, &new_dir_name)
    })
    .await
}
