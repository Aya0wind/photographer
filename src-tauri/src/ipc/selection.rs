//! 选片命令（阶段 B1）：颜色标签 / 接受拒绝 / 应用内回收站 / 智能视图。
//!
//! - 颜色标签：DB 权威 + XMP 边车异步写 `xmp:Label`（与 rating 同策略：
//!   外部库只读资产跳过边车；LR 标准色名直映，无映射配置）。
//! - 拒绝状态：应用内选片状态；XMP 即时投影为 `xmp:Rating = -1`
//!   （Adobe 业界约定、LR 可识别，用户定案 2026-09-27），星级在 DB 保留；
//!   默认查询不排除已拒绝——只是可筛选项。
//! - 回收站：软删标记（in_trash+trashed_at），常规查询全链路默认排除；
//!   恢复还原可见性；purge 才动 DB 行与物理文件（外部库绝不物理删）。
//!   JSON 合法性。
//!
//! 全部走 active_library_db + run_blocking（铁律：DB/磁盘 IO 不上主线程）。

use std::path::PathBuf;

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

/// 批量设颜色标签核：DB 批量更新 → 派 XMP 边车同步（supervisor 后台线程，
/// 失败经 AppError 事件上报，不阻塞入库；外部库只读资产跳过边车）。
pub fn fetch_asset_label_set(
    state: &super::AppState,
    asset_ids: &[i64],
    label: Option<&str>,
) -> Result<u64, String> {
    let label = validate_label(label)?;
    let db = super::active_library_db(state)?;
    let n = db
        .assets_label_set(asset_ids, label)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(0);
    }
    // 边车同步清单（库内复制入册的资产；xmp 值形态 = 首字母大写标准色名）
    let targets: Vec<(String, Option<&'static str>)> =
        db.0.prepare(&format!(
            "SELECT path, origin FROM assets WHERE id IN ({})",
            (0..asset_ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .and_then(|mut stmt| {
            let rows = stmt.query_map(rusqlite::params_from_iter(asset_ids.iter()), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, origin)| origin != "external")
        .map(|(path, _)| (path, label))
        .collect();
    if targets.is_empty() {
        return Ok(n);
    }
    spawn_xmp_sync(
        state,
        XmpSync {
            task: format!("label-batch-{n}"),
            subject: "颜色标签",
            failure: "同步失败",
        },
        targets,
        |(path, label)| {
            crate::metadata::xmp::sync_label_to_sidecar(
                &PathBuf::from(path),
                label.and_then(crate::metadata::xmp::label_to_xmp),
            )
        },
    );
    Ok(n)
}

/// 批量设接受/拒绝核：DB 更新 → 派 XMP 边车投影同步（用户定案「XMP 即时
/// 投影」：边车评分 = rejected ? -1 : rating，LR 可识别的拒绝表示；星级在
/// DB 保留，取消拒绝恢复投影）。与 rating_set 共用同一投影，任一变更后
/// 边车都按 DB 真值重写。批量逐文件；单文件失败容忍不中断（AppError 上报）；
/// 边车缺失时新建；外部库只读资产跳过。
pub fn fetch_asset_reject_set(
    state: &super::AppState,
    asset_ids: &[i64],
    rejected: bool,
) -> Result<u64, String> {
    let db = super::active_library_db(state)?;
    let n = db
        .assets_reject_set(asset_ids, rejected)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(0);
    }
    // 边车投影清单（库内复制入册的资产；评分取 DB 当前值做投影）
    let targets: Vec<(String, i8)> =
        db.0.prepare(&format!(
            "SELECT path, origin, rating FROM assets WHERE id IN ({})",
            (0..asset_ids.len())
                .map(|i| format!("?{}", i + 1))
                .collect::<Vec<_>>()
                .join(", ")
        ))
        .and_then(|mut stmt| {
            let rows = stmt.query_map(rusqlite::params_from_iter(asset_ids.iter()), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()
        })
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|(_, origin, _)| origin != "external")
        .map(|(path, _, rating)| (path, super::rating::projected_rating(rating, rejected)))
        .collect();
    if targets.is_empty() {
        return Ok(n);
    }
    spawn_xmp_sync(
        state,
        XmpSync {
            task: format!("reject-batch-{n}"),
            subject: "拒绝状态",
            failure: "投影失败",
        },
        targets,
        |(path, projected)| {
            crate::metadata::xmp::sync_rating_to_sidecar(&PathBuf::from(path), projected)
        },
    );
    Ok(n)
}

/// 移入回收站核（软删：in_trash=1 + trashed_at；幂等）。返回新移入数。
pub fn fetch_asset_trash_move(state: &super::AppState, asset_ids: &[i64]) -> Result<u64, String> {
    let db = super::active_library_db(state)?;
    db.assets_trash_move(asset_ids).map_err(|e| e.to_string())
}

/// 回收站列表核（trashed_at DESC keyset；复用 AssetDto 契约）。
pub fn fetch_trash_list(
    state: &super::AppState,
    after_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::active_library_db(state)?;
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
    let db = super::active_library_db(state)?;
    db.trash_restore(asset_ids).map_err(|e| e.to_string())
}

/// 永久删除核（回收站「清空」动作；复用 duplicate_delete 的资产删除路径
/// 经验——失败容忍、日志记账、幂等）：
/// - DB：只作用于 in_trash=1 的行（防前端陈旧选中误删库内活跃资产），
///   行级联清引用（album_item/faces/view_history/similar_bucket/index_tasks）。
/// - 物理文件：仅 `delete_files=true` 且 origin=imported（外部库文件不在
///   库内，绝不物理删）；单文件失败不回滚 DB（计数+warn 日志）。
///
/// 返回 DB 删除行数。
pub fn fetch_trash_purge(
    state: &super::AppState,
    asset_ids: &[i64],
    delete_files: bool,
) -> Result<u64, String> {
    let db = super::active_library_db(state)?;
    let entries = db.trash_entries(asset_ids).map_err(|e| e.to_string())?;
    if entries.is_empty() {
        return Ok(0);
    }
    if delete_files {
        for (id, path, origin) in &entries {
            if origin == "external" {
                continue;
            }
            if let Err(e) = std::fs::remove_file(path) {
                if e.kind() != std::io::ErrorKind::NotFound {
                    let _ = db.append_log(
                        "warn",
                        None,
                        &format!("回收站清除：文件删除失败 {path}: {e}（库行仍清除）"),
                    );
                }
            }
            let _ = db.append_log("info", None, &format!("回收站清除：{path}"));
            let _ = id; // id 只用于选中集，删除走下面的批量
        }
    }
    let ids: Vec<i64> = entries.iter().map(|(id, _, _)| *id).collect();
    let deleted = db.assets_delete_rows(&ids).map_err(|e| e.to_string())?;
    Ok(deleted)
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

/// 永久删除（delete_files=true 时物理删文件；外部库只删库行）。返回 DB 删除数。
#[tauri::command]
pub async fn trash_purge(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
    delete_files: bool,
) -> Result<u64, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_trash_purge(state, &asset_ids, delete_files)
    })
    .await
}
