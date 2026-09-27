//! 归册挪移 + LR 暂存夹命令（阶段 B3，用户定案 2026-09-27 及其修订）。
//!
//! `album_claim_assets`：归入 = 物理挪移并改主相册。支持从日期根（历史
//! 遗留）、从未分组、从任意相册主目录挪到目标相册目录
//! （`photoRoot/{album.dir_name}/{YYYY}/{MM-DD}/`，日期段取 captured_at
//! 本地时区；无拍摄时间归 `0000/00-00`）。同卷 rename 毫秒级；跨卷
//! copy（.part 原子落位）+ xxhash 校验 + 删源。XMP 边车随行；DB 路径
//! 同步更新。挪移成功后：从原主相册移除引用（原主相册为「未分组」时
//! 保留——它是系统兜底袋，引用不清）+ 目标相册引用建立。已在目标相册
//! 目录 → 幂等跳过（仅补引用）；外部库（origin=external，文件不在库内）
//! 拒绝挪移；回收站资产拒绝。
//!
//! 主相册判定（实现选型）：资产路径落在某相册 `photoRoot/{dir_name}/`
//! 前缀之下即归属该相册为「主相册」——按路径前缀判定，免额外记录列
//! （路径即真值，claim/导入两条写入路径天然一致）；「未分组」的照片在
//! UI 判定为未真正归类（前端读未分组引用或路径前缀均可）。
//!
//! `lr_staging_create`：把所选资产放入「LR 暂存夹」供 Lightroom 导入/
//! 修改后回传——目标 `photoRoot 所在卷/.lr-staging/{name 或 时间戳}/`。
//! 逐文件优先硬链接（零额外占用，NAS SMB 支持则同样零拷贝），失败或
//! 跨卷回退复制。暂存夹一次性可弃，应用不管理其生命周期（v1 无清理命令）。
//!
//! 全部走 active_library_db + run_blocking（铁律：磁盘 IO 不上主线程）。

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 单条挪移失败（不整批回滚）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimFailureDto {
    pub asset_id: i64,
    pub path: String,
    pub error: String,
}

/// 归册挪移结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimResultDto {
    /// 实际挪移（含引用补挂）条数。
    pub moved: u64,
    /// 已在目标相册目录内（幂等重放）条数。
    pub skipped: u64,
    pub failed: Vec<ClaimFailureDto>,
}

/// LR 暂存夹创建结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LrStagingResultDto {
    /// 暂存夹绝对路径。
    pub dir: String,
    /// 本次是否新建了暂存夹目录（已存在则复用）。
    pub created: bool,
    /// 硬链接条目数（零额外占用）。
    pub hardlinked: u64,
    /// 回退复制条目数。
    pub copied: u64,
}

/// 卷根比较：src 与 dst 卷根字符串不同 → 视为跨卷（走 copy+校验+删源）。
/// 注意：verbatim 前缀（`\\?\C:\`）与普通盘根（`C:\`）判为不同卷——保守
/// 走复制路径（挪移安全性优先），同时给测试提供确定性的「模拟两卷」手段。
fn volume_root_of(path: &Path) -> String {
    path.ancestors()
        .last()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 相册主目录前缀（含尾分隔符；字符串路径与库内存储形态一致）。
fn album_prefix(photo_root: &str, dir_name: &str) -> String {
    format!(
        "{}{}",
        Path::new(photo_root).join(dir_name).display(),
        std::path::MAIN_SEPARATOR
    )
}

/// 冲突后缀：原名保留，重名追加 ` (2)`、` (3)`…（与导入引擎约定一致）。
fn resolve_conflict(dir: &Path, filename: &str) -> PathBuf {
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

/// captured_at（UTC RFC3339）→ 本地时区 `(YYYY, MM-DD)` 目录段。
/// 无拍摄时间归 `("0000", "00-00")`（与画廊 unknown 组语义对齐且路径安全）。
fn date_segments(captured_at: Option<&str>) -> (String, String) {
    captured_at
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| {
            let local = t.with_timezone(&chrono::Local);
            (
                local.format("%Y").to_string(),
                local.format("%m-%d").to_string(),
            )
        })
        .unwrap_or_else(|| ("0000".to_string(), "00-00".to_string()))
}

/// 挪移单个文件（含 XMP 边车随行）：同卷 rename / 跨卷 copy+校验+删源。
/// expected_xxhash 为库内权威指纹（0 哨兵 = 只校验 size）。
fn move_file_with_sidecar(
    src: &Path,
    dst: &Path,
    size: u64,
    expected_xxhash: u64,
) -> Result<(), String> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let sidecar_src = crate::metadata::xmp::sidecar_path(src);
    let sidecar_dst = crate::metadata::xmp::sidecar_path(dst);
    let sidecar = sidecar_src.is_file();

    let same_volume = volume_root_of(src) == volume_root_of(dst);
    if same_volume {
        std::fs::rename(src, dst).map_err(|e| format!("同卷改名失败: {e}"))?;
    } else {
        // 跨卷：.part 原子落位 + 流式 xxhash 校验（与库内指纹对账）
        let part = dst.with_extension("part");
        let copy = || -> Result<u64, String> {
            let mut reader = std::fs::File::open(src).map_err(|e| format!("打开失败: {e}"))?;
            let mut writer = std::fs::File::create(&part).map_err(|e| format!("写失败: {e}"))?;
            let mut hasher = xxhash_rust::xxh64::Xxh64::new(0);
            let mut buf = vec![0u8; 8 * 1024 * 1024];
            loop {
                let n = reader.read(&mut buf).map_err(|e| format!("读失败: {e}"))?;
                if n == 0 {
                    break;
                }
                writer
                    .write_all(&buf[..n])
                    .map_err(|e| format!("写失败: {e}"))?;
                hasher.update(&buf[..n]);
            }
            writer.flush().map_err(|e| format!("写失败: {e}"))?;
            Ok(hasher.digest())
        };
        let copied_size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
        match copy() {
            Ok(hash) => {
                if copied_size != size || (expected_xxhash != 0 && hash != expected_xxhash) {
                    let _ = std::fs::remove_file(&part);
                    return Err(format!("跨卷校验失败（size {copied_size}/{size}）——源保留"));
                }
                std::fs::rename(&part, dst).map_err(|e| format!("落位失败: {e}"))?;
            }
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                return Err(e);
            }
        }
        // 校验通过才删源
        std::fs::remove_file(src).map_err(|e| format!("删源失败（成片已就位）: {e}"))?;
    }
    if sidecar {
        if same_volume {
            let _ = std::fs::rename(&sidecar_src, &sidecar_dst);
        } else {
            let _ = std::fs::copy(&sidecar_src, &sidecar_dst)
                .and_then(|_| std::fs::remove_file(&sidecar_src));
        }
    }
    Ok(())
}

/// 归册挪移核。
pub fn fetch_album_claim_assets(
    state: &super::AppState,
    album_id: i64,
    asset_ids: &[i64],
) -> Result<ClaimResultDto, String> {
    let db = super::active_library_db(state)?;
    if !db.album_exists(album_id).map_err(|e| e.to_string())? {
        return Err("相册不存在".into());
    }
    let dir_name = db
        .album_dir_name(album_id)
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
    let target_prefix = album_prefix(&photo_root, &dir_name);
    // 全相册主目录前缀（主相册判定：路径前缀命中即归属）
    let prefixes: Vec<(i64, String)> = db
        .album_dir_names()
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|(aid, dir)| (aid, album_prefix(&photo_root, &dir)))
        .collect();
    // 默认相册 id（挪出「未分组」时引用保留——它是不清空的兜底袋）
    let default_album_id: Option<i64> =
        db.0.query_row(
            "SELECT id FROM album WHERE name = ?1",
            [crate::db::DEFAULT_ALBUM_NAME],
            |r| r.get(0),
        )
        .ok();

    let mut result = ClaimResultDto {
        moved: 0,
        skipped: 0,
        failed: Vec::new(),
    };
    for asset_id in asset_ids.iter().copied() {
        let mut fail = |error: String| {
            result.failed.push(ClaimFailureDto {
                asset_id,
                path: String::new(),
                error,
            });
        };
        let row: Option<(String, String, Option<String>, i64, u64)> =
            db.0.query_row(
                "SELECT path, filename, captured_at, in_trash, size FROM assets WHERE id = ?1",
                [asset_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
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
        let Some((path, filename, captured_at, in_trash, size)) = row else {
            continue; // 失效 id：静默跳过（与 album_add_assets 同语义）
        };
        if in_trash != 0 {
            fail("资产在回收站，不能挪移".into());
            continue;
        }
        let src = PathBuf::from(&path);
        if path.starts_with(&target_prefix) {
            // 幂等：已在目标相册目录——仅确保引用存在
            let _ = db.album_add_assets(album_id, &[asset_id]);
            result.skipped += 1;
            continue;
        }
        // 主相册判定：路径前缀命中的相册（可为他册 / 未分组 / None=日期根）
        let main_album: Option<i64> = prefixes
            .iter()
            .find(|(_, p)| path.starts_with(p))
            .map(|(aid, _)| *aid);
        let origin: String =
            db.0.query_row("SELECT origin FROM assets WHERE id = ?1", [asset_id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if origin == "external" {
            fail("外部库只读资产不可挪移（只能引用加入相册）".into());
            continue;
        }
        if !src.is_file() {
            fail(format!("源文件不在盘：{path}"));
            continue;
        }
        let (year, month_day) = date_segments(captured_at.as_deref());
        let dst_dir = Path::new(&photo_root)
            .join(&dir_name)
            .join(year)
            .join(month_day);
        let dst = resolve_conflict(&dst_dir, &filename);
        let xxhash: i64 =
            db.0.query_row("SELECT xxhash FROM assets WHERE id = ?1", [asset_id], |r| {
                r.get(0)
            })
            .map_err(|e| e.to_string())?;
        if let Err(e) = move_file_with_sidecar(&src, &dst, size, xxhash as u64) {
            fail(e);
            continue;
        }
        let new_name = dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| filename.clone());
        if let Err(e) = db.asset_update_path(asset_id, &dst.to_string_lossy(), &new_name) {
            fail(format!("库路径更新失败: {e}"));
            continue;
        }
        // 引用转移：从原主相册移除（「未分组」保留——兜底袋不清引用）+
        // 目标相册建立（幂等）
        if let Some(mid) = main_album {
            if Some(mid) != default_album_id {
                let _ = db.album_remove_assets(mid, &[asset_id]);
            }
        }
        let _ = db.album_add_assets(album_id, &[asset_id]);
        let _ = db.append_log(
            "info",
            None,
            &format!("归册挪移：{path} → {}（相册 {album_id}）", dst.display()),
        );
        result.moved += 1;
    }
    Ok(result)
}

/// LR 暂存夹核。
pub fn fetch_lr_staging_create(
    state: &super::AppState,
    asset_ids: &[i64],
    name: Option<&str>,
) -> Result<LrStagingResultDto, String> {
    let db = super::active_library_db(state)?;
    let photo_root = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?
        .photo_root;
    let volume_root = volume_root_of(Path::new(&photo_root));
    if volume_root.is_empty() {
        return Err("无法解析照片根所在卷".into());
    }
    let base = Path::new(&volume_root).join(".lr-staging");
    let dir_name = match name.map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) => crate::db::sanitize_dir_name(n),
        None => chrono::Local::now().format("lr-%Y%m%d-%H%M%S").to_string(),
    };
    let dir = base.join(&dir_name);
    let created = !dir.is_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建暂存夹失败: {e}"))?;

    let mut result = LrStagingResultDto {
        dir: dir.to_string_lossy().into_owned(),
        created,
        hardlinked: 0,
        copied: 0,
    };
    for asset_id in asset_ids.iter().copied() {
        let path: Option<String> =
            db.0.query_row("SELECT path FROM assets WHERE id = ?1", [asset_id], |r| {
                r.get(0)
            })
            .ok();
        let Some(path) = path else {
            continue; // 失效 id 跳过
        };
        let src = PathBuf::from(&path);
        if !src.is_file() {
            continue; // 不在盘：跳过（暂存夹对缺席文件无意义）
        }
        let filename = src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let dst = resolve_conflict(&dir, &filename);
        // 同卷优先硬链接（Windows = CreateHardLinkW，失败回退复制）；跨卷直接复制
        let same_volume = volume_root_of(&src) == volume_root_of(&dir);
        let staged = same_volume && std::fs::hard_link(&src, &dst).is_ok();
        if staged {
            result.hardlinked += 1;
        } else if std::fs::copy(&src, &dst).is_ok() {
            result.copied += 1;
        }
    }
    let _ = db.append_log(
        "info",
        None,
        &format!(
            "LR 暂存夹：{}（硬链 {}，复制 {}，共 {} 项）",
            result.dir,
            result.hardlinked,
            result.copied,
            result.hardlinked + result.copied
        ),
    );
    Ok(result)
}

// ---------------------------------------------------------------------------
// Tauri 命令壳（async + spawn_blocking）
// ---------------------------------------------------------------------------

/// 归册挪移（物理挪进相册主目录 + 补挂引用；幂等可重试）。
#[tauri::command]
pub async fn album_claim_assets(
    state: State<'_, SharedState>,
    album_id: i64,
    asset_ids: Vec<i64>,
) -> Result<ClaimResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_claim_assets(state, album_id, &asset_ids)
    })
    .await
}

/// 创建 LR 暂存夹并放入所选资产（硬链接优先、失败回退复制）。
#[tauri::command]
pub async fn lr_staging_create(
    state: State<'_, SharedState>,
    asset_ids: Vec<i64>,
    name: Option<String>,
) -> Result<LrStagingResultDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_lr_staging_create(state, &asset_ids, name.as_deref())
    })
    .await
}
