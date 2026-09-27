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
use rusqlite::types::{FromSql, FromSqlError, FromSqlResult, ToSqlOutput, ValueRef};
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
    /// 评分 0-5（0009，NOT NULL DEFAULT 0；0 = 未评）。
    #[serde(default)]
    pub rating: i64,
    /// 收藏旗标（0009，布尔语义 0/1）。
    #[serde(default)]
    pub flagged: i64,
    /// 颜色标签（0016，LR 标准色名小写 token；NULL = 无标签）。
    #[serde(default)]
    pub color_label: Option<String>,
    /// 接受/拒绝状态（0016，布尔语义 0/1；与星级分层的应用内选片状态）。
    #[serde(default)]
    pub rejected: i64,
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

/// 那年今天的筛选口径（列表与侧栏计数共用，防两处漂移）：本地时区
/// 同月日的 photo/raw（captured_at 为 RFC3339；'localtime' 把 UTC 存储
/// 转本地后取 %m-%d）。
const ON_THIS_DAY_WHERE: &str = "kind IN ('photo', 'raw') AND captured_at IS NOT NULL \
     AND in_trash = 0 \
     AND strftime('%m-%d', captured_at, 'localtime') = ?1 \
     AND strftime('%Y', captured_at, 'localtime') < strftime('%Y', 'now', 'localtime')";

/// 资产分页过滤（IPC 载荷，camelCase）。日期为 RFC3339 字符串或纯日期
/// `YYYY-MM-DD`（IPC 层归一定宽 UTC：纯日期 after=当日 00:00、before=当日
/// 23:59:59.999 本地时区），字典序比较即时间序；任一日期过滤出现时
/// NULL captured_at 的行被排除（无日期不落任何区间）。`kinds` 多选（SQL IN，
/// 空 = 不过滤；用户分类语义「照片」=photo+raw 由前端传 [photo,raw]）；
/// `cameras` 多选 OR（搜索页相机勾选）。///
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
    /// 评分范围 0-5（闭区间；rating NOT NULL DEFAULT 0，无 NULL 态）。
    pub rating_min: Option<i64>,
    pub rating_max: Option<i64>,
    /// 收藏旗标：true → flagged=1；false → flagged=0。
    pub flagged: Option<bool>,
    /// 收藏页组合条件：true → rating > 0 OR flagged = 1（评分或旗标任一）；
    /// false → rating = 0 AND flagged = 0。
    pub favorite: Option<bool>,
    /// 相册维度（0015）：Some(id) → 仅「在 id 相册中」的资产（EXISTS
    /// album_item 求交；相册不存在命中空集）。全局筛选「在某相册」用；
    /// 相册内时间线（album_assets_page）也复用此条件与 keyset 机制。
    pub album_id: Option<i64>,
    /// 颜色标签（0016）：精确匹配小写 token（red/yellow/green/blue/purple）。
    pub color_label: Option<String>,
    /// 接受/拒绝状态（0016）：true → rejected=1；false → rejected=0。
    /// 默认查询**不排除**已拒绝——只是可筛选项，区别于回收站。
    pub rejected: Option<bool>,
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
    /// 连拍组 id（M6；未入组 None）。
    pub burst_id: Option<i64>,
    pub flagged: bool,
    pub rating: i64,
    /// 颜色标签（0016；无标签 None）。
    pub color_label: Option<String>,
    /// 接受/拒绝状态（0016；布尔语义）。
    pub rejected: bool,
}

/// 连拍扫描行（分组引擎输入：id/phash/captured_at/kind/pair）。
pub type BurstScanRow = (i64, i64, Option<String>, String, Option<i64>);

/// 桶成员表条目（多探针候选装配用）。
pub type SimilarBucketMembers = ((i64, i64), Vec<i64>);

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

/// 相册行（album_list 数据源 / IPC 载荷 AlbumDto，camelCase）：纯引用照片组
/// 的元数据面——name 全库唯一；cover_asset_id 只是封面引用（资产永久删除时
/// FK SET NULL 自动解除）；item_count 为引用数（相册为空合法）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRow {
    pub id: i64,
    pub name: String,
    pub cover_asset_id: Option<i64>,
    pub item_count: u64,
    pub created_at: String,
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

/// 智能视图行（0016 smart_view 表；IPC 载荷 SmartViewDto 同构 camelCase）。
/// filters_json 为前端 AssetFilters 序列化——后端不解释只存取。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartViewRow {
    pub id: i64,
    pub name: String,
    pub filters_json: String,
    pub created_at: String,
}

// ---------------------------------------------------------------------------
// 仓储方法
// ---------------------------------------------------------------------------

/// 分页行投影列（[`map_asset_page`] 消费顺序；各查询共用，防列序漂移）。
const ASSET_PAGE_COLS: &str = "id, path, filename, size, kind, captured_at, camera, \
     width, height, iso, f_number, exposure_time, focal_length, lens, pair_asset_id, \
     thumb_state, burst_id, flagged, rating, color_label, rejected";
/// 同 [`ASSET_PAGE_COLS`] 的 `a.` 别名前缀形态（内层子查询用）。
const ASSET_PAGE_COLS_A: &str = "a.id, a.path, a.filename, a.size, a.kind, a.captured_at, \
     a.camera, a.width, a.height, a.iso, a.f_number, a.exposure_time, a.focal_length, a.lens, \
     a.pair_asset_id, a.thumb_state, a.burst_id, a.flagged, a.rating, a.color_label, a.rejected";

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
        self.0.execute(
            "INSERT INTO job_files (job_id, src, dst, size, state, error, xxhash, dst2) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
             ON CONFLICT (job_id, src) DO UPDATE SET \
             dst = excluded.dst, size = excluded.size, state = excluded.state, \
             error = excluded.error, xxhash = excluded.xxhash, \
             dst2 = excluded.dst2",
            params![
                row.job_id,
                row.src,
                row.dst,
                row.size as i64,
                row.state,
                row.error,
                row.xxhash.map(|v| v as i64),
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
            "SELECT job_id, src, dst, size, state, error, xxhash, dst2 FROM job_files \
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
            "SELECT job_id, src, dst, size, state, error, xxhash, dst2 FROM job_files \
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
        let mut params_vec: Vec<V> = Vec::new();
        // 统一槽位构造：?N 显式编号 + params_vec 同序 push（按编号绑定）
        let slot = |params_vec: &mut Vec<V>, v: V| -> String {
            let s = format!("?{}", params_vec.len() + 1);
            params_vec.push(v);
            s
        };

        // —— 排序哨兵（子查询内 COALESCE 的 NULL 归一）——
        let sentinel_slot = slot(&mut params_vec, V::from(CAPTURED_NULL_HIGH.to_string()));

        let conds = asset_filter_conditions(filters, &mut params_vec);

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
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT {ASSET_PAGE_COLS_A}, \
              COALESCE(a.captured_at, {sentinel_slot}) AS k \
              FROM assets a WHERE {all}) \
             WHERE ({cursor_id_slot} = 0 OR k < {cursor_key_slot} \
                    OR (k = {cursor_key_slot} AND id < {cursor_id_slot})) \
             ORDER BY k DESC, id DESC LIMIT {limit_slot}",
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params_vec), map_asset_page)?;
        rows.collect()
    }

    /// 当前筛选条件的真实总数，与 assets_page 复用完全相同的 SQL 条件。
    pub fn assets_count(&self, filters: &AssetFilters) -> Result<u64> {
        use rusqlite::types::Value as V;
        let mut params_vec: Vec<V> = Vec::new();
        let conditions = asset_filter_conditions(filters, &mut params_vec);
        let where_sql = if conditions.is_empty() {
            "1 = 1".to_string()
        } else {
            conditions.join(" AND ")
        };
        let count: i64 = self.0.query_row(
            &format!("SELECT COUNT(*) FROM assets a WHERE {where_sql}"),
            rusqlite::params_from_iter(params_vec),
            |row| row.get(0),
        )?;
        Ok(count as u64)
    }

    /// 最近添加分页（「最近添加」页数据源）：created_at DESC、id DESC
    /// keyset——created_at NOT NULL 定宽 RFC3339，字典序即时间序；游标
    /// after_id 为上一页末行 id（0 = 第一页；行已删按第一页）。回收站资产
    /// 不出现（in_trash=0）。
    pub fn recent_assets_page(&self, after_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.0.query_row(
                "SELECT created_at FROM assets WHERE id = ?1",
                [after_id],
                |r| r.get::<_, String>(0),
            ) {
                Ok(key) => (key, after_id),
                Err(_) => ("9999-12-31T23:59:59.999Z".to_string(), 0), // 行已删：回退第一页
            }
        } else {
            ("9999-12-31T23:59:59.999Z".to_string(), 0)
        };
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT {ASSET_PAGE_COLS_A}, a.created_at AS ck FROM assets a \
              WHERE a.in_trash = 0) \
             WHERE (?1 = 0 OR ck < ?2 OR (ck = ?2 AND id < ?1)) \
             ORDER BY ck DESC, id DESC LIMIT ?3",
        ))?;
        let rows = stmt.query_map(params![cursor_id, cursor_key, limit], map_asset_page)?;
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
    /// （captured DESC、id DESC）。回收站资产不计（in_trash=0）。
    pub fn asset_group_dates(&self) -> Result<Vec<DateGroupRow>> {
        let mut stmt = self.0.prepare(
            "SELECT day, COUNT(*), \
               (SELECT t.id FROM assets t \
                 WHERE COALESCE(date(t.captured_at, 'localtime'), 'unknown') = day \
                   AND t.in_trash = 0 \
                 ORDER BY COALESCE(t.captured_at, ?1) DESC, t.id DESC LIMIT 1) \
             FROM (SELECT COALESCE(date(captured_at, 'localtime'), 'unknown') AS day \
                   FROM assets WHERE in_trash = 0) \
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
            "SELECT path, filename, size, mtime, xxhash, kind, captured_at, camera, \
             source, created_at, origin, width, height, iso, f_number, exposure_time, \
             focal_length, lens, pair_asset_id, thumb_state, orientation, flash, \
             metering_mode, white_balance, exposure_program, software, artist, \
             gps_lat, gps_lon, rating, flagged, color_label, rejected FROM assets WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(map_asset_full(row)?)),
            None => Ok(None),
        }
    }

    /// 库内同指纹 (size, xxhash) 的**其他**资产数（不含自身）。回收站资产
    /// 不计（详情页重复计数与画廊一致口径）。
    pub fn asset_duplicate_count(&self, id: i64, size: u64, xxhash: u64) -> Result<u64> {
        let mut stmt = self.0.prepare(
            "SELECT COUNT(*) FROM assets WHERE size = ?1 AND xxhash = ?2 AND id != ?3 \
             AND in_trash = 0",
        )?;
        let count: i64 = stmt.query_row(params![size as i64, xxhash as i64, id], |r| r.get(0))?;
        Ok(count as u64)
    }

    /// 资产入库：同路径重复导入整行覆盖（REPLACE 换 id 时自动重指
    /// pair_asset_id 既有引用），随后按 (同目录, 同 stem, 异扩展名) 双向
    /// 写 RAW/JPG 配对（pair_asset_id）。
    /// （引擎走 [`Db::insert_asset_with_album`]；本方法保留为仓储基元，
    /// 测试覆盖——与 create_job 同款约定。）
    #[allow(dead_code)]
    pub fn insert_asset(&self, a: &AssetRow) -> Result<()> {
        insert_asset_on(&self.0, a, None)
    }

    /// 资产入库 + 同事务挂相册（导入引擎的 album_id 通道，0015）：资产行、
    /// 配对、索引待办与 album_item 引用原子落库——中断恢复时要么资产与
    /// 引用都在、要么都不在，INSERT OR IGNORE 保证 resume 重放幂等。
    /// 相册在导入期间被删除（用户侧并发操作）时**跳过挂载不报错**：
    /// 文件已安全复制落盘，不因相册消失判整个文件失败（journal 不留假失败）。
    pub fn insert_asset_with_album(&self, a: &AssetRow, album_id: Option<i64>) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        insert_asset_on(&tx, a, album_id)?;
        tx.commit()
    }

    /// M8 视频海报解锁：历史库的 video 资产曾被置 thumb_state=2（当时无
    /// 海报路径的永久占位）。海报管线就位后复位为 0——按需队列下次请求
    /// 即补生成；真失败仍由队列自身失败计数兜底。幂等（无 marker：每次
    /// 启动重置一次无副作用，反而是侧车补装后的自愈通道）。
    pub fn reset_video_thumb_placeholders(&self) -> Result<u64> {
        let n = self.0.execute(
            "UPDATE assets SET thumb_state = 0 WHERE kind = 'video' AND thumb_state = 2",
            [],
        )?;
        Ok(n as u64)
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

    /// 批量认领（2026-09-21 语义推理批量化）：一次认领至多 `limit` 条
    /// pending（id 升序）原子置 running——回填 worker 组批单次
    /// session.run 的前提。不足 limit 照常小批，空批返回空 Vec。
    pub fn claim_index_tasks(&self, kind: &str, limit: usize) -> Result<Vec<IndexTaskRow>> {
        let now = now_rfc3339();
        let mut stmt = self.0.prepare(
            "UPDATE index_tasks SET state = 'running', updated_at = ?2 \
             WHERE id IN (SELECT id FROM index_tasks WHERE state = 'pending' \
                          AND kind = ?1 ORDER BY id LIMIT ?3) \
             RETURNING id, kind, asset_id, state, attempts, created_at, updated_at",
        )?;
        let rows = stmt.query_map(params![kind, now, limit as i64], map_index_task)?;
        rows.collect()
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

    /// 语义检索 join：资产是否存在且可检索（photo/raw）。回收站资产不可检索
    /// （0016：常规链路默认排除，恢复后自动回到检索结果）。
    pub fn asset_searchable(&self, id: i64) -> Result<bool> {
        let ok: i64 = self.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE id = ?1 AND kind IN ('photo', 'raw') \
             AND in_trash = 0",
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

    /// 重建用：重排**全部** photo/raw 的 thumb 任务（既有复位 pending +
    /// 无任务行新建；配合 thumb_state=0 复位 + 缩略图目录删除）。
    pub fn requeue_thumb_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'thumb' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'thumb', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'thumb' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("thumb")
    }

    /// 重建用：EXIF 深提取列全清（0004 拍摄参数 + 0008 深字段全 NULL，
    /// captured_at/camera 保留——目录结构/时间线依赖它们），配合 exif 任务
    /// 重排由 worker 重新提取。
    pub fn clear_exif_columns(&self) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET                  width = NULL, height = NULL, iso = NULL, f_number = NULL,                  exposure_time = NULL, focal_length = NULL, lens = NULL,                  orientation = NULL, flash = NULL, metering_mode = NULL,                  white_balance = NULL, exposure_program = NULL, software = NULL,                  artist = NULL, gps_lat = NULL, gps_lon = NULL",
            [],
        )?;
        Ok(())
    }

    /// 重建用：缩略图状态镜像全复位（配合 thumbs 目录删除）。
    pub fn reset_thumb_states(&self) -> Result<()> {
        self.0.execute("UPDATE assets SET thumb_state = 0", [])?;
        Ok(())
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
                 AND f3.asset_id IN (SELECT id FROM assets WHERE in_trash = 0) \
                 ORDER BY f3.box_w * f3.box_h DESC, f3.id ASC LIMIT 1) \
             FROM people p JOIN faces f ON f.cluster_id = p.id \
             JOIN assets a ON a.id = f.asset_id AND a.in_trash = 0 \
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
    /// captured_at 最先——与画廊排序契约一致）。回收站资产不出现。
    pub fn assets_by_cluster(&self, cluster_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM \
             (SELECT DISTINCT {ASSET_PAGE_COLS_A}, \
                    COALESCE(a.captured_at, ?2) AS k \
                   FROM assets a JOIN faces f ON f.asset_id = a.id \
                   WHERE f.cluster_id = ?1 AND a.in_trash = 0) \
             ORDER BY k DESC, id DESC LIMIT ?3",
        ))?;
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

    // —— 相册（M9，album + album_item 表：纯引用照片组）——

    /// 相册列表（item_count 子查询免 GROUP BY 空相册丢行；createdAt DESC、
    /// id DESC tiebreak——空相册同样返回且 itemCount=0）。
    pub fn album_list(&self) -> Result<Vec<AlbumRow>> {
        let mut stmt = self.0.prepare(
            "SELECT a.id, a.name, a.cover_asset_id, \
                    (SELECT COUNT(*) FROM album_item i WHERE i.album_id = a.id), a.created_at \
             FROM album a ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let rows = stmt.query_map([], map_album)?;
        rows.collect()
    }

    /// 资产 → 所属相册反查（查看器详情「所属相册」行）。按相册创建时间 DESC；
    /// 未入任何相册返回空。
    pub fn asset_albums(&self, asset_id: i64) -> Result<Vec<AlbumRow>> {
        let mut stmt = self.0.prepare(
            "SELECT a.id, a.name, a.cover_asset_id, \
                    (SELECT COUNT(*) FROM album_item i2 WHERE i2.album_id = a.id), a.created_at \
             FROM album a \
             WHERE EXISTS (SELECT 1 FROM album_item i WHERE i.album_id = a.id AND i.asset_id = ?1) \
             ORDER BY a.created_at DESC, a.id DESC",
        )?;
        let rows = stmt.query_map(params![asset_id], map_album)?;
        rows.collect()
    }

    /// 建相册，返回新行（item_count=0、cover=None）。重名由 name UNIQUE
    /// 兜底（错误透传，IPC 层转友好文案）；名称 trim/空校验在 IPC 层。
    pub fn album_create(&self, name: &str) -> Result<AlbumRow> {
        let created_at = now_rfc3339();
        self.0.execute(
            "INSERT INTO album (name, cover_asset_id, created_at) VALUES (?1, NULL, ?2)",
            params![name, created_at],
        )?;
        Ok(AlbumRow {
            id: self.0.last_insert_rowid(),
            name: name.to_string(),
            cover_asset_id: None,
            item_count: 0,
            created_at,
        })
    }

    /// 相册重命名；相册不存在报错（同 rename_person 语义）。
    /// 重名由 UNIQUE 兜底透传。
    pub fn album_rename(&self, id: i64, name: &str) -> Result<()> {
        let n = self.0.execute(
            "UPDATE album SET name = ?2 WHERE id = ?1",
            params![id, name],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 删相册：只删相册行——album_item 引用经 FK ON DELETE CASCADE 级联
    /// 消失，资产与物理文件绝不动；相册不存在报错（幂等删除由 IPC 语义定）。
    pub fn album_delete(&self, id: i64) -> Result<()> {
        let n = self
            .0
            .execute("DELETE FROM album WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 设/清相册封面（纯引用；asset_id 为 None 清回默认封面）。
    /// 相册不存在报错；asset 不存在触发 FK 约束错误透传（IPC 层转文案）。
    pub fn album_cover_set(&self, id: i64, asset_id: Option<i64>) -> Result<()> {
        let n = self.0.execute(
            "UPDATE album SET cover_asset_id = ?2 WHERE id = ?1",
            params![id, asset_id],
        )?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
        Ok(())
    }

    /// 批量入册引用：`INSERT OR IGNORE` 对 PK(album_id, asset_id) 幂等，
    /// 返回**实际新增**数；不存在的资产 id 静默跳过（相册页列表来自
    /// 实时库，恰好在他处被永久删除的 id 属预期陈旧值）；相册不存在报错。
    pub fn album_add_assets(&self, album_id: i64, asset_ids: &[i64]) -> Result<u64> {
        if !self.album_exists(album_id)? {
            return Err(Error::QueryReturnedNoRows);
        }
        let added_at = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        let mut added = 0u64;
        for asset_id in asset_ids {
            // INSERT..SELECT：资产不存在 → 0 行（跳过），不触发 FK 错误
            added += tx.execute(
                "INSERT OR IGNORE INTO album_item (album_id, asset_id, added_at) \
                     SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM assets WHERE id = ?2)",
                params![album_id, asset_id, added_at],
            )? as u64;
        }
        tx.commit()?;
        Ok(added)
    }

    /// 批量移除引用（幂等：不在册的 id 删 0 行）；相册不存在报错。
    /// 只删 album_item 行，绝不动资产行/物理文件。
    pub fn album_remove_assets(&self, album_id: i64, asset_ids: &[i64]) -> Result<()> {
        if !self.album_exists(album_id)? {
            return Err(Error::QueryReturnedNoRows);
        }
        if asset_ids.is_empty() {
            return Ok(());
        }
        let slots = (0..asset_ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM album_item WHERE album_id = ?1 AND asset_id IN ({slots})");
        self.0.execute(
            &sql,
            rusqlite::params_from_iter(std::iter::once(album_id).chain(asset_ids.iter().copied())),
        )?;
        Ok(())
    }

    /// 相册是否存在（IPC 存在性校验 / add-remove 前置）。
    pub fn album_exists(&self, id: i64) -> Result<bool> {
        let exists: i64 = self.0.query_row(
            "SELECT EXISTS(SELECT 1 FROM album WHERE id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        Ok(exists != 0)
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
             WHERE camera IS NOT NULL AND camera != '' AND in_trash = 0 \
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
             WHERE lens IS NOT NULL AND lens != '' AND in_trash = 0 \
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
             WHERE path LIKE '%.%' AND in_trash = 0 \
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

    /// 评分写入（0-5 由 IPC 层校验；资产不存在返回 false）。
    /// 只动 DB——XMP 边车同步由调用方（ipc::rating）异步派发。
    pub fn set_asset_rating(&self, id: i64, rating: i64) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE assets SET rating = ?2 WHERE id = ?1",
            params![id, rating],
        )?;
        Ok(n > 0)
    }

    /// 当前评分（资产不存在 None；exif 通道 XMP 回填判定用）。
    pub fn asset_rating_of(&self, id: i64) -> Result<Option<i64>> {
        let mut stmt = self.0.prepare("SELECT rating FROM assets WHERE id = ?1")?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(row.get(0)?)),
            None => Ok(None),
        }
    }

    /// 浏览记账（upsert：每资产一行，浏览即刷新 viewed_at）。
    /// 资产不存在静默（调用方契约：mark 对无效 id 不报错）。
    pub fn mark_asset_viewed(&self, asset_id: i64) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO view_history (asset_id, viewed_at) VALUES (?1, ?2)              ON CONFLICT (asset_id) DO UPDATE SET viewed_at = excluded.viewed_at",
            params![asset_id, now_rfc3339()],
        )?;
        tx.execute(
            "DELETE FROM view_history WHERE asset_id NOT IN (SELECT asset_id FROM view_history ORDER BY viewed_at DESC, asset_id DESC LIMIT 200)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 最近浏览资产（viewed_at DESC；复用画廊分页行结构）。回收站资产不出现。
    pub fn recently_viewed(&self, limit: u32) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM view_history v \
             JOIN assets a ON a.id = v.asset_id \
             WHERE a.in_trash = 0 \
             ORDER BY v.viewed_at DESC, v.asset_id DESC LIMIT ?1",
        ))?;
        let rows = stmt.query_map(params![limit], map_asset_page)?;
        rows.collect()
    }

    /// 连拍扫描行：(id, phash, captured_at, kind, pair_asset_id)——
    /// 已算 pHash 的 photo/raw 按 (camera, captured_at, id) 升序。
    pub fn burst_scan_rows(&self) -> Result<Vec<BurstScanRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, phash, captured_at, kind, pair_asset_id FROM assets              WHERE phash IS NOT NULL AND kind IN ('photo', 'raw')              ORDER BY camera ASC, captured_at ASC, id ASC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect()
    }

    /// 连拍组整体重写（事务：清旧组 + burst_id 复位 → 建组 + 回填归属）。
    /// members = 每组 (asset_id, captured_at) 列表（组内有序）。
    pub fn write_bursts(&self, groups: &[Vec<(i64, Option<String>)>]) -> Result<()> {
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            "UPDATE assets SET burst_id = NULL WHERE burst_id IS NOT NULL",
            [],
        )?;
        tx.execute("DELETE FROM bursts", [])?;
        for members in groups {
            let (started, ended) = (
                members.first().and_then(|m| m.1.clone()),
                members.last().and_then(|m| m.1.clone()),
            );
            tx.execute(
                "INSERT INTO bursts (asset_count, started_at, ended_at) VALUES (?1, ?2, ?3)",
                params![members.len() as i64, started, ended],
            )?;
            let burst_id = tx.last_insert_rowid();
            for (asset_id, _) in members {
                tx.execute(
                    "UPDATE assets SET burst_id = ?2 WHERE id = ?1",
                    params![asset_id, burst_id],
                )?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 连拍统计（设置页/调试）：(组数, 入组资产数)。
    pub fn burst_stats(&self) -> Result<(u64, u64)> {
        let (groups, photos): (i64, i64) = self.0.query_row(
            "SELECT COUNT(*), COALESCE(SUM(asset_count), 0) FROM bursts",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        Ok((groups as u64, photos as u64))
    }

    /// 批量取组员数（页内 burstId → burstCount 装配，免 N+1）。
    pub fn burst_counts(&self, burst_ids: &[i64]) -> Result<HashMap<i64, u32>> {
        let mut out = HashMap::new();
        let ids: Vec<i64> = {
            let mut seen = std::collections::HashSet::new();
            burst_ids
                .iter()
                .copied()
                .filter(|id| seen.insert(*id))
                .collect()
        };
        if ids.is_empty() {
            return Ok(out);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT b.id, b.asset_count FROM bursts b WHERE b.id IN ({slots})");
        let mut stmt = self.0.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u32))
        })?;
        for row in rows {
            let (id, count) = row?;
            out.insert(id, count);
        }
        Ok(out)
    }

    /// pHash 写入（worker 成果）。
    pub fn set_phash(&self, id: i64, phash: u64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET phash = ?2 WHERE id = ?1",
            params![id, phash as i64],
        )?;
        Ok(())
    }

    /// 为 pHash 回填建任务：phash IS NULL 的 photo/raw 且无未完成 phash 任务。
    /// RAW+JPG 孪生两边都算（分组时才跳孪生；查重也要用）。
    pub fn create_phash_tasks_for_unindexed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'phash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.phash IS NULL AND a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'phash' AND t.asset_id = a.id                                AND t.state IN ('pending', 'running'))",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// phash 通道代际重排（既有复位 pending + 无任务行新建）。
    pub fn requeue_phash_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'phash' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'phash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.kind IN ('photo', 'raw')                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'phash' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("phash")
    }

    /// xxhash 补算写入（hash worker 成果）。
    pub fn set_xxhash(&self, id: i64, xxhash: u64) -> Result<()> {
        self.0.execute(
            "UPDATE assets SET xxhash = ?2 WHERE id = ?1",
            params![id, xxhash as i64],
        )?;
        Ok(())
    }

    /// 为哈希补算建任务：xxhash = 0 哨兵（rename 快道遗留）且无未完成
    /// hash 任务的资产（全 kind——视频等同样参与查重）。
    pub fn create_hash_tasks_for_unhashed(&self) -> Result<u64> {
        let now = now_rfc3339();
        let created = self.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'hash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.xxhash = 0                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'hash' AND t.asset_id = a.id                                AND t.state IN ('pending', 'running'))",
            params![now],
        )?;
        Ok(created as u64)
    }

    /// hash 通道代际重排（既有复位 + 无任务行新建；对齐 requeue_* 家族）。
    pub fn requeue_hash_tasks_for_all(&self) -> Result<u64> {
        let now = now_rfc3339();
        self.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?1              WHERE kind = 'hash' AND state != 'pending'",
            params![now],
        )?;
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              SELECT 'hash', a.id, 'pending', 0, ?1, ?1 FROM assets a              WHERE a.xxhash = 0                AND NOT EXISTS (SELECT 1 FROM index_tasks t                                WHERE t.kind = 'hash' AND t.asset_id = a.id)",
            params![now],
        )?;
        self.pending_index_task_count("hash")
    }

    /// 单资产 hash 任务登记（rename 快道入库后即时建；幂等）。
    pub fn create_hash_task_for(&self, asset_id: i64) -> Result<()> {
        self.0.execute(
            "INSERT OR IGNORE INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at)              VALUES ('hash', ?1, 'pending', 0, ?2, ?2)",
            params![asset_id, now_rfc3339()],
        )?;
        Ok(())
    }

    /// pHash 指纹的 4 段 16-bit 分桶插入（phash 任务成功后增量维护）。
    pub fn insert_similar_buckets(&self, id: i64, phash: u64) -> Result<()> {
        let p = phash as i64;
        self.0.execute(
            "INSERT OR IGNORE INTO similar_bucket (segment, seg_val, asset_id) VALUES \
             (0, (?1 >> 48) & 0xFFFF, ?5), (1, (?1 >> 32) & 0xFFFF, ?5), \
             (2, (?1 >> 16) & 0xFFFF, ?5), (3, ?1 & 0xFFFF, ?5)",
            params![p, p, p, p, id],
        )?;
        Ok(())
    }

    /// 桶表懒校验+全量重建（行数 ≠ 4×phash 行数时；纯 SQL，20 万行秒级）。
    /// 返回是否执行了重建。
    pub fn ensure_similar_buckets(&self) -> Result<bool, String> {
        let (phash_rows, bucket_rows): (i64, i64) = self
            .0
            .query_row(
                "SELECT (SELECT COUNT(*) FROM assets WHERE phash IS NOT NULL), \
                 (SELECT COUNT(*) FROM similar_bucket)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(|e| e.to_string())?;
        if bucket_rows == phash_rows * 4 {
            return Ok(false);
        }
        self.0
            .execute_batch(
                "DELETE FROM similar_bucket; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 0, (phash >> 48) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 1, (phash >> 32) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 2, (phash >> 16) & 0xFFFF, id FROM assets WHERE phash IS NOT NULL; \
                 INSERT INTO similar_bucket (segment, seg_val, asset_id) \
                 SELECT 3, phash & 0xFFFF, id FROM assets WHERE phash IS NOT NULL;",
            )
            .map_err(|e| e.to_string())?;
        Ok(true)
    }

    /// 近重复扫描行：(asset_id, phash, pair_asset_id)。**RAW 孪生整行排除**
    /// （pair 非空且自己是 RAW——同拍摄不算重复；只排除边不够，孪生会经
    /// 第三成员间接入组）。
    pub fn similar_scan_rows(&self) -> Result<Vec<(i64, i64, Option<i64>)>> {
        let mut stmt = self.0.prepare(
            "SELECT id, phash, pair_asset_id FROM assets \
             WHERE phash IS NOT NULL AND kind IN ('photo', 'raw') AND in_trash = 0 \
               AND NOT (kind = 'raw' AND pair_asset_id IS NOT NULL) ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect()
    }

    /// 多探针桶成员表：(segment, seg_val) → 资产 id 列表（仅 ≥2 成员的桶）。
    pub fn similar_bucket_members(&self) -> Result<Vec<SimilarBucketMembers>> {
        let mut stmt = self.0.prepare(
            "SELECT segment, seg_val, asset_id FROM similar_bucket \
             ORDER BY segment, seg_val, asset_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?;
        let mut out: Vec<((i64, i64), Vec<i64>)> = Vec::new();
        for row in rows {
            let (seg, val, id) = row?;
            match out.last_mut() {
                Some((key, members)) if *key == (seg, val) => members.push(id),
                _ => out.push(((seg, val), vec![id])),
            }
        }
        out.retain(|(_, members)| members.len() >= 2);
        Ok(out)
    }

    /// 完全重复组：(size, xxhash) 分组计数 >1 的指纹键（count 降序）。
    pub fn exact_duplicate_keys(&self) -> Result<Vec<(i64, u64)>> {
        let mut stmt = self.0.prepare(
            "SELECT size, xxhash, COUNT(*) AS c FROM assets \
             WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
             GROUP BY size, xxhash HAVING c > 1 ORDER BY c DESC, size, xxhash",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)? as u64))
        })?;
        rows.collect()
    }

    /// 按多个 id 批量取分页行（组内按 created_at 升序；分组装配由调用方做）。
    pub fn assets_by_ids_ordered(&self, ids: &[i64]) -> Result<Vec<AssetPageRow>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets WHERE id IN ({slots}) ORDER BY created_at ASC, id ASC"
        );
        let mut stmt = self.0.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), map_asset_page)?;
        rows.collect()
    }

    /// 那年今天：本地时区同月日的 photo/raw，年份 DESC、年内时间 ASC。
    /// WHERE 片段抽成常量——列表与 sidebar 计数两处共用，杜绝口径漂移。
    pub fn assets_on_this_day(&self, month_day: &str) -> Result<Vec<AssetPageRow>> {
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets \
             WHERE {ON_THIS_DAY_WHERE} \
             ORDER BY substr(captured_at, 1, 4) DESC, captured_at ASC, id ASC",
        ))?;
        let rows = stmt.query_map(params![month_day], map_asset_page)?;
        rows.collect()
    }

    /// 那年今天的计数口径（列表 assets_on_this_day 与侧栏计数共用；
    /// month_day 为本地时区 "%m-%d"）。
    pub fn count_on_this_day(&self, month_day: &str) -> Result<i64> {
        self.0.query_row(
            &format!("SELECT COUNT(*) FROM assets WHERE {ON_THIS_DAY_WHERE}"),
            params![month_day],
            |r| r.get(0),
        )
    }

    /// 侧栏计数（2026-09-21）：库内资产总数（全 kind——侧栏「照片」即画廊
    /// 全量；0016 起排除回收站）与浏览历史行数。纯 COUNT，毫秒级。
    pub fn sidebar_assets_count(&self) -> Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM assets WHERE in_trash = 0", [], |r| {
                r.get(0)
            })
    }

    pub fn sidebar_viewed_count(&self) -> Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM view_history", [], |r| r.get(0))
    }

    /// 相册数（侧栏计数 0015 起：真实 COUNT(album)，不再是标签墙常量）。
    pub fn sidebar_albums_count(&self) -> Result<i64> {
        self.0
            .query_row("SELECT COUNT(*) FROM album", [], |r| r.get(0))
    }

    /// 器材统计桶计数（单遍 SQL：子查询把 exposure_time 展示串解析成秒，
    /// 外层 CASE 分桶；GLOB 守卫挡住非数值串——CAST('垃圾' AS REAL)=0 会
    /// 污染快桶）。快门桶序：>1s / 1-1/2 / 1/2-1/8 / 1/8-1/60 / 1/60-1/500 /
    /// ≤1/500（秒）；光圈序：≤1.4/1.4-2.8/2.8-4/4-5.6/5.6-8/>8；焦段序：
    /// <24/24-50/50-85/85-135/135-200/≥200；ISO 序：≤100/…/1600-3200/>3200。
    pub fn gear_bucket_counts(&self) -> Result<Vec<i64>, String> {
        let mut stmt = self
            .0
            .prepare(
                "SELECT \
                    SUM(CASE WHEN f < 24 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 24 AND f < 50 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 50 AND f < 85 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 85 AND f < 135 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 135 AND f < 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN f >= 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso <= 100 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 100 AND iso <= 200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 200 AND iso <= 400 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 400 AND iso <= 800 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 800 AND iso <= 1600 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 1600 AND iso <= 3200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN iso > 3200 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap <= 1.4 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 1.4 AND ap <= 2.8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 2.8 AND ap <= 4 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 4 AND ap <= 5.6 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 5.6 AND ap <= 8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN ap > 8 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec > 1.0 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec <= 1.0 AND sec >= 0.5 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 0.5 AND sec >= 0.125 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 0.125 AND sec >= 1.0/60 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec < 1.0/60 AND sec > 1.0/500 THEN 1 ELSE 0 END), \
                    SUM(CASE WHEN sec <= 1.0/500 THEN 1 ELSE 0 END) \
                 FROM ( \
                    SELECT CASE WHEN focal_length GLOB '[0-9]*' \
                            THEN CAST(focal_length AS REAL) END AS f, iso, \
                    CASE WHEN f_number GLOB '[0-9]*' \
                         THEN CAST(f_number AS REAL) END AS ap, \
                    CASE WHEN exposure_time GLOB '1/[0-9]*' \
                         THEN 1.0 / CAST(substr(exposure_time, 3) AS REAL) \
                         WHEN exposure_time GLOB '[0-9]*' \
                         THEN CAST(exposure_time AS REAL) END AS sec \
                    FROM assets WHERE kind IN ('photo', 'raw') AND in_trash = 0 \
                 )",
            )
            .map_err(|e| e.to_string())?;
        let counts = stmt
            .query_row([], |r| {
                let mut row = [0i64; 25];
                for (i, slot) in row.iter_mut().enumerate() {
                    *slot = r.get::<_, Option<i64>>(i)?.unwrap_or(0);
                }
                Ok(row)
            })
            .map_err(|e| e.to_string())?;
        Ok(counts.to_vec())
    }

    /// 收藏旗标写入（0/1；资产不存在返回 false）。
    pub fn set_asset_flagged(&self, id: i64, flagged: bool) -> Result<bool> {
        let n = self.0.execute(
            "UPDATE assets SET flagged = ?2 WHERE id = ?1",
            params![id, i64::from(flagged)],
        )?;
        Ok(n > 0)
    }

    // —— 选片状态（0016：颜色标签 / 拒绝；批量）——

    /// 批量写颜色标签（token 已由 IPC 层校验；None = 清除）。
    /// 返回实际更新行数（失效 id 自然不计）。XMP 写回由调用方派发。
    pub fn assets_label_set(&self, ids: &[i64], label: Option<&str>) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!("UPDATE assets SET color_label = ?1 WHERE id IN ({slots})"),
            rusqlite::params_from_iter(
                std::iter::once(rusqlite::types::Value::from(label.map(str::to_string)))
                    .chain(ids.iter().map(|id| rusqlite::types::Value::from(*id))),
            ),
        )?;
        Ok(n as u64)
    }

    /// 批量写接受/拒绝状态（布尔语义 0/1）。返回实际更新行数。
    pub fn assets_reject_set(&self, ids: &[i64], rejected: bool) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 2))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!("UPDATE assets SET rejected = ?1 WHERE id IN ({slots})"),
            rusqlite::params_from_iter(
                std::iter::once(i64::from(rejected)).chain(ids.iter().copied()),
            ),
        )?;
        Ok(n as u64)
    }

    // —— 应用内回收站（0016：软删标记 + 显式恢复/清除）——

    /// 移入回收站（幂等：已在站的行不动、trashed_at 不刷新）。返回新移入数。
    pub fn assets_trash_move(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!(
                "UPDATE assets SET in_trash = 1, trashed_at = ?{0} \
                 WHERE id IN ({slots}) AND in_trash = 0",
                ids.len() + 1
            ),
            rusqlite::params_from_iter(
                ids.iter()
                    .map(|id| rusqlite::types::Value::from(*id))
                    .chain(std::iter::once(rusqlite::types::Value::from(now_rfc3339()))),
            ),
        )?;
        Ok(n as u64)
    }

    /// 回收站列表（trashed_at DESC、id DESC keyset；复用画廊分页行结构）。
    /// 游标 after_id 为上一页末行 id（0 = 第一页；行已删/已恢复按第一页）。
    pub fn trash_list(&self, after_id: i64, limit: u32) -> Result<Vec<AssetPageRow>> {
        let (cursor_key, cursor_id) = if after_id > 0 {
            match self.0.query_row(
                "SELECT trashed_at FROM assets WHERE id = ?1 AND in_trash = 1",
                [after_id],
                |r| r.get::<_, String>(0),
            ) {
                Ok(key) => (key, after_id),
                Err(_) => (now_rfc3339(), 0), // 游标失效：回退第一页
            }
        } else {
            (now_rfc3339(), 0)
        };
        // 哨兵 = 当前时刻：trashed_at 恒不晚于 now，第一页必含最新行
        let mut stmt = self.0.prepare(&format!(
            "SELECT {ASSET_PAGE_COLS} FROM assets \
             WHERE in_trash = 1 \
               AND (?2 = 0 OR trashed_at < ?1 OR (trashed_at = ?1 AND id < ?2)) \
             ORDER BY trashed_at DESC, id DESC LIMIT ?3",
        ))?;
        let rows = stmt.query_map(params![cursor_key, cursor_id, limit], map_asset_page)?;
        rows.collect()
    }

    /// 回收站还原（幂等）。返回还原行数。
    pub fn trash_restore(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let n = self.0.execute(
            &format!(
                "UPDATE assets SET in_trash = 0, trashed_at = NULL \
                 WHERE id IN ({slots}) AND in_trash = 1"
            ),
            rusqlite::params_from_iter(ids.iter().copied()),
        )?;
        Ok(n as u64)
    }

    /// 回收站内资产行（id → (path, origin)）：purge 物理删除前取清单，
    /// 外部库（origin='external'，文件不在库内）绝不物理删。
    pub fn trash_entries(&self, ids: &[i64]) -> Result<Vec<(i64, String, String)>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let mut stmt = self.0.prepare(&format!(
            "SELECT id, path, origin FROM assets \
             WHERE id IN ({slots}) AND in_trash = 1"
        ))?;
        let rows = stmt.query_map(rusqlite::params_from_iter(ids.iter()), |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.collect()
    }

    /// 资产记录级删除（duplicate_delete / trash_purge 共用路径）：事务内
    /// 清 pair 双向引用 → 删行（album_item/faces/view_history/similar_bucket/
    /// index_tasks 等经既有 FK ON DELETE CASCADE 级联）→ 清空组。返回删除数。
    /// 物理文件删除由调用方负责（失败容忍记账，见 ipc::duplicates）。
    pub fn assets_delete_rows(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let slots = (0..ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect::<Vec<_>>()
            .join(", ");
        let tx = self.0.unchecked_transaction()?;
        tx.execute(
            &format!("UPDATE assets SET pair_asset_id = NULL WHERE pair_asset_id IN ({slots})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        let n = tx.execute(
            &format!("DELETE FROM assets WHERE id IN ({slots})"),
            rusqlite::params_from_iter(ids.iter()),
        )?;
        tx.commit()?;
        Ok(n as u64)
    }

    // —— 智能视图（0016：AssetFilters 序列化的命名存取，后端不解释）——

    /// 智能视图列表（created_at DESC、id DESC）。
    pub fn smart_view_list(&self) -> Result<Vec<SmartViewRow>> {
        let mut stmt = self.0.prepare(
            "SELECT id, name, filters_json, created_at FROM smart_view \
             ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(SmartViewRow {
                id: row.get(0)?,
                name: row.get(1)?,
                filters_json: row.get(2)?,
                created_at: row.get(3)?,
            })
        })?;
        rows.collect()
    }

    /// 建智能视图（重名由 name UNIQUE 兜底，错误透传由 IPC 层转文案）。
    pub fn smart_view_create(&self, name: &str, filters_json: &str) -> Result<SmartViewRow> {
        let created_at = now_rfc3339();
        self.0.execute(
            "INSERT INTO smart_view (name, filters_json, created_at) VALUES (?1, ?2, ?3)",
            params![name, filters_json, created_at],
        )?;
        Ok(SmartViewRow {
            id: self.0.last_insert_rowid(),
            name: name.to_string(),
            filters_json: filters_json.to_string(),
            created_at,
        })
    }

    /// 删智能视图；不存在报错（幂等删除由 IPC 语义定）。
    pub fn smart_view_delete(&self, id: i64) -> Result<()> {
        let n = self
            .0
            .execute("DELETE FROM smart_view WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(Error::QueryReturnedNoRows);
        }
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

    /// 启动自愈：孤儿导入任务终老（2026-09-21）。进程重启后引擎会话天然
    /// 清零，jobs 表遗留的 running/paused 是跨会话死任务（真库 job 18
    /// running 挂 3 天、任务抽屉删不掉）——统一转 cancelled、补 finished_at，
    /// 每任务另写一行日志。journal 行不动：resume_import 不校验终态，设备
    /// 回连后仍可从 journal 手动续传（终老只清「死状态」，不毁恢复语义）。
    /// 只动 kind='import'：photo-root-migrate 任务有自己的跨会话续跑语义
    /// （db_dir_migrate 时显式恢复未完成任务），不得误伤。
    /// 返回终老的 job id 列表（空 = 无孤儿，幂等）。
    pub fn reap_orphan_import_jobs(&self) -> Result<Vec<i64>> {
        let now = now_rfc3339();
        let tx = self.0.unchecked_transaction()?;
        let ids: Vec<i64> = {
            let mut stmt = tx.prepare(
                "UPDATE jobs SET status = 'cancelled', finished_at = ?1 \
                 WHERE kind = 'import' AND status IN ('running', 'paused') \
                 RETURNING id",
            )?;
            let rows = stmt.query_map(params![now], |r| r.get(0))?;
            rows.collect::<std::result::Result<Vec<i64>, _>>()?
        };
        for id in &ids {
            tx.execute(
                "INSERT INTO logs (ts, level, job_id, message) VALUES (?1, 'warn', ?2, ?3)",
                params![
                    now,
                    id,
                    "进程重启：孤儿任务无活跃引擎会话，自动终老为 cancelled（journal 保留，设备回连后可手动恢复）"
                ],
            )?;
        }
        tx.commit()?;
        Ok(ids)
    }

    /// 删除任务历史（用户语义：这条历史连同日志一起消失）。
    /// 非终态（running/paused）拒绝；终态（done/cancelled/failed）删
    /// logs + jobs（job_files 经 FK ON DELETE CASCADE 级联清）。
    /// 返回 Ok(false) = 任务不存在；Err = 进行中。
    pub fn delete_job_history(&self, job_id: i64) -> Result<Result<bool, String>> {
        let status: Option<String> = self
            .0
            .query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
                r.get(0)
            })
            .map(Some)
            .or_else(|e| match e {
                Error::QueryReturnedNoRows => Ok(None),
                other => Err(other),
            })?;
        let Some(status) = status else {
            return Ok(Ok(false)); // 不存在：幂等删除
        };
        if !matches!(status.as_str(), "done" | "cancelled" | "failed") {
            return Ok(Err("任务进行中，无法删除".into()));
        }
        let tx = self.0.unchecked_transaction()?;
        tx.execute("DELETE FROM logs WHERE job_id = ?1", [job_id])?;
        tx.execute("DELETE FROM jobs WHERE id = ?1", [job_id])?; // job_files 级联
        tx.commit()?;
        Ok(Ok(true))
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

/// 资产入库核（连接/事务通用）：同路径重复导入整行覆盖（REPLACE 换 id 时
/// 自动重指 pair_asset_id 既有引用）→ 按目录/配对规则双向写 pair →
/// 索引待办 → （可选）同事务挂相册。见 [`Db::insert_asset`] /
/// [`Db::insert_asset_with_album`]。
fn insert_asset_on(conn: &Connection, a: &AssetRow, album_id: Option<i64>) -> Result<()> {
    let old_id: Option<i64> = conn
        .query_row("SELECT id FROM assets WHERE path = ?1", [&a.path], |r| {
            r.get(0)
        })
        .map(Some)
        .or_else(|e| match e {
            Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?;
    conn.execute(
        "INSERT OR REPLACE INTO assets \
         (path, filename, size, mtime, xxhash, kind, captured_at, camera, source, \
         created_at, origin, width, height, iso, f_number, exposure_time, focal_length, \
         lens, pair_asset_id, thumb_state, orientation, flash, metering_mode, \
         white_balance, exposure_program, software, artist, gps_lat, gps_lon, \
         rating, flagged, color_label, rejected) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
         ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33)",
        params![
            a.path,
            a.filename,
            a.size as i64,
            a.mtime,
            a.xxhash as i64,
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
            a.rating,
            a.flagged,
            a.color_label,
            a.rejected,
        ],
    )?;
    let id: i64 = conn
        .query_row("SELECT id FROM assets WHERE path = ?1", [&a.path], |r| {
            r.get(0)
        })
        .map(Some)
        .or_else(|e| match e {
            Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })?
        .ok_or(Error::QueryReturnedNoRows)?;
    if let Some(old) = old_id {
        if old != id {
            conn.execute(
                "UPDATE assets SET pair_asset_id = ?2 WHERE pair_asset_id = ?1 AND id != ?2",
                params![old, id],
            )?;
        }
    }
    refresh_asset_pair_on(conn, id, &a.path)?;

    // 索引待办（导入/索引任务分离）：photo/raw 写 thumb 任务；video 留
    // thumb_state=0 走按需队列（M8：ffmpeg 海报，侧车失败由队列失败
    // 计数兜底）；other 无缩略图可言直接永久占位。REPLACE 旧资产行时
    // 其任务行随 ON DELETE CASCADE 消失，这里只补新行。
    if matches!(a.kind, AssetKind::Photo | AssetKind::Raw) {
        let now = now_rfc3339();
        conn.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('thumb', ?1, 'pending', 0, ?2, ?2)",
            params![id, now],
        )?;
    } else if a.kind != AssetKind::Video {
        conn.execute(
            "UPDATE assets SET thumb_state = 2 WHERE id = ?1",
            params![id],
        )?;
    }

    // 相册挂载（导入 album_id 通道，0015）：与资产入册同事务；REPLACE 换
    // id 时旧行随级联消失、此处按新 id 重写，天然幂等。相册已被并发删除
    // 时跳过（见 insert_asset_with_album 契约）。
    if let Some(album) = album_id {
        let album_alive: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM album WHERE id = ?1)",
                [album],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if album_alive {
            conn.execute(
                "INSERT OR IGNORE INTO album_item (album_id, asset_id, added_at) \
                 VALUES (?1, ?2, ?3)",
                params![album, id, now_rfc3339()],
            )?;
        }
    }
    Ok(())
}

/// 按 (同目录, 同 stem, 异扩展名) 找配对伙伴并双向写 pair_asset_id；
/// 多候选取 id 最小（先入册者）。目录前缀 = 原样前缀（含分隔符）匹配，
/// stem 大小写折叠（Windows 路径大小写不敏感）。
fn refresh_asset_pair_on(conn: &Connection, id: i64, path: &str) -> Result<()> {
    let (dir, stem, _ext) = split_dir_stem_ext(path);
    if stem.is_empty() || dir.is_empty() {
        return Ok(());
    }
    let prefix_chars = dir.chars().count(); // dir 已含尾分隔符
    let stem_chars = stem.chars().count();
    let mut stmt = conn.prepare(
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
    conn.execute(
        "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
        params![id, partner],
    )?;
    if let Some(p) = partner {
        conn.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
            params![p, id],
        )?;
    }
    Ok(())
}

fn map_album(row: &Row<'_>) -> Result<AlbumRow> {
    Ok(AlbumRow {
        id: row.get(0)?,
        name: row.get(1)?,
        cover_asset_id: row.get(2)?,
        item_count: row.get::<_, i64>(3)? as u64,
        created_at: row.get(4)?,
    })
}

/// 分页与计数共用条件，避免筛选结果数和实际分页发生口径偏差。
fn asset_filter_conditions(
    filters: &AssetFilters,
    params_vec: &mut Vec<rusqlite::types::Value>,
) -> Vec<String> {
    use rusqlite::types::Value as V;
    let mut conds: Vec<String> = Vec::new();
    let slot = |params_vec: &mut Vec<V>, v: V| -> String {
        let s = format!("?{}", params_vec.len() + 1);
        params_vec.push(v);
        s
    };
    // —— 回收站排除（0016）：常规查询全链路默认不可见（唯一入口 trash_list）——
    conds.push("a.in_trash = 0".to_string());
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
            .map(|k| slot(params_vec, V::from(k.as_db_str().to_string())))
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("kind IN ({slots})"));
    }
    if !cameras.is_empty() {
        let slots = cameras
            .iter()
            .map(|c| slot(params_vec, V::from(c.clone())))
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("camera IN ({slots})"));
    }
    if !lenses.is_empty() {
        let slots = lenses
            .iter()
            .map(|l| slot(params_vec, V::from(l.clone())))
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
                let s = slot(params_vec, V::from(format!("%.{f}")));
                format!("path LIKE {s}")
            })
            .collect::<Vec<_>>();
        conds.push(format!("({})", frags.join(" OR ")));
    }

    // —— 日期（RFC3339 定宽字典序比较即时间序）——
    if let Some(after) = &filters.captured_after {
        let s = slot(params_vec, V::from(after.clone()));
        conds.push(format!("captured_at >= {s}"));
    }
    if let Some(before) = &filters.captured_before {
        let s = slot(params_vec, V::from(before.clone()));
        conds.push(format!("captured_at <= {s}"));
    }

    // —— 数值范围（闭区间；列 NULL 时比较结果为 NULL → 行被排除，
    //    即「未知」不冒充任何区间，见 AssetFilters 注释）——
    let range = |conds: &mut Vec<String>,
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
        params_vec,
        "CAST(focal_length AS REAL)",
        filters.focal_min,
        filters.focal_max,
    );
    range(
        &mut conds,
        params_vec,
        "CAST(iso AS REAL)",
        filters.iso_min.map(|v| v as f64),
        filters.iso_max.map(|v| v as f64),
    );
    range(
        &mut conds,
        params_vec,
        "CAST(f_number AS REAL)",
        filters.aperture_min,
        filters.aperture_max,
    );
    // 快门秒数：exposure_time 为本应用写入的展示串（"1/250" / "0.4"），
    // 按写入格式解析——分数串取倒数，其余 CAST REAL。
    range(
            &mut conds,
            params_vec,
            "CASE WHEN exposure_time LIKE '1/%' THEN 1.0 / CAST(substr(exposure_time, 3) AS REAL) ELSE CAST(exposure_time AS REAL) END",
            filters.shutter_min,
            filters.shutter_max,
        );
    range(
        &mut conds,
        params_vec,
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
                let s = slot(params_vec, V::from("%fired%".to_string()));
                conds.push(format!("flash LIKE {s}"));
            }
            "off" => {
                let s = slot(params_vec, V::from("no_flash%".to_string()));
                conds.push(format!("(flash LIKE {s} AND flash IS NOT NULL)"));
            }
            "unknown" => conds.push("flash IS NULL".to_string()),
            _ => {} // 未知 token 容错：不过滤
        }
    }
    match filters.orientation.as_deref() {
            // width/height 任一 NULL → 比较为 NULL → 排除
            Some("landscape") => conds.push("((orientation BETWEEN 5 AND 8 AND height > width) OR (COALESCE(orientation, 1) NOT BETWEEN 5 AND 8 AND width > height))".into()),
            Some("portrait") => conds.push("((orientation BETWEEN 5 AND 8 AND width > height) OR (COALESCE(orientation, 1) NOT BETWEEN 5 AND 8 AND height > width))".into()),
            _ => {}
        }
    if let Some(has) = filters.has_gps {
        conds.push(if has {
            "gps_lat IS NOT NULL".into()
        } else {
            "gps_lat IS NULL".into()
        });
    }

    // —— 评分 / 旗标（0009；NOT NULL 无 NULL 态）——
    for (bound, cmp) in [(filters.rating_min, ">="), (filters.rating_max, "<=")] {
        if let Some(v) = bound {
            let s = slot(params_vec, V::from(v));
            conds.push(format!("rating {cmp} {s}"));
        }
    }
    if let Some(flag) = filters.flagged {
        conds.push(if flag {
            "flagged = 1".into()
        } else {
            "flagged = 0".into()
        });
    }
    if let Some(fav) = filters.favorite {
        conds.push(if fav {
            "(rating > 0 OR flagged = 1)".into()
        } else {
            "rating = 0 AND flagged = 0".into()
        });
    }

    // —— 相册维度（0015）：EXISTS 求交，相册不存在命中空集 ——
    if let Some(album) = filters.album_id {
        let s = slot(params_vec, V::from(album));
        conds.push(format!(
            "EXISTS (SELECT 1 FROM album_item ai WHERE ai.asset_id = a.id AND ai.album_id = {s})"
        ));
    }

    // —— 颜色标签 / 拒绝状态（0016；与星级同层级的选片筛选）——
    if let Some(label) = &filters.color_label {
        let s = slot(params_vec, V::from(label.clone()));
        conds.push(format!("color_label = {s}"));
    }
    if let Some(rej) = filters.rejected {
        conds.push(if rej {
            "rejected = 1".into()
        } else {
            "rejected = 0".into()
        });
    }

    conds
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
        burst_id: row.get(16)?,
        flagged: row.get::<_, i64>(17)? != 0,
        rating: row.get(18)?,
        color_label: row.get(19)?,
        rejected: row.get::<_, i64>(20)? != 0,
    })
}

fn map_asset_full(row: &Row<'_>) -> Result<AssetRow> {
    Ok(AssetRow {
        path: row.get(0)?,
        filename: row.get(1)?,
        size: row.get::<_, i64>(2)? as u64,
        mtime: row.get(3)?,
        xxhash: row.get::<_, i64>(4)? as u64,
        kind: row.get(5)?,
        captured_at: row.get(6)?,
        camera: row.get(7)?,
        source: row.get(8)?,
        created_at: row.get(9)?,
        origin: row.get(10)?,
        width: row.get::<_, Option<i64>>(11)?.map(|v| v as u32),
        height: row.get::<_, Option<i64>>(12)?.map(|v| v as u32),
        iso: row.get::<_, Option<i64>>(13)?.map(|v| v as u32),
        f_number: row.get(14)?,
        exposure_time: row.get(15)?,
        focal_length: row.get(16)?,
        lens: row.get(17)?,
        pair_asset_id: row.get(18)?,
        thumb_state: row.get::<_, Option<i64>>(19)?.unwrap_or(0) as i32,
        orientation: row.get(20)?,
        flash: row.get(21)?,
        metering_mode: row.get(22)?,
        white_balance: row.get(23)?,
        exposure_program: row.get(24)?,
        software: row.get(25)?,
        artist: row.get(26)?,
        gps_lat: row.get(27)?,
        gps_lon: row.get(28)?,
        rating: row.get(29)?,
        flagged: row.get(30)?,
        color_label: row.get(31)?,
        rejected: row.get(32)?,
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
        dst2: row.get(7)?,
    })
}
