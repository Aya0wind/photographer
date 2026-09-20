//! SQLite 基础 + journal（spec §5.4）：M1 T2 由核心 lane 实现。
//!
//! - [`Db::open`]：WAL + `foreign_keys=ON` + `busy_timeout=5s`（多连接并发）。
//! - [`Db::migrate`]：`PRAGMA user_version` 驱动的内嵌迁移（SQL 在 [`migrations`]，只加不改）。
//! - jobs / job_files（断点恢复 journal）/ assets / logs 的仓储方法。
//!
//! [`FileState`]/[`AssetKind`] 复用 events 模块的领域枚举，列存储格式与其
//! serde camelCase 字符串严格一致（手写 rusqlite To/FromSql 映射，不引 derive 扩展 crate）。

mod migrations;

use std::collections::HashMap;
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
    // —— 0004 起的可空列（存量资产全 None，不回填）——
    /// 像素宽/高（EXIF SOF 或 TIFF 尺寸 tag）。
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
    /// 感光度。
    #[serde(default)]
    pub iso: Option<u32>,
    /// 光圈（展示态，如 "2.8"）。
    #[serde(default)]
    pub f_number: Option<String>,
    /// 快门（展示态，如 "1/250"）。
    #[serde(default)]
    pub exposure_time: Option<String>,
    /// 焦距 mm（展示态，如 "85"）。
    #[serde(default)]
    pub focal_length: Option<String>,
    /// 镜头型号。
    #[serde(default)]
    pub lens: Option<String>,
    /// RAW/JPG 配对（同目录同 stem 另一格式资产的 id；导入入册时双向写）。
    #[serde(default)]
    pub pair_asset_id: Option<i64>,
    /// 缩略图状态镜像（index_tasks 的 O(1) 读路径）：
    /// 0=pending 1=done 2=permanent-none（视频/不可解码/生成失败）。
    #[serde(default)]
    pub thumb_state: i32,
    // —— 0008 起的深提取可空列（存量资产经 exif-gen-2 代际回填补齐）——
    /// 拍摄方向 EXIF 1-8。
    #[serde(default)]
    pub orientation: Option<i64>,
    /// 闪光灯 token（"fired" 族 / "no_flash" 族，见 exif_lite 映射）。
    #[serde(default)]
    pub flash: Option<String>,
    /// 测光模式 token（average/center_weighted/spot/pattern…）。
    #[serde(default)]
    pub metering_mode: Option<String>,
    /// 白平衡 token（auto/manual）。
    #[serde(default)]
    pub white_balance: Option<String>,
    /// 曝光程序 token（manual/aperture_priority/shutter_priority…）。
    #[serde(default)]
    pub exposure_program: Option<String>,
    /// 处理软件（IFD0 Software）。
    #[serde(default)]
    pub software: Option<String>,
    /// 作者（IFD0 Artist）。
    #[serde(default)]
    pub artist: Option<String>,
    /// GPS 纬度（十进制度，南纬为负）。
    #[serde(default)]
    pub gps_lat: Option<f64>,
    /// GPS 经度（十进制度，西经为负）。
    #[serde(default)]
    pub gps_lon: Option<f64>,
}

/// 索引任务行（index_tasks；导入/索引任务分离后的资产级待办）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexTaskRow {
    pub id: i64,
    /// "thumb" | "exif" | "ai" | "face"（通道路由：thumb/exif=CPU 全核；ai/face
    /// =AI 推理通道，CPU 串行；kind 集合由 migration 0006 扩展）。
    pub kind: String,
    pub asset_id: i64,
    /// "pending" | "running" | "done" | "failed"
    pub state: String,
    pub attempts: i64,
    pub created_at: String,
    pub updated_at: String,
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
// 画廊查询（M3）：分页 / 日期分组 / 详情
// ---------------------------------------------------------------------------

/// captured_at 为 NULL 的高哨兵（RFC3339 字典序最大）：COALESCE 归一后
/// `ORDER BY k DESC` 即「NULL 最先，随后 captured 降序」，keyset 游标
/// 退化为 (k, id) 二元组比较（真机契约 2026-09-19：117 资产 5 NULL）。
pub const CAPTURED_NULL_HIGH: &str = "9999-12-31T23:59:59.999Z";

/// 资产分页过滤（IPC 载荷，camelCase）。日期为 RFC3339 字符串或纯日期
/// `YYYY-MM-DD`（IPC 层归一定宽 UTC：纯日期 after=当日 00:00、before=当日
/// 23:59:59.999 本地时区），字典序比较即时间序；任一日期过滤出现时
/// NULL captured_at 的行被排除（无日期不落任何区间）。`kinds` 多选（SQL IN，
/// 空 = 不过滤；用户分类语义「照片」=photo+raw 由前端传 [photo,raw]）；
/// `cameras` 多选 OR（搜索页相机勾选）。
///
/// # M5 扩展（数值/布尔条件）与 NULL 语义（统一约定）
/// 数值范围条件对列为 NULL 的行**不匹配**——「未知」不冒充任何区间
/// （如 focal_length NULL 的资产在任何 focalMin/focalMax 组合下都排除）。
/// 布尔条件同理只在已知值上判定：flash="on" → flash 归 "fired" 族；
/// flash="off" → flash 已知且非 fired 族；flash="unknown" → flash IS NULL。
/// hasGps 按 gps_lat IS NOT NULL。orientation 的
/// landscape/portrait 按 width/height 数值比较（任一 NULL 排除）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AssetFilters {
    /// 格式分类多选（空 = 不过滤）。
    pub kinds: Vec<AssetKind>,
    pub captured_after: Option<String>,
    pub captured_before: Option<String>,
    /// 相机型号多选（OR 语义；空 = 不过滤）。
    pub cameras: Vec<String>,
    /// 镜头型号多选（OR 语义；上限 16 同 cameras）。
    pub lenses: Vec<String>,
    /// 焦距范围 mm（闭区间；对 focal_length TEXT 列 CAST REAL 比较）。
    pub focal_min: Option<f64>,
    pub focal_max: Option<f64>,
    /// 感光度范围（闭区间）。
    pub iso_min: Option<i64>,
    pub iso_max: Option<i64>,
    /// 光圈范围 f/（闭区间；f_number TEXT CAST REAL）。
    pub aperture_min: Option<f64>,
    pub aperture_max: Option<f64>,
    /// 快门范围（**秒**，闭区间；shutterMin=最慢下限 shutterMax=最快上限，
    /// 对 "1/250" 展示串按写入格式解析为秒后比较）。
    pub shutter_min: Option<f64>,
    pub shutter_max: Option<f64>,
    /// 闪光灯三态："on" → fired 族；"off" → 已知未闪光；"unknown" →
    /// flash IS NULL（无 EXIF/未提取）；None → 不过滤。其他值容错不过滤。
    pub flash: Option<String>,
    /// "landscape"（width>height）| "portrait"（height>width）。
    pub orientation: Option<String>,
    /// GPS：true → gps_lat 非空；false → gps_lat 为空。
    pub has_gps: Option<bool>,
    /// 文件格式多选（扩展名，大小写不敏感尾部匹配；上限 16）。
    pub formats: Vec<String>,
    /// 文件大小范围（字节，闭区间）。
    pub size_min: Option<i64>,
    pub size_max: Option<i64>,
}

/// 分页行（画廊网格数据源）。
#[derive(Debug, Clone, PartialEq)]
pub struct AssetPageRow {
    pub id: i64,
    pub path: String,
    pub filename: String,
    pub size: u64,
    pub kind: AssetKind,
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub iso: Option<u32>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub lens: Option<String>,
    /// RAW/JPG 配对资产 id（无配对为 None）。
    pub pair_id: Option<i64>,
    /// 缩略图状态（0 pending / 1 done / 2 permanent-none）。
    pub thumb_state: i32,
}

/// 相机聚合行（搜索页相机勾选数据源）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCountRow {
    pub camera: String,
    pub count: u64,
}

/// 人物簇行（people_list 数据源 / IPC 载荷，camelCase）：名称可空 = 未命名；
/// 封面资产 = 封面人脸所在资产（封面人脸被级联删除时回退簇内最大框人脸）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonRow {
    pub id: i64,
    pub name: Option<String>,
    pub face_count: u64,
    pub cover_asset_id: Option<i64>,
}

/// 日期分组行（画廊吸顶 + 跳转）。
#[derive(Debug, Clone, PartialEq)]
pub struct DateGroupRow {
    /// 本地时区日期 `YYYY-MM-DD`；captured_at 为 NULL 的资产归 "unknown" 组。
    pub date: String,
    pub count: u64,
    /// 组内同排序首张（最新/最大 id）的资产 id。
    pub cover_asset_id: i64,
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

    /// 画廊 keyset 分页：序 = (COALESCE(captured_at, 哨兵) DESC, id DESC)，
    /// 即 NULL captured_at 最先、随后拍摄时间降序、id 倒序 tiebreak。
    /// 游标 after_id 为上一页末行 id（0 = 第一页）；游标行已不存在时按第一页
    /// 处理。全参数化：所有过滤条件经动态槽位构造（`?N` 显式编号 + 同序
    /// push，绑定按编号而非文本位置），零值拼接。M5 扩展条件的 NULL 语义见
    /// [`AssetFilters`] 注释。
    pub fn assets_page(
        &self,
        after_id: i64,
        limit: u32,
        filters: &AssetFilters,
    ) -> Result<Vec<AssetPageRow>> {
        // 游标键解析：after 行的归一排序键（NULL → 高哨兵）
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.sort_key_of(after_id)? {
                Some(key) => (key, after_id),
                None => (CAPTURED_NULL_HIGH.to_string(), 0), // 行已删：回退第一页
            }
        } else {
            (CAPTURED_NULL_HIGH.to_string(), 0)
        };
        use rusqlite::types::Value as V;
        let mut conds: Vec<String> = Vec::new();
        let mut params_vec: Vec<V> = Vec::new();
        let mut next: usize = 1;
        // 统一槽位构造：?N 显式编号 + params_vec 同序 push（按编号绑定）
        let mut slot = |params_vec: &mut Vec<V>, v: V| -> String {
            let s = format!("?{next}");
            next += 1;
            params_vec.push(v);
            s
        };

        // —— 排序哨兵（子查询内 COALESCE 的 NULL 归一）——
        let sentinel_slot = slot(&mut params_vec, V::from(CAPTURED_NULL_HIGH.to_string()));

        // —— 多选 IN / 尾部匹配（各上限 16；空 = 不过滤）——
        let kinds: Vec<AssetKind> = filters.kinds.iter().take(16).copied().collect();
        let cameras: Vec<String> = filters.cameras.iter().take(16).cloned().collect();
        let lenses: Vec<String> = filters.lenses.iter().take(16).cloned().collect();
        let formats: Vec<String> = filters
            .formats
            .iter()
            .map(|f| f.trim().trim_start_matches('.').to_string())
            .filter(|f| !f.is_empty())
            .take(16)
            .collect();
        if !kinds.is_empty() {
            let slots = kinds
                .iter()
                .map(|k| slot(&mut params_vec, V::from(k.as_db_str().to_string())))
                .collect::<Vec<_>>()
                .join(", ");
            conds.push(format!("kind IN ({slots})"));
        }
        if !cameras.is_empty() {
            let slots = cameras
                .iter()
                .map(|c| slot(&mut params_vec, V::from(c.clone())))
                .collect::<Vec<_>>()
                .join(", ");
            conds.push(format!("camera IN ({slots})"));
        }
        if !lenses.is_empty() {
            let slots = lenses
                .iter()
                .map(|l| slot(&mut params_vec, V::from(l.clone())))
                .collect::<Vec<_>>()
                .join(", ");
            conds.push(format!("lens IN ({slots})"));
        }
        if !formats.is_empty() {
            // 扩展名 = 路径尾部 ".ext"（LIKE 对 ASCII 大小写不敏感，等价于
            // lower(扩展名) IN；无扩展名路径不命中任何模式）
            let frags = formats
                .iter()
                .map(|f| {
                    let s = slot(&mut params_vec, V::from(format!("%.{f}")));
                    format!("path LIKE {s}")
                })
                .collect::<Vec<_>>();
            conds.push(format!("({})", frags.join(" OR ")));
        }

        // —— 日期（RFC3339 定宽字典序比较即时间序）——
        if let Some(after) = &filters.captured_after {
            let s = slot(&mut params_vec, V::from(after.clone()));
            conds.push(format!("captured_at >= {s}"));
        }
        if let Some(before) = &filters.captured_before {
            let s = slot(&mut params_vec, V::from(before.clone()));
            conds.push(format!("captured_at <= {s}"));
        }

        // —— 数值范围（闭区间；列 NULL 时比较结果为 NULL → 行被排除，
        //    即「未知」不冒充任何区间，见 AssetFilters 注释）——
        let mut range = |conds: &mut Vec<String>,
                         params_vec: &mut Vec<V>,
                         expr: &str,
                         min: Option<f64>,
                         max: Option<f64>| {
            for (bound, cmp) in [(min, ">="), (max, "<=")] {
                if let Some(v) = bound {
                    let s = slot(params_vec, V::from(v));
                    conds.push(format!("{expr} {cmp} {s}"));
                }
            }
        };
        range(
            &mut conds,
            &mut params_vec,
            "CAST(focal_length AS REAL)",
            filters.focal_min,
            filters.focal_max,
        );
        range(
            &mut conds,
            &mut params_vec,
            "CAST(iso AS REAL)",
            filters.iso_min.map(|v| v as f64),
            filters.iso_max.map(|v| v as f64),
        );
        range(
            &mut conds,
            &mut params_vec,
            "CAST(f_number AS REAL)",
            filters.aperture_min,
            filters.aperture_max,
        );
        // 快门秒数：exposure_time 为本应用写入的展示串（"1/250" / "0.4"），
        // 按写入格式解析——分数串取倒数，其余 CAST REAL。
        range(
            &mut conds,
            &mut params_vec,
            "CASE WHEN exposure_time LIKE '1/%' THEN 1.0 / CAST(substr(exposure_time, 3) AS REAL) ELSE CAST(exposure_time AS REAL) END",
            filters.shutter_min,
            filters.shutter_max,
        );
        range(
            &mut conds,
            &mut params_vec,
            "CAST(size AS REAL)",
            filters.size_min.map(|v| v as f64),
            filters.size_max.map(|v| v as f64),
        );

        // —— 布尔 / 方向 / GPS ——
        if let Some(state) = &filters.flash {
            // flash token 族见 exif_lite 映射：fired 族均含 "fired" 子串，
            // no_flash 族前缀 "no_flash"；"unknown" 显式取 NULL（未知 ≠ 未闪光）。
            match state.as_str() {
                "on" => {
                    let s = slot(&mut params_vec, V::from("%fired%".to_string()));
                    conds.push(format!("flash LIKE {s}"));
                }
                "off" => {
                    let s = slot(&mut params_vec, V::from("no_flash%".to_string()));
                    conds.push(format!("(flash LIKE {s} AND flash IS NOT NULL)"));
                }
                "unknown" => conds.push("flash IS NULL".to_string()),
                _ => {} // 未知 token 容错：不过滤
            }
        }
        match filters.orientation.as_deref() {
            // width/height 任一 NULL → 比较为 NULL → 排除
            Some("landscape") => conds.push("width > height".into()),
            Some("portrait") => conds.push("height > width".into()),
            _ => {}
        }
        if let Some(has) = filters.has_gps {
            conds.push(if has {
                "gps_lat IS NOT NULL".into()
            } else {
                "gps_lat IS NULL".into()
            });
        }

        // —— 游标（归一排序键二元组；cursor_id=0 即第一页短路）——
        let cursor_id_slot = slot(&mut params_vec, V::from(cursor_id));
        let cursor_key_slot = slot(&mut params_vec, V::from(cursor_key));
        let limit_slot = slot(&mut params_vec, V::from(limit));

        let all = if conds.is_empty() {
            "1 = 1".to_string()
        } else {
            conds.join(" AND ")
        };
        // 过滤条件作用于内层（可引用 assets 全列），游标/排序用外层投影列
        let mut stmt = self.0.prepare(&format!(
            "SELECT id, path, filename, size, kind, captured_at, camera, \
             width, height, iso, f_number, exposure_time, focal_length, lens, pair_asset_id, thumb_state FROM \
             (SELECT a.id, a.path, a.filename, a.size, a.kind, a.captured_at, a.camera, \
              a.width, a.height, a.iso, a.f_number, a.exposure_time, a.focal_length, a.lens, \
              a.pair_asset_id, a.thumb_state, COALESCE(a.captured_at, {sentinel_slot}) AS k \
              FROM assets a WHERE {all}) \
             WHERE ({cursor_id_slot} = 0 OR k < {cursor_key_slot} \
                    OR (k = {cursor_key_slot} AND id < {cursor_id_slot})) \
             ORDER BY k DESC, id DESC LIMIT {limit_slot}",
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), map_asset_page)?;
        rows.collect()
    }

    /// 某资产 id 的归一排序键（行不存在返回 None）。
    fn sort_key_of(&self, id: i64) -> Result<Option<String>> {
        let mut stmt = self
            .0
            .prepare("SELECT COALESCE(captured_at, ?2) FROM assets WHERE id = ?1")?;
        let mut rows = stmt.query(params![id, CAPTURED_NULL_HIGH])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 本地时区日期分组（降序；unknown 组置顶与画廊页序一致）。
    /// date() 无值（NULL）→ 'unknown'；cover 取组内同排序首张
    /// （captured DESC、id DESC）。
    pub fn asset_group_dates(&self) -> Result<Vec<DateGroupRow>> {
        let mut stmt = self.0.prepare(
            "SELECT day, COUNT(*), \
               (SELECT t.id FROM assets t \
                 WHERE COALESCE(date(t.captured_at, 'localtime'), 'unknown') = day \
                 ORDER BY COALESCE(t.captured_at, ?1) DESC, t.id DESC LIMIT 1) \
             FROM (SELECT COALESCE(date(captured_at, 'localtime'), 'unknown') AS day \
                   FROM assets) \
             GROUP BY day ORDER BY (day = 'unknown') DESC, day DESC",
        )?;
        let rows = stmt.query_map(params![CAPTURED_NULL_HIGH], |row| {
            Ok(DateGroupRow {
                date: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
                cover_asset_id: row.get(2)?,
            })
        })?;
        rows.collect()
    }

    /// 按 id 取完整资产行（详情数据源）。
    pub fn asset_by_id(&self, id: i64) -> Result<Option<AssetRow>> {
        let mut stmt = self.0.prepare(
            "SELECT path, filename, size, mtime, xxhash, sha256, kind, captured_at, camera, \
             source, created_at, origin, width, height, iso, f_number, exposure_time, \
             focal_length, lens, pair_asset_id, thumb_state, orientation, flash, \
             metering_mode, white_balance, exposure_program, software, artist, \
             gps_lat, gps_lon FROM assets WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(map_asset_full(row)?)),
            None => Ok(None),
        }
    }

    /// 库内同指纹 (size, xxhash) 的**其他**资产数（不含自身）。
    pub fn asset_duplicate_count(&self, id: i64, size: u64, xxhash: u64) -> Result<u64> {
        let mut stmt = self
            .0
            .prepare("SELECT COUNT(*) FROM assets WHERE size = ?1 AND xxhash = ?2 AND id != ?3")?;
        let count: i64 = stmt.query_row(params![size as i64, xxhash as i64, id], |r| r.get(0))?;
        Ok(count as u64)
    }

    /// 资产入库：同路径重复导入整行覆盖（REPLACE 换 id 时自动重指
    /// pair_asset_id 既有引用），随后按 (同目录, 同 stem, 异扩展名) 双向
    /// 写 RAW/JPG 配对（pair_asset_id）。
    pub fn insert_asset(&self, a: &AssetRow) -> Result<()> {
        let old_id: Option<i64> = self
            .0
            .query_row("SELECT id FROM assets WHERE path = ?1", [&a.path], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        self.0.execute(
            "INSERT OR REPLACE INTO assets \
             (path, filename, size, mtime, xxhash, sha256, kind, captured_at, camera, source, \
             created_at, origin, width, height, iso, f_number, exposure_time, focal_length, \
             lens, pair_asset_id, thumb_state, orientation, flash, metering_mode, \
             white_balance, exposure_program, software, artist, gps_lat, gps_lon) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
             ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30)",
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
                a.origin,
                a.width.map(|v| v as i64),
                a.height.map(|v| v as i64),
                a.iso.map(|v| v as i64),
                a.f_number,
                a.exposure_time,
                a.focal_length,
                a.lens,
                a.pair_asset_id,
                a.thumb_state,
                a.orientation,
                a.flash,
                a.metering_mode,
                a.white_balance,
                a.exposure_program,
                a.software,
                a.artist,
                a.gps_lat,
                a.gps_lon,
            ],
        )?;
        let id = self
            .asset_id_by_path(&a.path)?
            .ok_or(Error::QueryReturnedNoRows)?;
        if let Some(old) = old_id {
            if old != id {
                self.0.execute(
                    "UPDATE assets SET pair_asset_id = ?2 WHERE pair_asset_id = ?1 AND id != ?2",
                    params![old, id],
                )?;
            }
        }
        self.refresh_asset_pair(id, &a.path)?;

        // 索引待办（导入/索引任务分离）：photo/raw 写 thumb 任务；video/
        // other 直接永久占位（无缩略图可言）。REPLACE 旧资产行时其任务行
        // 随 ON DELETE CASCADE 消失，这里只补新行。
        if matches!(a.kind, AssetKind::Photo | AssetKind::Raw) {
            let now = now_rfc3339();
            self.0.execute(
                "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
                 VALUES ('thumb', ?1, 'pending', 0, ?2, ?2)",
                params![id, now],
            )?;
        } else {
            self.0.execute(
                "UPDATE assets SET thumb_state = 2 WHERE id = ?1",
                params![id],
            )?;
        }
        Ok(())
    }

    /// 按 (同目录, 同 stem, 异扩展名) 找配对伙伴并双向写 pair_asset_id；
    /// 多候选取 id 最小（先入册者）。目录前缀 = 原样前缀（含分隔符）匹配，
    /// stem 大小写折叠（Windows 路径大小写不敏感）。
    fn refresh_asset_pair(&self, id: i64, path: &str) -> Result<()> {
        let (dir, stem, _ext) = split_dir_stem_ext(path);
        if stem.is_empty() || dir.is_empty() {
            return Ok(());
        }
        let prefix_chars = dir.chars().count(); // dir 已含尾分隔符
        let stem_chars = stem.chars().count();
        let mut stmt = self.0.prepare(
            "SELECT id FROM assets WHERE id != ?1 \
             AND substr(path, 1, ?2) = ?3 \
             AND lower(substr(path, ?2 + 1, ?4 + 1)) = lower(?5) \
             AND length(substr(path, ?2 + 1)) > ?4 + 1 \
             ORDER BY id LIMIT 1",
        )?;
        let partner: Option<i64> = stmt
            .query_row(
                params![
                    id,
                    prefix_chars as i64,
                    dir,
                    stem_chars as i64,
                    format!("{stem}.")
                ],
                |r| r.get(0),
            )
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        self.0.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
            params![id, partner],
        )?;
        if let Some(p) = partner {
            self.0.execute(
                "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
                params![p, id],
            )?;
        }
        Ok(())
    }

    // —— 索引任务（index_tasks）——

    /// 为缩略图状态仍未完成、但任务账缺失的资产补种待办。旧版本清理过
    /// done 行，因此不能只依赖 index_tasks 判断是否已经生成。
    pub fn create_thumb_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.thumb_state = 0 AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 认领一条 pending 任务（原子置 running；多 worker 并发认领不重不漏，
    /// SQLite 写锁串行化保证单行独占）。无待办返回 None。
    pub fn claim_index_task(&self, kind: &str) -> Result<Option<IndexTaskRow>> {
        let now = now_rfc3339();
        let mut stmt = self.0.prepare(
            "UPDATE index_tasks SET state = 'running', updated_at = ?2 \
             WHERE id = (SELECT id FROM index_tasks WHERE state = 'pending' \
                         AND kind = ?1 ORDER BY id LIMIT 1) \
             RETURNING id, kind, asset_id, state, attempts, created_at, updated_at",
        )?;
        let mut rows = stmt.query_map(params![kind, now], map_index_task)?;
        match rows.next() {
            Some(row) => Ok(Some(row?)),
            None => Ok(None),
        }
    }

    /// 为语义回填建任务：`ai_indexed_at IS NULL` 的库内照片（photo/raw）
    /// 且无未完成 ai 任务的行，逐行插 pending ai 任务；返回建任务数。
    pub fn create_ai_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'ai', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.ai_indexed_at IS NULL AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'ai' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 语义嵌入记账（usearch 落盘成功后调用）。
    pub fn set_ai_indexed(&self, id: i64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET ai_indexed_at = ?2 WHERE id = ?1",
            params![id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 语义索引的持久化进度（已记账资产 / 可索引资产）。手动与自动触发共用
    /// 同一张任务账和资产账，事件进度必须从这里的基线继续累加。
    pub fn ai_index_progress(&self) -> Result<(u64, u64)> {
        let (done, total): (i64, i64) = self.0.query_row(
            "SELECT COUNT(CASE WHEN ai_indexed_at IS NOT NULL THEN 1 END), COUNT(*) \
             FROM assets WHERE kind IN ('photo', 'raw')",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((done as u64, total as u64))
    }

    /// 语义检索 join：资产是否存在且可检索（photo/raw）。
    pub fn asset_searchable(&self, id: i64) -> Result<bool> {
        let ok: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE id = ?1 AND kind IN ('photo', 'raw')",
            params![id],
            |r| r.get(0),
        )?;
        Ok(ok > 0)
    }

    /// 启动恢复：上次中断遗留的 running 复位为 pending，返回复位条数。
    pub fn reclaim_running_index_tasks(&self) -> Result<usize> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', updated_at = ?1 WHERE state = 'running'",
            params![now],
        )
    }

    /// 指定通道待办数（pending；回填 worker 的启动判据——不能依赖本轮
    /// 新建任务数：存量 pending 也必须有人消费，否则重复 kick 全部空转）。
    pub fn pending_index_task_count(&self, kind: &str) -> Result<u64> {
        let count: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = ?1 AND state = 'pending'",
            params![kind],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    /// RAW 缩略图源代际升级自愈：重排全部 RAW 的 thumb 任务（done/failed
    /// 复位 pending + 为无任务行的新建），返回待处理数。照片任务不动。
    pub fn requeue_thumb_tasks_for_raw(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1 \
             WHERE kind = 'thumb' AND state != 'pending' \
             AND asset_id IN (SELECT id FROM assets WHERE kind = 'raw')",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.kind = 'raw' \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("thumb")
    }

    /// 手动“立即索引”重试：复用已有失败任务，不再为同一资产重复插行。
    /// 历史版本可能已经为同一资产制造多条 failed，先压成每资产一条；
    /// attempts 清零后仍沿用单轮最多三次的坏文件熔断策略。
    pub fn retry_failed_index_tasks(&self, kind: &str) -> Result<usize> {
        self.0.execute(
            "DELETE FROM index_tasks WHERE kind = ?1 AND state = 'failed' \
             AND id NOT IN (SELECT MAX(id) FROM index_tasks \
                            WHERE kind = ?1 AND state = 'failed' GROUP BY asset_id)",
            params![kind],
        )?;
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?2 \
             WHERE kind = ?1 AND state = 'failed'",
            params![kind, now_rfc3339()],
        )
    }

    /// 待办统计（pending+running；启动恢复事件/前端提示数据源）。
    pub fn pending_index_count(&self) -> Result<u64> {
        let count: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE state IN ('pending', 'running')",
            [],
            |r| r.get(0),
        )?;
        Ok(count as u64)
    }

    /// 任务收尾：done（成果落 assets 列由调用方先写）或 failed（attempts+1，
    /// 未达 3 次封顶回 pending 自动重试，达 3 次保持 failed——防坏死资产
    /// 无限循环）。
    pub fn finish_index_task(&self, id: i64, ok: bool) -> Result<()> {
        let now = now_rfc3339();
        if ok {
            self.0.execute(
                "UPDATE index_tasks SET state = 'done', updated_at = ?2 WHERE id = ?1",
                params![id, now],
            )?;
        } else {
            self.0.execute(
                "UPDATE index_tasks SET attempts = attempts + 1, updated_at = ?2, \
                 state = CASE WHEN attempts + 1 >= 3 THEN 'failed' ELSE 'pending' END \
                 WHERE id = ?1",
                params![id, now],
            )?;
        }
        Ok(())
    }

    /// 资产缩略图状态镜像更新（index worker 成果）。
    pub fn set_thumb_state(&self, id: i64, state: i32) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET thumb_state = ?2 WHERE id = ?1",
            params![id, state],
        )?;
        Ok(())
    }

    /// 按需兜底通道的快路径：资产 (path, thumb_state)；无资产返回 None。
    pub fn thumb_info_by_id(&self, id: i64) -> Result<Option<(String, i32)>> {
        let mut stmt = self
            .0
            .prepare("SELECT path, thumb_state FROM assets WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some((
                row.get(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or(0) as i32,
            ))),
            None => Ok(None),
        }
    }

    // —— 人脸 / 人物簇（M4，faces + people 表）——

    /// 新建未命名人物簇，返回簇 id。
    pub fn create_person(&self) -> Result<i64> {
        self.0.execute(
            "INSERT INTO people (name, cover_face_id, created_at) VALUES (NULL, NULL, ?1)",
            params![now_rfc3339()],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 人物重命名（空串归一为 NULL = 未命名）；簇不存在报错。
    pub fn rename_person(&self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        let name: Option<&str> = if name.is_empty() { None } else { Some(name) };
        let n = self.0.execute(
            "UPDATE people SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 删除人物簇（删簇不删脸数据：faces.cluster_id 经 FK ON DELETE SET NULL
    /// 解除归属）；簇不存在报错。
    pub fn delete_person(&self, id: i64) -> Result<()> {
        let n = self
            .0
            .execute("DELETE FROM people WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 落一条人脸（SCRFD 框 + ArcFace 特征 + 簇归属），返回 face id。
    /// embedding 存 f32 小端字节流（512×4 = 2048 B blob）。
    #[allow(clippy::too_many_arguments)]
    pub fn insert_face(
        &self,
        asset_id: i64,
        box_x: f64,
        box_y: f64,
        box_w: f64,
        box_h: f64,
        embedding: &[f32],
        cluster_id: Option<i64>,
    ) -> Result<i64> {
        let blob: Vec<u8> = embedding.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.0.execute(
            "INSERT INTO faces (asset_id, box_x, box_y, box_w, box_h, embedding, cluster_id, \
             created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                asset_id,
                box_x,
                box_y,
                box_w,
                box_h,
                blob,
                cluster_id,
                now_rfc3339()
            ],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    /// 封面晋升：新脸面积大于当前封面脸时替换（首张无条件担任封面）。
    pub fn maybe_promote_cover(&self, face_id: i64, cluster_id: i64, area: f64) -> Result<()> {
        self.0.execute(
            "UPDATE people SET cover_face_id = ?1 \
             WHERE id = ?2 AND (cover_face_id IS NULL OR \
               ?3 > (SELECT f.box_w * f.box_h FROM faces f \
                     WHERE f.id = people.cover_face_id))",
            params![face_id, cluster_id, area],
        )?;
        Ok(())
    }

    /// 各簇特征向量和（在线聚类簇心的增量基元）：cluster_id → (向量和, 成员数)。
    pub fn face_cluster_sums(&self) -> Result<HashMap<i64, (Vec<f32>, u32)>> {
        let mut stmt = self
            .0
            .prepare("SELECT cluster_id, embedding FROM faces WHERE cluster_id IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut sums: HashMap<i64, (Vec<f32>, u32)> = HashMap::new();
        for row in rows {
            let (cluster, blob) = row?;
            let entry = sums.entry(cluster).or_insert_with(|| (Vec::new(), 0));
            let chunks: &[[u8; 4]] = blob.as_chunks().0;
            if entry.0.is_empty() {
                entry.0 = vec![0f32; chunks.len()];
            }
            for (dim, chunk) in chunks.iter().enumerate() {
                if let Some(slot) = entry.0.get_mut(dim) {
                    *slot += f32::from_le_bytes(*chunk);
                }
            }
            entry.1 += 1;
        }
        Ok(sums)
    }

    /// 人物簇列表（face_count 降序、id 升序稳定排序；空簇不返回）。
    /// 封面资产 = 封面人脸所在资产，缺失回退簇内最大框人脸的资产。
    pub fn people_list(&self) -> Result<Vec<PersonRow>> {
        let mut stmt = self.0.prepare(
            "SELECT p.id, p.name, COUNT(f.id), \
               (SELECT f2.asset_id FROM faces f2 WHERE f2.id = p.cover_face_id), \
               (SELECT f3.asset_id FROM faces f3 WHERE f3.cluster_id = p.id \
                 ORDER BY f3.box_w * f3.box_h DESC, f3.id ASC LIMIT 1) \
             FROM people p JOIN faces f ON f.cluster_id = p.id \
             GROUP BY p.id \
             ORDER BY COUNT(f.id) DESC, p.id ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PersonRow {
                id: row.get(0)?,
                name: row.get(1)?,
                face_count: row.get::<_, i64>(2)? as u64,
                cover_asset_id: row
                    .get::<_, Option<i64>>(3)?
                    .or(row.get::<_, Option<i64>>(4)?),
            })
        })?;
        rows.collect()
    }

    /// 某人物簇的资产页（去重，captured_at DESC、id DESC tiebreak；NULL
    /// captured_at 最先——与画廊排序契约一致）。
    pub fn assets_by_cluster(&self, cluster_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, path, filename, size, kind, captured_at, camera, width, height, iso, \
             f_number, exposure_time, focal_length, lens, pair_asset_id, thumb_state \
             FROM (SELECT DISTINCT a.id, a.path, a.filename, a.size, a.kind, a.captured_at, \
                    a.camera, a.width, a.height, a.iso, a.f_number, a.exposure_time, \
                    a.focal_length, a.lens, a.pair_asset_id, a.thumb_state, \
                    COALESCE(a.captured_at, ?2) AS k \
                   FROM assets a JOIN faces f ON f.asset_id = a.id \
                   WHERE f.cluster_id = ?1) \
             ORDER BY k DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(
            params![cluster_id, CAPTURED_NULL_HIGH, limit],
            map_asset_page,
        )?;
        rows.collect()
    }

    /// 一键清除人脸数据（事务）：faces + people 清空、face 通道任务清空、
    /// assets.face_indexed_at 复位（可重新回填）。
    pub fn clear_face_data(&self) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM faces", [])?;
        tx.execute("DELETE FROM people", [])?;
        tx.execute("DELETE FROM index_tasks WHERE kind = 'face'", [])?;
        tx.execute("UPDATE assets SET face_indexed_at = NULL", [])?;
        tx.commit()?;
        Ok(())
    }

    /// 为人脸回填建任务：`face_indexed_at IS NULL` 的库内照片（photo/raw）
    /// 且无未完成 face 任务的行，逐行插 pending face 任务；返回建任务数。
    pub fn create_face_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'face', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.face_indexed_at IS NULL AND a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'face' AND t.asset_id = a.id)",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// 人脸处理记账（聚类入库成功后调用）。
    pub fn set_face_indexed(&self, id: i64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET face_indexed_at = ?2 WHERE id = ?1",
            params![id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 各通道任务状态计数（(kind, state, count)，index_status IPC 数据源）。
    pub fn index_task_state_counts(&self) -> Result<Vec<(String, String, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT kind, state, COUNT(DISTINCT asset_id) FROM index_tasks GROUP BY kind, state",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)? as u64,
            ))
        })?;
        rows.collect()
    }

    /// 相机聚合（搜索页相机勾选）：camera 非空分组计数，count 降序、
    /// camera 升序稳定排序。
    pub fn camera_list(&self) -> Result<Vec<CameraCountRow>> {
        let mut stmt = self.0.prepare(
            "SELECT camera, COUNT(*) FROM assets \
             WHERE camera IS NOT NULL AND camera != '' \
             GROUP BY camera ORDER BY COUNT(*) DESC, camera ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }

    /// 镜头聚合（搜索页镜头勾选）：lens 非空分组计数，count 降序、
    /// lens 升序稳定排序（与 camera_list 同构）。
    pub fn lens_list(&self) -> Result<Vec<CameraCountRow>> {
        let mut stmt = self.0.prepare(
            "SELECT lens, COUNT(*) FROM assets \
             WHERE lens IS NOT NULL AND lens != '' \
             GROUP BY lens ORDER BY COUNT(*) DESC, lens ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }

    /// 格式聚合（搜索页格式勾选）：路径最后一个点后的扩展名大写分组，
    /// count 降序、格式升序稳定排序；无扩展名的文件不计入（LIKE '%.%'
    /// 门槛。rtrim 技巧：`rtrim(path, replace(path,'.',''))` 剥掉尾部扩展名字符、
    /// 停在点前（结果**含**该点）→ substr(前缀长 + 1) = ext。
    pub fn format_list(&self) -> Result<Vec<CameraCountRow>> {
        let mut stmt = self.0.prepare(
            "SELECT upper(substr(path, length(rtrim(path, replace(path, '.', ''))) + 1)) AS fmt, \
             COUNT(*) FROM assets \
             WHERE path LIKE '%.%' \
             GROUP BY fmt ORDER BY COUNT(*) DESC, fmt ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(CameraCountRow {
                camera: row.get(0)?,
                count: row.get::<_, i64>(1)? as u64,
            })
        })?;
        rows.collect()
    }
    /// exif 任务深提取成果落库（gen-3 回填 worker / 导入管线共用）。
    /// 真机发现（2026-09-20）：119 张老库 lens/0004 拍摄参数列全空——旧链
    /// 只在导入时提取，存量永远没人补。gen-3 起一并回填（COALESCE 保留
    /// 已有值，提取不到不清空）；captured_at/camera 不动（导入已写对）。
    pub fn update_asset_deep_exif(
        &self,
        id: i64,
        meta: &crate::metadata::exif_lite::MetaLite,
    ) -> Result<()> {
        let deep = &meta.deep;
        self.0.execute(
            "UPDATE assets SET lens = COALESCE(?2, lens), \
             width = COALESCE(?3, width), height = COALESCE(?4, height), \
             iso = COALESCE(?5, iso), f_number = COALESCE(?6, f_number), \
             exposure_time = COALESCE(?7, exposure_time), \
             focal_length = COALESCE(?8, focal_length), \
             orientation = COALESCE(?9, orientation), flash = ?10, \
             metering_mode = ?11, white_balance = ?12, exposure_program = ?13, \
             software = ?14, artist = ?15, gps_lat = ?16, gps_lon = ?17 \
             WHERE id = ?1",
            params![
                id,
                meta.lens,
                meta.width,
                meta.height,
                meta.iso,
                meta.f_number,
                meta.exposure_time,
                meta.focal_length,
                deep.orientation,
                deep.flash,
                deep.metering_mode,
                deep.white_balance,
                deep.exposure_program,
                deep.software,
                deep.artist,
                deep.gps_lat,
                deep.gps_lon,
            ],
        )?;
        Ok(())
    }

    /// EXIF 提取代际升级自愈（exif-gen-2）：重排**全部** photo/raw 资产的
    /// exif 任务（既有任务复位 pending + 为无任务行新建），返回待处理数。
    /// 唯一索引（0007）保证 INSERT OR IGNORE 不重复。
    pub fn requeue_exif_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1 \
             WHERE kind = 'exif' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'exif', a.id, 'pending', 0, ?1, ?1 FROM assets a \
             WHERE a.kind IN ('photo', 'raw') \
               AND NOT EXISTS (SELECT 1 FROM index_tasks t \
                               WHERE t.kind = 'exif' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("exif")
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

fn map_asset_page(row: &Row<'_>) -> Result<AssetPageRow> {
    Ok(AssetPageRow {
        id: row.get(0)?,
        path: row.get(1)?,
        filename: row.get(2)?,
        size: row.get::<_, i64>(3)? as u64,
        kind: row.get(4)?,
        captured_at: row.get(5)?,
        camera: row.get(6)?,
        width: row.get::<_, Option<i64>>(7)?.map(|v| v as u32),
        height: row.get::<_, Option<i64>>(8)?.map(|v| v as u32),
        iso: row.get::<_, Option<i64>>(9)?.map(|v| v as u32),
        f_number: row.get(10)?,
        exposure_time: row.get(11)?,
        focal_length: row.get(12)?,
        lens: row.get(13)?,
        pair_id: row.get(14)?,
        thumb_state: row.get::<_, Option<i64>>(15)?.unwrap_or(0) as i32,
    })
}

fn map_asset_full(row: &Row<'_>) -> Result<AssetRow> {
    Ok(AssetRow {
        path: row.get(0)?,
        filename: row.get(1)?,
        size: row.get::<_, i64>(2)? as u64,
        mtime: row.get(3)?,
        xxhash: row.get::<_, i64>(4)? as u64,
        sha256: blob_to_sha256(Some(row.get(5)?))?.unwrap_or([0; 32]),
        kind: row.get(6)?,
        captured_at: row.get(7)?,
        camera: row.get(8)?,
        source: row.get(9)?,
        created_at: row.get(10)?,
        origin: row.get(11)?,
        width: row.get::<_, Option<i64>>(12)?.map(|v| v as u32),
        height: row.get::<_, Option<i64>>(13)?.map(|v| v as u32),
        iso: row.get::<_, Option<i64>>(14)?.map(|v| v as u32),
        f_number: row.get(15)?,
        exposure_time: row.get(16)?,
        focal_length: row.get(17)?,
        lens: row.get(18)?,
        pair_asset_id: row.get(19)?,
        thumb_state: row.get::<_, Option<i64>>(20)?.unwrap_or(0) as i32,
        orientation: row.get(21)?,
        flash: row.get(22)?,
        metering_mode: row.get(23)?,
        white_balance: row.get(24)?,
        exposure_program: row.get(25)?,
        software: row.get(26)?,
        artist: row.get(27)?,
        gps_lat: row.get(28)?,
        gps_lon: row.get(29)?,
    })
}

fn map_index_task(row: &Row<'_>) -> Result<IndexTaskRow> {
    Ok(IndexTaskRow {
        id: row.get(0)?,
        kind: row.get(1)?,
        asset_id: row.get(2)?,
        state: row.get(3)?,
        attempts: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

/// 路径 → (目录含尾分隔符, stem, 扩展名含点)；无扩展名 stem = 文件名。
fn split_dir_stem_ext(path: &str) -> (&str, &str, &str) {
    let after_dir = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let dir = &path[..path.len() - after_dir.len()];
    match after_dir.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (dir, stem, ext),
        _ => (dir, after_dir, ""),
    }
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
