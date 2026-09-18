//! SQLite 基础 + journal（spec §5.4）：M1 T2 由核心 lane 实现。
//!
//! - [`Db::open`]：WAL + `foreign_keys=ON` + `busy_timeout=5s`（多连接并发）。
//! - [`Db::migrate`]：`PRAGMA user_version` 驱动的内嵌迁移（SQL 在 [`migrations`]，只加不改）。
//! - jobs / job_files（断点恢复 journal）/ assets / logs 的仓储方法。
//!
//! [`FileState`]/[`AssetKind`] 复用 events 模块的领域枚举，列存储格式与其
//! serde camelCase 字符串严格一致（手写 rusqlite To/FromSql 映射，不引 derive 扩展 crate）。

mod migrations;

use std::path::Path;
use std::time::Duration;

use chrono::Utc;
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, Type, ValueRef};
use rusqlite::{params, Connection, Error, Result, Row, ToSql};
use serde::{Deserialize, Serialize};

use crate::events::{AssetKind, FileState};

/// 打开（必要时创建）库文件，并应用连接级 PRAGMA。
pub struct Db(pub Connection);

impl Db {
    /// WAL（读写不互斥）+ foreign_keys + 5s busy_timeout。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        // NORMAL 是与 WAL 配套的常规同步档位（断电最多丢最后事务，不损坏库）。
        conn.pragma_update(None, "synchronous", "normal")?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        Ok(Self(conn))
    }

    /// `PRAGMA user_version` 驱动的顺序迁移；每条迁移独立事务提交。
    pub fn migrate(&self) -> Result<()> {
        let current: i64 = self
            .0
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for (index, sql) in migrations::MIGRATIONS.iter().enumerate() {
            let version = (index + 1) as i64;
            if version <= current {
                continue;
            }
            let tx = self.0.unchecked_transaction()?;
            tx.execute_batch(sql)?;
            tx.pragma_update(None, "user_version", version)?;
            tx.commit()?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// 行结构
// ---------------------------------------------------------------------------

/// jobs 行（任务中心列表 / 断点恢复入口）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobRow {
    pub id: i64,
    pub kind: String,
    pub device_id: String,
    pub device_name: String,
    pub status: String,
    pub total_files: u64,
    pub total_bytes: u64,
    pub stats_json: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
}

/// job_files 行（journal：每文件一条，PK(job_id, src) 覆盖更新）。
/// dst2 为 F2 双目的地导入的第二目的地路径（单目的地导入恒空串）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFileRow {
    pub job_id: i64,
    pub src: String,
    pub dst: String,
    pub size: u64,
    pub state: FileState,
    pub error: Option<String>,
    pub xxhash: Option<u64>,
    pub sha256: Option<[u8; 32]>,
    #[serde(default)]
    pub dst2: String,
}

/// logs 行。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRow {
    pub id: i64,
    pub ts: String,
    pub level: String,
    pub job_id: Option<i64>,
    pub message: String,
}

/// assets 行（查重索引与库内资产表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetRow {
    pub path: String,
    pub filename: String,
    pub size: u64,
    pub mtime: String,
    pub xxhash: u64,
    pub sha256: [u8; 32],
    pub kind: AssetKind,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub source: String,
    pub created_at: String,
    /// 资产来源（M2 迁移 0003）：'imported' 复制/移动入册；
    /// 'external' 原地索引只读入册（文件不在库内，绝不可被清理/移动）。
    #[serde(default = "default_origin")]
    pub origin: String,
}

/// assets.origin 默认值（导入入册）。
pub fn default_origin() -> String {
    "imported".to_string()
}

// ---------------------------------------------------------------------------
// FileState / AssetKind 的 SQL 列映射（与 serde camelCase 输出一致）
// ---------------------------------------------------------------------------

impl FileState {
    /// 列存储字符串（与 serde 序列化结果一致）。
    pub const fn as_db_str(self) -> &'static str {
        match self {
            FileState::Pending => "pending",
            FileState::Copying => "copying",
            FileState::Verified => "verified",
            FileState::Skipped => "skipped",
            FileState::Failed => "failed",
        }
    }

    fn from_db_str(text: &str) -> Option<Self> {
        match text {
            "pending" => Some(FileState::Pending),
            "copying" => Some(FileState::Copying),
            "verified" => Some(FileState::Verified),
            "skipped" => Some(FileState::Skipped),
            "failed" => Some(FileState::Failed),
            _ => None,
        }
    }
}

impl ToSql for FileState {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.as_db_str().as_bytes(),
        )))
    }
}

impl FromSql for FileState {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Self::from_db_str(text).ok_or_else(|| {
            FromSqlError::Other(format!("unknown file state column value: {text}").into())
        })
    }
}

impl AssetKind {
    /// 列存储字符串（与 serde 序列化结果一致）。
    pub const fn as_db_str(self) -> &'static str {
        match self {
            AssetKind::Photo => "photo",
            AssetKind::Raw => "raw",
            AssetKind::Video => "video",
            AssetKind::Other => "other",
        }
    }

    fn from_db_str(text: &str) -> Option<Self> {
        match text {
            "photo" => Some(AssetKind::Photo),
            "raw" => Some(AssetKind::Raw),
            "video" => Some(AssetKind::Video),
            "other" => Some(AssetKind::Other),
            _ => None,
        }
    }
}

impl ToSql for AssetKind {
    fn to_sql(&self) -> Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::Borrowed(ValueRef::Text(
            self.as_db_str().as_bytes(),
        )))
    }
}

impl FromSql for AssetKind {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        let text = value.as_str()?;
        Self::from_db_str(text).ok_or_else(|| {
            FromSqlError::Other(format!("unknown asset kind column value: {text}").into())
        })
    }
}

// ---------------------------------------------------------------------------
// 仓储方法
// ---------------------------------------------------------------------------

impl Db {
    /// 建导入任务（status='running'，started_at=now），返回 job_id。
    /// （引擎走 [`Db::create_job_with_plan`]；本方法保留为仓储基元，测试覆盖。）
    #[allow(dead_code)]
    pub fn create_job(
        &self,
        kind: &str,
        device_id: &str,
        device_name: &str,
        total_files: u64,
        total_bytes: u64,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6)",
            params![
                kind,
                device_id,
                device_name,
                total_files as i64,
                total_bytes as i64,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 建导入任务并保存 ImportPlan JSON（断点恢复/失败重试时重建引擎）。
    pub fn create_job_with_plan(
        &self,
        kind: &str,
        device_id: &str,
        device_name: &str,
        total_files: u64,
        total_bytes: u64,
        plan_json: &str,
    ) -> Result<i64> {
        self.0.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             plan_json, started_at) VALUES (?1, ?2, ?3, 'running', ?4, ?5, ?6, ?7)",
            params![
                kind,
                device_id,
                device_name,
                total_files as i64,
                total_bytes as i64,
                plan_json,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 读取任务的 ImportPlan JSON（无则 None）。
    pub fn job_plan_json(&self, job_id: i64) -> Result<Option<String>> {
        let mut stmt = self.0.prepare("SELECT plan_json FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query(params![job_id])?;
        match rows.next()? {
            Some(row) => Ok(row.get(0)?),
            None => Err(Error::QueryReturnedNoRows),
        }
    }

    /// 任务的设备标识（resume 校验源一致性）。
    pub fn job_device(&self, job_id: i64) -> Result<Option<(String, String)>> {
        let mut stmt = self
            .0
            .prepare("SELECT device_id, device_name FROM jobs WHERE id = ?1")?;
        let mut rows = stmt.query_map(params![job_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 收尾任务：写终态（受 status CHECK 约束）、stats_json、finished_at。
    pub fn finish_job(&self, job_id: i64, status: &str, stats_json: &str) -> Result<()> {
        self.0.execute(
            "UPDATE jobs SET status = ?2, stats_json = ?3, finished_at = ?4 WHERE id = ?1",
            params![job_id, status, stats_json, now_rfc3339()],
        )?;
        Ok(())
    }

    /// journal 落状态：PK(job_id, src) 冲突时整行覆盖。
    pub fn upsert_job_file(&self, row: &JobFileRow) -> Result<()> {
        let sha256: Option<&[u8]> = row.sha256.as_ref().map(|sha| sha.as_slice());
        self.0.execute(
            "INSERT INTO job_files (job_id, src, dst, size, state, error, xxhash, sha256, dst2) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9) \
             ON CONFLICT (job_id, src) DO UPDATE SET \
             dst = excluded.dst, size = excluded.size, state = excluded.state, \
             error = excluded.error, xxhash = excluded.xxhash, sha256 = excluded.sha256, \
             dst2 = excluded.dst2",
            params![
                row.job_id,
                row.src,
                row.dst,
                row.size as i64,
                row.state,
                row.error,
                row.xxhash.map(|v| v as i64),
                sha256,
                row.dst2
            ],
        )?;
        Ok(())
    }

    /// 断点恢复：取 pending / failed 的文件（按 src 有序）。
    /// （引擎用 [`Db::all_job_files`] 自行分流；保留为仓储基元，测试覆盖。）
    #[allow(dead_code)]
    pub fn pending_job_files(&self, job_id: i64) -> Result<Vec<JobFileRow>> {
        let mut stmt = self.0.prepare(
            "SELECT job_id, src, dst, size, state, error, xxhash, sha256, dst2 FROM job_files \
             WHERE job_id = ?1 AND state IN (?2, ?3) ORDER BY src",
        )?;
        let rows = stmt.query_map(
            params![job_id, FileState::Pending, FileState::Failed],
            map_job_file,
        )?;
        rows.collect()
    }

    /// 任务全部 journal 行（resume 重建统计基线；按 src 有序）。
    pub fn all_job_files(&self, job_id: i64) -> Result<Vec<JobFileRow>> {
        let mut stmt = self.0.prepare(
            "SELECT job_id, src, dst, size, state, error, xxhash, sha256, dst2 FROM job_files \
             WHERE job_id = ?1 ORDER BY src",
        )?;
        let rows = stmt.query_map(params![job_id], map_job_file)?;
        rows.collect()
    }

    /// 按状态分组计数（总结弹窗 / 进度统计）。
    #[allow(dead_code)]
    pub fn job_file_counts(&self, job_id: i64) -> Result<Vec<(FileState, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT state, COUNT(*) FROM job_files WHERE job_id = ?1 GROUP BY state ORDER BY state",
        )?;
        let rows = stmt.query_map(params![job_id], |row| {
            Ok((row.get::<_, FileState>(0)?, row.get::<_, i64>(1)? as u64))
        })?;
        rows.collect()
    }

    /// 任务列表 keyset 分页：id 严格大于 after_id，升序取 limit 条。
    pub fn jobs_page(&self, after_id: i64, limit: u32) -> Result<Vec<JobRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, kind, device_id, device_name, status, total_files, total_bytes, \
             stats_json, started_at, finished_at FROM jobs \
             WHERE id > ?1 ORDER BY id ASC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![after_id, limit], |row| {
            Ok(JobRow {
                id: row.get(0)?,
                kind: row.get(1)?,
                device_id: row.get(2)?,
                device_name: row.get(3)?,
                status: row.get(4)?,
                total_files: row.get::<_, i64>(5)? as u64,
                total_bytes: row.get::<_, i64>(6)? as u64,
                stats_json: row.get(7)?,
                started_at: row.get(8)?,
                finished_at: row.get(9)?,
            })
        })?;
        rows.collect()
    }

    /// 资产入库：同路径重复导入整行覆盖。
    pub fn insert_asset(&self, a: &AssetRow) -> Result<()> {
        self.0.execute(
            "INSERT OR REPLACE INTO assets \
             (path, filename, size, mtime, xxhash, sha256, kind, captured_at, camera, source, \
             created_at, origin) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                a.path,
                a.filename,
                a.size as i64,
                a.mtime,
                a.xxhash as i64,
                a.sha256.as_slice(),
                a.kind,
                a.captured_at,
                a.camera,
                a.source,
                a.created_at,
                a.origin
            ],
        )?;
        Ok(())
    }

    /// 查重索引：同 (size, xxhash) 的既有资产 id（导入前快速预判）。
    pub fn find_asset_by_size_xxh(&self, size: u64, xxhash: u64) -> Result<Option<i64>> {
        let mut stmt = self
            .0
            .prepare("SELECT id FROM assets WHERE size = ?1 AND xxhash = ?2 LIMIT 1")?;
        let mut rows = stmt.query(params![size as i64, xxhash as i64])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 宽松查重键（T7 §查重① / T8 new_files 预判）：size + filename + mtime ±2s。
    /// RFC3339 定宽字符串按字典序比较即时间序。
    pub fn find_asset_loose(
        &self,
        size: u64,
        filename: &str,
        mtime_from: &str,
        mtime_to: &str,
    ) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT id FROM assets WHERE size = ?1 AND filename = ?2 \
             AND mtime >= ?3 AND mtime <= ?4 LIMIT 1",
        )?;
        let mut rows = stmt.query(params![size as i64, filename, mtime_from, mtime_to])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 路径是否已在库中（同路径同名查重层；引擎现用文件系统直查）。
    #[allow(dead_code)]
    pub fn asset_path_exists(&self, path: &str) -> Result<bool> {
        let mut stmt = self
            .0
            .prepare("SELECT 1 FROM assets WHERE path = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![path])?;
        Ok(rows.next()?.is_some())
    }

    /// 按路径取资产 id（F1 清卡：journal dst → 库内资产映射）；无则 None。
    pub fn asset_id_by_path(&self, path: &str) -> Result<Option<i64>> {
        let mut stmt = self
            .0
            .prepare("SELECT id FROM assets WHERE path = ?1 LIMIT 1")?;
        let mut rows = stmt.query(params![path])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 失败重试：把 old 任务中 failed 的文件复制为 new 任务的 pending 行
    /// （沿用 kind/device/plan），返回 new job_id；无 failed 行返回 None。
    pub fn retry_failed_into_new_job(&self, old_job_id: i64) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare(
            "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM job_files \
             WHERE job_id = ?1 AND state = 'failed'",
        )?;
        let (count, bytes): (i64, i64) =
            stmt.query_row(params![old_job_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        if count == 0 {
            return Ok(None);
        }
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO jobs (kind, device_id, device_name, status, total_files, total_bytes, \
             plan_json, started_at) \
             SELECT kind, device_id, device_name, 'running', ?2, ?3, plan_json, ?4 \
             FROM jobs WHERE id = ?1",
            params![old_job_id, count, bytes, now_rfc3339()],
        )?;
        let new_id = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO job_files (job_id, src, dst, size, state) \
             SELECT ?2, src, dst, size, 'pending' FROM job_files \
             WHERE job_id = ?1 AND state = 'failed'",
            params![old_job_id, new_id],
        )?;
        tx.commit()?;
        Ok(Some(new_id))
    }

    /// 追加日志（ts=now）。
    pub fn append_log(&self, level: &str, job_id: Option<i64>, msg: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO logs (ts, level, job_id, message) VALUES (?1, ?2, ?3, ?4)",
            params![now_rfc3339(), level, job_id, msg],
        )?;
        Ok(())
    }

    /// 日志游标分页：job 内 id 严格大于 after_id，升序取 limit 条。
    pub fn logs_page(&self, job_id: i64, after_id: i64, limit: u32) -> Result<Vec<LogRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, ts, level, job_id, message FROM logs \
             WHERE job_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![job_id, after_id, limit], |row| {
            Ok(LogRow {
                id: row.get(0)?,
                ts: row.get(1)?,
                level: row.get(2)?,
                job_id: row.get(3)?,
                message: row.get(4)?,
            })
        })?;
        rows.collect()
    }
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn map_job_file(row: &Row<'_>) -> Result<JobFileRow> {
    Ok(JobFileRow {
        job_id: row.get(0)?,
        src: row.get(1)?,
        dst: row.get(2)?,
        size: row.get::<_, i64>(3)? as u64,
        state: row.get(4)?,
        error: row.get(5)?,
        xxhash: row.get::<_, Option<i64>>(6)?.map(|v| v as u64),
        sha256: blob_to_sha256(row.get(7)?)?,
        dst2: row.get(8)?,
    })
}

fn blob_to_sha256(bytes: Option<Vec<u8>>) -> Result<Option<[u8; 32]>> {
    match bytes {
        None => Ok(None),
        Some(bytes) => {
            let sha: [u8; 32] = bytes.try_into().map_err(|bad: Vec<u8>| {
                Error::FromSqlConversionFailure(
                    bad.len(),
                    Type::Blob,
                    format!("sha256 blob must be 32 bytes, got {}", bad.len()).into(),
                )
            })?;
            Ok(Some(sha))
        }
    }
}
