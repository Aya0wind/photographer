//! 选片命令（阶段 B1）：颜色标签 / 接受拒绝 / 应用内回收站 / 智能视图。
//!
//! - 颜色标签：DB 权威 + XMP 边车异步写 `xmp:Label`，包括用户主动修改的
//!   外部引用照片；LR 标准色名直映，无映射配置。
//! - 拒绝状态：应用内选片状态；XMP 即时投影为 `xmp:Rating = -1`
//!   （Adobe 业界约定、LR 可识别，用户定案 2026-09-27），星级在 DB 保留；
//!   默认查询不排除已拒绝——只是可筛选项。
//! - 回收站：软删标记（in_trash+trashed_at），常规查询全链路默认排除；
//!   恢复还原可见性；purge 语义 = 2026-10-09 §五 定案——在线库真删本体、
//!   离线库回收站项原样保留 + 总结提示、缺失项仅删记录（不做「强制仅删
//!   记录」）。写方向改动一律同语句置 `xmp_dirty`，缺失/离线期间不派边车
//!   任务，库恢复在线后由库扫描补写清标志（§五 闭环）。
//!
//! 全部走 app_database_db + run_blocking（铁律：DB/磁盘 IO 不上主线程）。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::sidecar::{spawn_xmp_sync, XmpSync};
use super::{run_blocking, SharedState};

/// LR 标准颜色标签（应用内小写 token；xmp:Label 写首字母大写标准色名）。
pub const COLOR_LABELS: &[&str] = &["red", "yellow", "green", "blue", "purple"];

/// 颜色标签归一校验：trim + 小写；None/空串 = 清除。非法色名拒绝。
pub fn validate_label(raw: Option<&str>) -> Result<Option<&'static str>, String> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let token = raw.trim().to_ascii_lowercase();
    if token.is_empty() {
        return Ok(None);
    }
    match COLOR_LABELS.iter().find(|c| **c == token) {
        Some(valid) => Ok(Some(*valid)),
        None => Err(format!(
            "非法颜色标签: {raw}（可选 {}）",
            COLOR_LABELS.join("/")
        )),
    }
}

/// 边车同步目标的在盘性预检 SQL 片段（§五 M2c 写方向闭环）：资产缺失或
/// 所在照片库离线的行不派边车任务——改动已入库并置脏（同语句），库恢复
/// 在线后由库扫描补写（[`crate::scan`]）。library_id 为 NULL 的历史/外部
/// 引用行 status 视同 online（保持旧语义：主动改动照写边车）。
const SIDECAR_TARGET_WHERE: &str = "a.missing = 0 AND COALESCE((SELECT l.status = 'offline' \
     FROM photos_libraries l WHERE l.id = a.library_id), 0) = 0";

/// 批量设颜色标签核：DB 批量更新（同语句置 `xmp_dirty`，§五 M2c）→ 在线
/// 且在盘的行派 XMP 边车同步（成功后清脏；缺失/离线行保持脏标志待库扫描
/// 补写）。supervisor 后台线程，失败经 AppError 事件上报，不阻塞入库。
pub fn fetch_asset_label_set(
    state: &super::AppState,
    asset_ids: &[i64],
    label: Option<&str>,
) -> Result<u64, String> {
    let label = validate_label(label)?;
    let db = super::app_database_db(state)?;
    let n = db
        .assets_label_set(asset_ids, label)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(0);
    }
    // 用户主动修改的照片均同步边车；xmp 值形态 = 首字母大写标准色名。
    // 目标 = 更新行中在线且在盘的（id + path；缺失/离线行走 xmp_dirty 闭环）。
    let targets: Vec<(i64, String, Option<&'static str>)> =
        db.0.prepare(&format!(
            "SELECT a.id, a.path FROM assets a WHERE a.id IN ({}) AND {SIDECAR_TARGET_WHERE}",
            (0..asset_ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .and_then(|mut stmt| {
            let rows = stmt.query_map(rusqlite::params_from_iter(asset_ids.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(id, path)| (id, path, label))
        .collect();
    if targets.is_empty() {
        return Ok(n);
    }
    let db_dir = super::app_database_dir(state)?;
    spawn_xmp_sync(
        state,
        XmpSync {
            task: format!("label-batch-{n}"),
            subject: "颜色标签",
            failure: "同步失败",
        },
        targets,
        move |(asset_id, path, label)| {
            crate::metadata::xmp::sync_label_to_sidecar(
                &PathBuf::from(path),
                label.and_then(crate::metadata::xmp::label_to_xmp),
            )?;
            if let Ok(db) = super::open_library_db(&db_dir) {
                let _ = db.clear_asset_xmp_dirty(asset_id);
            }
            Ok(())
        },
    );
    Ok(n)
}

/// 批量设接受/拒绝核：DB 更新（同语句置 `xmp_dirty`，§五 M2c；缺失/离线
/// 行不派边车任务，走脏标志闭环——见 [`fetch_asset_label_set`]）→ 按投影
/// （用户定案「XMP 即时投影」：边车评分 = rejected ? -1 : rating，LR 可
/// 识别的拒绝表示；星级在 DB 保留，取消拒绝恢复投影）派同步。与
/// rating_set 共用同一投影，任一变更后边车都按 DB 真值重写。批量逐文件；
/// 单文件失败容忍不中断（AppError 上报）；边车缺失时新建，包括用户主动
/// 修改的外部引用照片。
pub fn fetch_asset_reject_set(
    state: &super::AppState,
    asset_ids: &[i64],
    rejected: bool,
) -> Result<u64, String> {
    let db = super::app_database_db(state)?;
    let n = db
        .assets_reject_set(asset_ids, rejected)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(0);
    }
    // 边车投影清单：评分取 DB 当前值做投影（在线且在盘的更新行）。
    let targets: Vec<(i64, String, i8)> =
        db.0.prepare(&format!(
            "SELECT a.id, a.path, a.rating FROM assets a WHERE a.id IN ({}) AND {SIDECAR_TARGET_WHERE}",
            (0..asset_ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .and_then(|mut stmt| {
            let rows = stmt.query_map(rusqlite::params_from_iter(asset_ids.iter()), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(id, path, rating)| (id, path, super::rating::projected_rating(rating, rejected)))
        .collect();
    if targets.is_empty() {
        return Ok(n);
    }
    let db_dir = super::app_database_dir(state)?;
    spawn_xmp_sync(
        state,
        XmpSync {
            task: format!("reject-batch-{n}"),
            subject: "拒绝状态",
            failure: "投影失败",
        },
        targets,
        move |(asset_id, path, projected)| {
            crate::metadata::xmp::sync_rating_to_sidecar(&PathBuf::from(path), projected)?;
            if let Ok(db) = super::open_library_db(&db_dir) {
                let _ = db.clear_asset_xmp_dirty(asset_id);
            }
            Ok(())
        },
    );
    Ok(n)
}

/// 移入回收站核（软删：in_trash=1 + trashed_at；幂等）。返回新移入数。
pub fn fetch_asset_trash_move(state: &super::AppState, asset_ids: &[i64]) -> Result<u64, String> {
    let db = super::app_database_db(state)?;
    db.assets_trash_move(asset_ids).map_err(|e| e.to_string())
}

/// 回收站列表核（trashed_at DESC keyset；复用 AssetDto 契约）。
pub fn fetch_trash_list(
    state: &super::AppState,
    after_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::app_database_db(state)?;
    let rows = db
        .trash_list(after_id, limit.clamp(1, 200))
        .map_err(|e| e.to_string())?;
    let mut dtos: Vec<_> = rows
        .into_iter()
        .map(super::assets::page_row_to_dto)
        .collect();
    super::assets::attach_burst_counts_pub(&db, &mut dtos);
    Ok(dtos)
}

/// 回收站还原核（幂等）。返回还原数。
pub fn fetch_trash_restore(state: &super::AppState, asset_ids: &[i64]) -> Result<u64, String> {
    let db = super::app_database_db(state)?;
    db.trash_restore(asset_ids).map_err(|e| e.to_string())
}

/// 永久删除结果（§五 M2c 清空回收站总结提示；camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashPurgeResult {
    /// 删除的资产记录数（离线库保留项不计）。
    pub deleted_records: u64,
    /// 物理删除的本体文件数（仅在线库 + deleteFiles=true + 非外部引用）。
    pub deleted_files: u64,
    /// 缺失项仅删记录数（本体已不在盘，记录清除收尾）。
    pub missing_records_only: u64,
    /// 离线库原样保留项数（不删文件、不删记录，不做「强制仅删记录」）。
    pub offline_kept: u64,
    /// 涉及的离线库名称（去重；总结提示用）。
    pub offline_libraries: Vec<String>,
}

/// 永久删除核（回收站「清空」动作；2026-10-09 §五 定案语义）：
/// - **在线库**：`delete_files=true` 真删本体（外部引用 origin='external'
///   仍绝不物理删——文件不在库管辖内）；单文件失败容忍、日志记账。
/// - **离线库**：回收站项**原样保留**（不删文件、不删记录）+ 总结提示，
///   不做「强制仅删记录」——库不在线时无法核实文件身份。
/// - **缺失项**（missing=1 / 本体已不在盘）：仅删记录。
/// - DB：只作用于 in_trash=1 的行（防前端陈旧选中误删库内活跃资产），
///   行级联清引用（album_item/faces/view_history/similar_bucket/index_tasks）。
pub fn fetch_trash_purge(
    state: &super::AppState,
    asset_ids: &[i64],
    delete_files: bool,
) -> Result<TrashPurgeResult, String> {
    let db = super::app_database_db(state)?;
    // (id, path, origin, missing, library_id, library_online, library_name)
    let entries = db.trash_entries(asset_ids).map_err(|e| e.to_string())?;
    let mut result = TrashPurgeResult {
        deleted_records: 0,
        deleted_files: 0,
        missing_records_only: 0,
        offline_kept: 0,
        offline_libraries: Vec::new(),
    };
    if entries.is_empty() {
        return Ok(result);
    }
    let mut ids_to_delete: Vec<i64> = Vec::with_capacity(entries.len());
    for (id, path, origin, missing, _library_id, library_online, library_name) in &entries {
        // 离线库：原样保留（§五 定案——不提供「强制仅删记录」）。
        if !library_online {
            result.offline_kept += 1;
            if let Some(name) = library_name {
                if !result.offline_libraries.contains(name) {
                    result.offline_libraries.push(name.clone());
                }
            }
            continue;
        }
        // 在线库：按 delete_files + 缺失/外部引用分类物理删除。
        if delete_files && origin != "external" {
            if *missing {
                // 缺失项仅删记录（§五）：本体已不在盘，记录清除收尾。
                result.missing_records_only += 1;
            } else {
                match std::fs::remove_file(path) {
                    Ok(()) => {
                        result.deleted_files += 1;
                        let _ = db.append_log("info", None, &format!("回收站清除：{path}"));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // 标记未及更新（惰性检测未触发/刚被外部删除）：同缺失项。
                        result.missing_records_only += 1;
                    }
                    Err(e) => {
                        // 删除失败不回滚 DB（计数 + warn 日志；行仍清除——
                        // 与 duplicate_delete 同款失败容忍）。
                        let _ = db.append_log(
                            "warn",
                            None,
                            &format!("回收站清除：文件删除失败 {path}: {e}（库行仍清除）"),
                        );
                    }
                }
            }
        }
        ids_to_delete.push(*id);
    }
    if ids_to_delete.is_empty() {
        return Ok(result);
    }
    result.deleted_records = db
        .assets_delete_rows(&ids_to_delete)
        .map_err(|e| e.to_string())?;
    Ok(result)
}

/// 清理源缺失资产核（2026-09-29 用户功能）：扫描库内活跃（in_trash=0）的
/// photo/raw 资产，源文件已不存在的行永久删除（行级联清引用；物理文件无需
/// 删——源已缺失是清理前提）。日志记账每条；返回删除数。
pub fn fetch_missing_purge(state: &super::AppState) -> Result<u64, String> {
    let db = super::app_database_db(state)?;
    let rows: Vec<(i64, String)> =
        db.0.prepare("SELECT id, path FROM assets WHERE in_trash = 0 AND kind IN ('photo', 'raw')")
            .map_err(|e| e.to_string())?
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
    let missing: Vec<i64> = rows
        .iter()
        .filter(|(_, path)| !std::path::Path::new(path).is_file())
        .map(|(id, path)| {
            let _ = db.append_log("info", None, &format!("清理源缺失资产：{path}"));
            *id
        })
        .collect();
    if missing.is_empty() {
        return Ok(0);
    }
    db.assets_delete_rows(&missing).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + spawn_blocking）
// ---------------------------------------------------------------------------

/// 批量设颜色标签（label=null 清除；非法色名拒绝）。返回更新行数。
#[tauri::command]
pub async fn asset_label_set(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
    label: Option<String>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_asset_label_set(state, &asset_ids, label.as_deref())
    })
    .await
}

/// 批量设接受/拒绝状态。返回更新行数。
#[tauri::command]
pub async fn asset_reject_set(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
    rejected: bool,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_asset_reject_set(state, &asset_ids, rejected)
    })
    .await
}

/// 移入回收站（软删）。返回新移入数。
#[tauri::command]
pub async fn asset_trash_move(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_asset_trash_move(state, &asset_ids)
    })
    .await
}

/// 回收站列表（trashed_at DESC keyset；after_id = 上一页末行 id）。
#[tauri::command]
pub async fn trash_list(
    state: State<'_, SharedState>,
    after_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_trash_list(state, after_id, limit)
    })
    .await
}

/// 回收站还原。返回还原数。
#[tauri::command]
pub async fn trash_restore(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_trash_restore(state, &asset_ids)).await
}

/// 永久删除（§五 M2c：在线库真删本体 / 离线库原样保留 / 缺失项仅删记录；
/// delete_files=false 一律不动文件）。返回总结（离线保留项与库名供前端提示）。
#[tauri::command]
pub async fn trash_purge(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
    delete_files: bool,
) -> Result<TrashPurgeResult, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_trash_purge(state, &asset_ids, delete_files)
    })
    .await
}

#[tauri::command]
pub async fn assets_purge_missing(state: State<'_, SharedState>) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_missing_purge(&state)).await
}
