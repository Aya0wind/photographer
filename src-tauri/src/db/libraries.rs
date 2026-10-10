//! photos_libraries 登记表仓储（2026-10-09 单数据库多照片库定案，计划 §二/§八-6）。
//!
//! 照片库 = 一个文件夹的登记项（纯物理概念）：N 个库共居一个数据库（多数据
//! 库修正：一切照片库操作作用于激活数据库，2026-10-09），
//! `assets.library_id` 引用这里的 id。相册/子组是数据库全局逻辑概念，不分库。
//!
//! root 互斥校验（§八-6）是登记的统一闸门：各库 root 之间不得相同
//! 或互相包含；root 不得与数据库目录相同或互相包含（防把 thumbs/ 缩略图
//! 目录登记进库）。校验同时覆盖逻辑路径与 canonical 实路径（junction/
//! 符号链接逃逸防线，与 settings::validate_library_storage_paths 同手法）。

use std::path::Path;

use rusqlite::{params, Result};

use super::{now_rfc3339, strip_root_prefix, Db};

/// photos_libraries 行（photo_library_list 数据源 / IPC 载荷 PhotoLibraryDto
/// 的行投影，camelCase）。asset_count/size_bytes 为统计缓存——登记/删除/
/// 移除后由 [`Db::photos_library_refresh_stats`] 重算，读路径不现算。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PhotosLibraryRow {
    /// uuid（登记时生成）。
    pub id: String,
    pub name: String,
    /// 库根目录（规范化绝对路径；登记闸门保证形态）。
    pub root_path: String,
    /// 登记时间（RFC3339）。
    pub created_at: String,
    /// "online" | "offline"（整库离线标记；单文件缺失走 assets.missing）。
    pub status: String,
    /// 库内照片数缓存。
    pub asset_count: u64,
    /// 库内照片容量缓存（字节）。
    pub size_bytes: u64,
}

/// 两路径「相同或互相包含」（忽略大小写与 `/`\\` 方向；前缀后必须是
/// 分隔符，防 `I:\\x` 误匹配 `I:\\xy`——复用 [`strip_root_prefix`] 语义）。
fn paths_overlap(a: &str, b: &str) -> bool {
    strip_root_prefix(a, b).is_some() || strip_root_prefix(b, a).is_some()
}

/// library_scan_jobs 行（M4b 批量登记任务账本）：一行一库（PK），
/// registered/skipped 为跨轮累计绝对值（进度入库 + 崩溃续跑的进度基线）。
#[derive(Debug, Clone, PartialEq)]
pub struct LibraryScanJobRow {
    pub library_id: String,
    /// pending | running | cancelling | done | failed。
    pub status: String,
    /// 预点数（进度分母，估算口径）。
    pub total: u64,
    pub registered: u64,
    pub skipped: u64,
    pub error: Option<String>,
    pub updated_at: String,
}

fn map_scan_job(row: &rusqlite::Row<'_>) -> Result<LibraryScanJobRow> {
    Ok(LibraryScanJobRow {
        library_id: row.get(0)?,
        status: row.get(1)?,
        total: row.get::<_, i64>(2)? as u64,
        registered: row.get::<_, i64>(3)? as u64,
        skipped: row.get::<_, i64>(4)? as u64,
        error: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// QueryReturnedNoRows → None 的共用映射（get 形态查询）。
fn map_no_rows<T>(e: rusqlite::Error) -> Result<Option<T>> {
    match e {
        rusqlite::Error::QueryReturnedNoRows => Ok(None),
        other => Err(other),
    }
}

/// root 互斥校验（§八-6，纯函数便于预检与单测）。
///
/// - `existing`：已登记照片库 (name, root_path) 列表（新 root 不得与任一
///   相同或互相包含）；
/// - `database_dir`：数据库目录解析结果（新 root 不得与其相同或互相包含，
///   防把 thumbs/ 缩略图登记进库）；
/// - `root`：待登记的库根目录（规范化绝对路径）。
///
/// 逻辑路径判重之外再做 canonical 实路径判重（两侧均在盘时）：junction/
/// 符号链接指向库内/库外的逃逸路径在逻辑形态上不可见，canonical 交叉核对
/// 兜住（同 settings::validate_library_storage_paths 手法）。
pub fn validate_photos_library_root(
    existing: &[(String, String)],
    database_dir: &Path,
    root: &Path,
) -> Result<(), String> {
    let root_str = root.to_string_lossy().into_owned();
    // 照片库根是用户照片文件夹，不得直接选磁盘/网络共享根目录
    //（与旧建库闸门一致：整盘登记会把回收站/系统卷信息目录卷进扫描）。
    if root
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return Err(format!(
            "照片库根目录不能是磁盘或网络共享的根目录：{root_str}"
        ));
    }
    let db_str = database_dir.to_string_lossy().into_owned();
    if paths_overlap(&root_str, &db_str) {
        return Err(format!(
            "照片库根目录不得与数据库目录相同或互相包含（数据库在 {db_str}）"
        ));
    }
    for (name, other) in existing {
        if paths_overlap(&root_str, other) {
            return Err(format!(
                "照片库根目录与已有照片库「{name}」相同或互相包含（{other}）"
            ));
        }
    }
    // canonical 实路径交叉核对（junction/符号链接逃逸防线；任一侧不在盘
    // 则跳过——新 root 允许先登记后挂载，缺失走 offline 语义）
    if let Ok(real_root) = root.canonicalize() {
        let real_root = real_root.to_string_lossy().into_owned();
        if let Ok(real_db) = database_dir.canonicalize() {
            let real_db = real_db.to_string_lossy().into_owned();
            if paths_overlap(&real_root, &real_db) {
                return Err(
                    "照片库根目录与数据库目录实际位置相同或互相包含（可能是链接指向）".into(),
                );
            }
        }
        for (name, other) in existing {
            if let Ok(real_other) = Path::new(other).canonicalize() {
                let real_other = real_other.to_string_lossy().into_owned();
                if paths_overlap(&real_root, &real_other) {
                    return Err(format!(
                        "照片库根目录与已有照片库「{name}」实际位置相同或互相包含（{real_other}）"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn map_library(row: &rusqlite::Row<'_>) -> Result<PhotosLibraryRow> {
    Ok(PhotosLibraryRow {
        id: row.get(0)?,
        name: row.get(1)?,
        root_path: row.get(2)?,
        created_at: row.get(3)?,
        status: row.get(4)?,
        asset_count: row.get::<_, i64>(5)? as u64,
        size_bytes: row.get::<_, i64>(6)? as u64,
    })
}

const LIBRARY_COLS: &str = "id, name, root_path, created_at, status, asset_count, size_bytes";

impl Db {
    /// 全部照片库（登记序：created_at, id）。
    pub fn photos_library_list(&self) -> Result<Vec<PhotosLibraryRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {LIBRARY_COLS} FROM photos_libraries \
                 ORDER BY created_at, id"
        ))?;
        let rows = stmt.query_map([], map_library)?;
        rows.collect()
    }

    /// 按 id 取照片库（不存在 None）。
    pub fn photos_library_get(&self, id: &str) -> Result<Option<PhotosLibraryRow>> {
        self.0
            .query_row(
                &format!("SELECT {LIBRARY_COLS} FROM photos_libraries WHERE id = ?1"),
                [id],
                map_library,
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// root 互斥预检（登记使用）：加载已登记 roots 后调用
    /// [`validate_photos_library_root`]。`exclude_id` 可排除已有登记。
    pub fn photos_library_validate_root(
        &self,
        root: &Path,
        database_dir: &Path,
        exclude_id: Option<&str>,
    ) -> Result<(), String> {
        let existing: Vec<(String, String)> = self
            .photos_library_list()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|lib| Some(lib.id.as_str()) != exclude_id)
            .map(|lib| (lib.name, lib.root_path))
            .collect();
        validate_photos_library_root(&existing, database_dir, root)
    }

    /// 登记照片库：生成 uuid + root 互斥校验 + 插入，返回新行（统计缓存
    /// 归零，status=online）。root 必须已过调用方规范化（IPC 层
    /// settings::normalize_library_path 同款闸门）。
    pub fn photos_library_register(
        &self,
        name: &str,
        root_path: &str,
        database_dir: &Path,
    ) -> Result<PhotosLibraryRow, String> {
        let root = Path::new(root_path);
        self.photos_library_validate_root(root, database_dir, None)?;
        let row = PhotosLibraryRow {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.to_string(),
            root_path: root_path.to_string(),
            created_at: now_rfc3339(),
            status: "online".to_string(),
            asset_count: 0,
            size_bytes: 0,
        };
        self.0
            .execute(
                "INSERT INTO photos_libraries (id, name, root_path, created_at, status, \
                 asset_count, size_bytes) VALUES (?1, ?2, ?3, ?4, ?5, 0, 0)",
                params![row.id, row.name, row.root_path, row.created_at, row.status],
            )
            .map_err(|e| e.to_string())?;
        Ok(row)
    }

    /// 设置在线状态（"online" | "offline"；reconcile 按库粒度翻转，§五）。
    pub fn photos_library_set_status(&self, id: &str, status: &str) -> Result<()> {
        self.0.execute(
            "UPDATE photos_libraries SET status = ?2 WHERE id = ?1",
            params![id, status],
        )?;
        Ok(())
    }

    /// 重算库统计缓存（照片数 + 容量字节；口径 = 库内 photo/raw 且不在
    /// 回收站）。登记/删除/移除后调用；列仅缓存，读路径不依赖实时性。
    pub fn photos_library_refresh_stats(&self, id: &str) -> Result<()> {
        self.0.execute(
            "UPDATE photos_libraries SET \
                 asset_count = (SELECT COUNT(*) FROM assets \
                     WHERE library_id = ?1 AND in_trash = 0 AND kind IN ('photo', 'raw')), \
                 size_bytes = (SELECT COALESCE(SUM(size), 0) FROM assets \
                     WHERE library_id = ?1 AND in_trash = 0 AND kind IN ('photo', 'raw')) \
             WHERE id = ?1",
            [id],
        )?;
        Ok(())
    }

    /// 移除登记：永不删照片文件（用户红线）；`delete_records=true` 连库内
    /// 资产记录一并删（问过用户后）。返回连带删除的资产记录数。
    /// 资产行删除触发全库级联（索引任务/人脸/相册引用等随行消失）。
    pub fn photos_library_remove(&self, id: &str, delete_records: bool) -> Result<u64, String> {
        if self
            .photos_library_get(id)
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Err(format!("照片库不存在：{id}"));
        }
        let deleted = if delete_records {
            self.0
                .execute("DELETE FROM assets WHERE library_id = ?1", [id])
                .map_err(|e| e.to_string())? as u64
        } else {
            0
        };
        self.0
            .execute("DELETE FROM photos_libraries WHERE id = ?1", [id])
            .map_err(|e| e.to_string())?;
        Ok(deleted)
    }

    /// 登记指纹查重（§三 两级识别第一级）：同卷同 file id = 同一物理文件
    /// （硬链接，库内导出物场景）→ 零哈希成本直跳。返回 (资产 id, 路径)。
    pub fn find_asset_by_volume_file_id(
        &self,
        volume_serial: i64,
        file_id: &str,
    ) -> Result<Option<(i64, String)>> {
        self.0
            .query_row(
                "SELECT id, path FROM assets WHERE volume_serial = ?1 AND file_id = ?2 LIMIT 1",
                params![volume_serial, file_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 登记指纹查重的重绑形态（§八-1）：返回 (id, path, missing)——命中
    /// 在线资产 = 硬链接直跳；命中 missing 资产 = 移动/改名 → 重绑路径。
    /// 排序 missing 升序：同指纹多行（历史数据）时在线者优先命中去重。
    pub fn find_asset_brief_by_volume_file_id(
        &self,
        volume_serial: i64,
        file_id: &str,
    ) -> Result<Option<(i64, String, bool)>> {
        self.0
            .query_row(
                "SELECT id, path, missing FROM assets \
                 WHERE volume_serial = ?1 AND file_id = ?2 \
                 ORDER BY missing ASC LIMIT 1",
                params![volume_serial, file_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 同库同指纹查重的重绑形态（§八-1）：**skip 仅当存在在线同哈希资产**；
    /// 命中 missing 资产 = 移动/改名 → 重绑。在线优先排序兜住同指纹多行。
    pub fn find_asset_brief_by_size_xxh(
        &self,
        library_id: &str,
        size: u64,
        xxhash: u64,
    ) -> Result<Option<(i64, String, bool)>> {
        self.0
            .query_row(
                "SELECT id, path, missing FROM assets \
                 WHERE library_id = ?1 AND size = ?2 AND xxhash = ?3 \
                 ORDER BY missing ASC LIMIT 1",
                params![library_id, size as i64, xxhash as i64],
                |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    // —— 库扫描状态账本（§三 增量扫描 / §八 2/3/7；表见 schema 注释）——

    /// 目录 mtime 剪枝缓存读取（未见过 None；RFC3339 字典序即时间序）。
    pub fn library_scan_dir_mtime(
        &self,
        library_id: &str,
        dir_path: &str,
    ) -> Result<Option<String>> {
        self.0
            .query_row(
                "SELECT mtime FROM library_scan_dirs WHERE library_id = ?1 AND dir_path = ?2",
                params![library_id, dir_path],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 目录 mtime 剪枝缓存写入（upsert）。
    pub fn library_scan_dir_set_mtime(
        &self,
        library_id: &str,
        dir_path: &str,
        mtime: &str,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO library_scan_dirs (library_id, dir_path, mtime) VALUES (?1, ?2, ?3) \
             ON CONFLICT (library_id, dir_path) DO UPDATE SET mtime = excluded.mtime",
            params![library_id, dir_path, mtime],
        )?;
        Ok(())
    }

    /// 目录剪枝缓存遗忘（未收敛目录：本轮有冷却跳过/新缺席记账——下轮
    /// 必须重枚举，否则冷却中的文件与两轮确认的第二轮被 mtime 剪枝饿死）。
    pub fn library_scan_dir_forget(&self, library_id: &str, dir_path: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM library_scan_dirs WHERE library_id = ?1 AND dir_path = ?2",
            params![library_id, dir_path],
        )?;
        Ok(())
    }

    /// 缺席第一轮记账（§八-7 两轮确认）：已记账返回 false（第二轮 → 调用方
    /// 标 missing）；新记账返回 true。
    pub fn library_scan_note_absent(&self, asset_id: i64, library_id: &str) -> Result<bool> {
        let inserted = self.0.execute(
            "INSERT OR IGNORE INTO library_scan_absent (asset_id, library_id, noted_at) \
             VALUES (?1, ?2, ?3)",
            params![asset_id, library_id, now_rfc3339()],
        )?;
        Ok(inserted > 0)
    }

    /// 资产回到在位（或被删除/重绑）：清缺席账。
    pub fn library_scan_clear_absent(&self, asset_id: i64) -> Result<()> {
        self.0.execute(
            "DELETE FROM library_scan_absent WHERE asset_id = ?1",
            [asset_id],
        )?;
        Ok(())
    }

    /// 晚到边车已读 mtime（§八-3 触发一次；未读过 None）。
    pub fn library_scan_sidecar_read_at(&self, asset_id: i64) -> Result<Option<String>> {
        self.0
            .query_row(
                "SELECT mtime FROM library_scan_sidecars WHERE asset_id = ?1",
                [asset_id],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })
    }

    /// 晚到边车读入记账（记录本次读入的边车 mtime）。
    pub fn library_scan_sidecar_set_read(&self, asset_id: i64, mtime: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO library_scan_sidecars (asset_id, mtime) VALUES (?1, ?2) \
             ON CONFLICT (asset_id) DO UPDATE SET mtime = excluded.mtime",
            params![asset_id, mtime],
        )?;
        Ok(())
    }

    // —— 批量登记任务账本（M4b 从文件夹建立 §三；表见 schema 注释）——

    /// 落/重置批量登记任务（pending，计数清零——重新建任务即重新计数）。
    /// 「从文件夹建立」登记库行后调用，随后 kick 扫描 worker 立即拾取。
    pub fn library_scan_job_enqueue(&self, library_id: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO library_scan_jobs (library_id, status, total, registered, skipped, \
             error, updated_at) VALUES (?1, 'pending', 0, 0, 0, NULL, ?2) \
             ON CONFLICT (library_id) DO UPDATE SET status = 'pending', total = 0, \
             registered = 0, skipped = 0, error = NULL, updated_at = excluded.updated_at",
            params![library_id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 待拾取任务清单（updated_at 序）。含 pending（常态）与 failed（自愈
    /// 重试，§「可重扫」）与 running（上一进程崩溃遗留——登记幂等，续跑
    /// 即重新全量收敛）；顺带清掉崩溃残留的 cancelling 行（取消已不可能
    /// 被响应，行删除即任务消失）。
    pub fn library_scan_jobs_pickup(&self) -> Result<Vec<LibraryScanJobRow>> {
        let _ = self.0.execute(
            "DELETE FROM library_scan_jobs WHERE status = 'cancelling'",
            [],
        );
        let mut stmt = self.0.prepare(
            "SELECT library_id, status, total, registered, skipped, error, updated_at \
             FROM library_scan_jobs WHERE status IN ('pending', 'running', 'failed') \
             ORDER BY updated_at, library_id",
        )?;
        let rows = stmt.query_map([], map_scan_job)?;
        rows.collect()
    }

    /// 单行读取（photo_library_scan_status 数据源；不存在 None）。
    pub fn library_scan_job_get(&self, library_id: &str) -> Result<Option<LibraryScanJobRow>> {
        self.0
            .query_row(
                "SELECT library_id, status, total, registered, skipped, error, updated_at \
                 FROM library_scan_jobs WHERE library_id = ?1",
                [library_id],
                map_scan_job,
            )
            .map(Some)
            .or_else(map_no_rows)
    }

    /// 全部任务行（photo_library_scan_status 全量投影）。
    pub fn library_scan_job_list(&self) -> Result<Vec<LibraryScanJobRow>> {
        let mut stmt = self.0.prepare(
            "SELECT library_id, status, total, registered, skipped, error, updated_at \
             FROM library_scan_jobs ORDER BY updated_at, library_id",
        )?;
        let rows = stmt.query_map([], map_scan_job)?;
        rows.collect()
    }

    /// 标记 running（条件更新即取消竞态闸：软取消删/改行在前 → 0 行，调用
    /// 方放弃拾取；SQLite 写串行保证判定原子）。
    pub fn library_scan_job_mark_running(&self, library_id: &str) -> Result<bool> {
        let updated = self.0.execute(
            "UPDATE library_scan_jobs SET status = 'running', updated_at = ?2 \
             WHERE library_id = ?1 AND status IN ('pending', 'running', 'failed')",
            params![library_id, now_rfc3339()],
        )?;
        Ok(updated > 0)
    }

    /// 进度落库（绝对值：registered/skipped 由调用方累计基线后写入）。upsert
    /// 只动计数列不动 status——软取消置的 cancelling 不被进度写覆盖。
    pub fn library_scan_job_progress(
        &self,
        library_id: &str,
        total: u64,
        registered: u64,
        skipped: u64,
    ) -> Result<()> {
        self.0.execute(
            "INSERT INTO library_scan_jobs (library_id, status, total, registered, skipped, \
             error, updated_at) VALUES (?1, 'pending', ?2, ?3, ?4, NULL, ?5) \
             ON CONFLICT (library_id) DO UPDATE SET total = excluded.total, \
             registered = excluded.registered, skipped = excluded.skipped, \
             updated_at = excluded.updated_at",
            params![
                library_id,
                total as i64,
                registered as i64,
                skipped as i64,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// 收尾（done/failed + 终值计数 + 错误文案）。
    pub fn library_scan_job_finish(
        &self,
        library_id: &str,
        status: &str,
        total: u64,
        registered: u64,
        skipped: u64,
        error: Option<&str>,
    ) -> Result<()> {
        self.0.execute(
            "UPDATE library_scan_jobs SET status = ?2, total = ?3, registered = ?4, \
             skipped = ?5, error = ?6, updated_at = ?7 WHERE library_id = ?1",
            params![
                library_id,
                status,
                total as i64,
                registered as i64,
                skipped as i64,
                error,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// 回队 pending（导入让路/根离线：导入结束或根回来后续跑；计数保留，
    /// 进度条跨轮连续）。
    pub fn library_scan_job_requeue(&self, library_id: &str) -> Result<()> {
        self.0.execute(
            "UPDATE library_scan_jobs SET status = 'pending', updated_at = ?2 \
             WHERE library_id = ?1",
            params![library_id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 软取消（photo_library_scan_cancel）：pending 行直接删（尚未开跑，
    /// 任务消失）；running 行置 cancelling（worker 文件边界响应后删行）。
    pub fn library_scan_job_cancel(&self, library_id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM library_scan_jobs WHERE library_id = ?1 AND status = 'pending'",
            [library_id],
        )?;
        self.0.execute(
            "UPDATE library_scan_jobs SET status = 'cancelling', updated_at = ?2 \
             WHERE library_id = ?1 AND status = 'running'",
            params![library_id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 删行（worker 响应取消收尾 / 库行已移除的孤儿任务清理）。
    pub fn library_scan_job_delete(&self, library_id: &str) -> Result<()> {
        self.0.execute(
            "DELETE FROM library_scan_jobs WHERE library_id = ?1",
            [library_id],
        )?;
        Ok(())
    }

    /// 跨库重复计数（§四：同内容不同库为合法状态、照常登记；收尾总结提示
    /// 「N 张与其他照片库内容相同」的数据底座——整库口径，续跑/重扫幂等）。
    pub fn count_cross_library_duplicates(&self, library_id: &str) -> Result<u64> {
        self.0.query_row(
            "SELECT COUNT(*) FROM assets a WHERE a.library_id = ?1 AND a.in_trash = 0 \
                 AND EXISTS (SELECT 1 FROM assets b WHERE b.library_id IS NOT NULL \
                 AND b.library_id != ?1 AND b.size = a.size AND b.xxhash = a.xxhash)",
            [library_id],
            |r| Ok(r.get::<_, i64>(0)? as u64),
        )
    }

    // —— xmp_dirty 闭环（§五 M2c：库在线 → 补写边车、清标志）——

    /// 库内待补写边车的脏资产清单（xmp_dirty=1 且在库在册、非回收站、
    /// 非缺失——missing 资产的补写归恢复/重绑路径，§八-1）。
    /// 返回 (id, path, rating, color_label, rejected)。
    pub fn assets_xmp_dirty_in_library(
        &self,
        library_id: &str,
    ) -> Result<Vec<(i64, String, i64, Option<String>, bool)>> {
        let mut stmt = self.0.prepare(
            "SELECT id, path, rating, color_label, rejected != 0 FROM assets \
             WHERE library_id = ?1 AND xmp_dirty = 1 AND in_trash = 0 AND missing = 0",
        )?;
        let rows = stmt.query_map([library_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, bool>(4)?,
            ))
        })?;
        rows.collect()
    }

    /// 清 xmp_dirty（边车补写成功后；幂等）。
    pub fn clear_asset_xmp_dirty(&self, asset_id: i64) -> Result<()> {
        self.0
            .execute("UPDATE assets SET xmp_dirty = 0 WHERE id = ?1", [asset_id])?;
        Ok(())
    }
}
