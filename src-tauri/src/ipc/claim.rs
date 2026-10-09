//! 归册命令（阶段 B3；2026-10-09 单数据库多照片库 §一 逻辑化）。
//!
//! `album_claim_assets`：归入相册 = **纯引用建立**（与 album_add_assets
//! 同语义；相册/子组无物理足迹，变更归属零文件操作）。旧「物理挪移进
//! 相册主目录 + XMP 边车随行」随纯时间布局退役——资产物理位置只由导入
//! 时的 `{库root}/{拍摄年}/{拍摄月}/` 决定，与相册归属无关。
//!
//! 本模块同时承载跨模块复用的命名工具 [`resolve_conflict`]（重名追加
//! ` (2)`、` (3)`…；联拍摄入等落库路径用）。
//!
//! 全部走 app_database_db + run_blocking（铁律：DB 查询不上主线程）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 单条归册失败（不整批回滚）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimFailureDto {
    pub asset_id: i64,
    pub path: String,
    pub error: String,
}

/// 归册结果（逻辑化后 moved=本次新建引用数、skipped=已引用幂等数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResultDto {
    /// 本次新建引用条数。
    pub moved: u64,
    /// 已在该相册（幂等重放）条数。
    pub skipped: u64,
    pub failed: Vec<ClaimFailureDto>,
}

/// 冲突后缀：原名保留，重名追加 ` (2)`、` (3)`…（联拍摄入等库内落盘
/// 路径的让位约定；导入引擎主路径用 `_1` 式 [`crate::import::templates::
/// unique_path`]，两处口径均为「绝不覆盖既有文件」）。
pub(crate) fn resolve_conflict(dir: &Path, filename: &str) -> PathBuf {
    let mut candidate = dir.join(filename);
    let (stem, ext) = match filename.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), Some(e.to_string())),
        _ => (filename.to_string(), None),
    };
    let mut n = 2;
    while candidate.exists() {
        let name = match &ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        candidate = dir.join(name);
        n += 1;
    }
    candidate
}

/// 归册核（逻辑化）：目标相册不存在报错；逐资产建立 album_item 引用
///（幂等——已引用计 skipped）；回收站资产拒绝；失效 id 静默跳过。
pub fn fetch_album_claim_assets(
    state: &super::AppState,
    album_id: i64,
    asset_ids: &[i64],
    subgroup: Option<&str>,
) -> Result<ClaimResultDto, String> {
    let db = super::app_database_db(state)?;
    if !db.album_exists(album_id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    let subgroup = subgroup.map(str::trim).filter(|s| !s.is_empty());
    let mut result = ClaimResultDto {
        moved: 0,
        skipped: 0,
        failed: Vec::new(),
    };
    for asset_id in asset_ids.iter().copied() {
        let row: Option<(String, i64)> =
            db.0.query_row(
                "SELECT path, in_trash FROM assets WHERE id = ?1",
                [asset_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
            .map_err(|e| e.to_string())?;
        let Some((path, in_trash)) = row else {
            continue; // 失效 id：静默跳过（与 album_add_assets 同语义）
        };
        if in_trash != 0 {
            result.failed.push(ClaimFailureDto {
                asset_id,
                path,
                error: "资产在回收站，不能归册".into(),
            });
            continue;
        }
        let already: bool =
            db.0.query_row(
                "SELECT EXISTS(SELECT 1 FROM album_item \
                 WHERE album_id = ?1 AND asset_id = ?2)",
                rusqlite::params![album_id, asset_id],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if already {
            // 幂等：已在该相册（显式给子分组时改写归属，与 add 同语义）
            if let Some(sub) = subgroup {
                let _ = db.album_item_move_subgroup(album_id, &[asset_id], Some(sub));
            }
            result.skipped += 1;
            continue;
        }
        match db.album_add_assets(album_id, &[asset_id], subgroup) {
            Ok(added) => {
                result.moved += added;
                let _ = db.append_log(
                    "info",
                    None,
                    &format!("归册引用：资产 {asset_id}（{path}）→ 相册 {album_id}"),
                );
            }
            Err(e) => result.failed.push(ClaimFailureDto {
                asset_id,
                path,
                error: e.to_string(),
            }),
        }
    }
    Ok(result)
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + spawn_blocking）
// ---------------------------------------------------------------------------

/// 归册（纯引用建立；幂等可重试，绝不动物理文件）。
#[tauri::command]
pub async fn album_claim_assets(
    state: State<'_, SharedState>,
    album_id: i64,
    asset_ids: Vec<i64>,
    subgroup: Option<String>,
) -> Result<ClaimResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_claim_assets(state, album_id, &asset_ids, subgroup.as_deref())
    })
    .await
}
