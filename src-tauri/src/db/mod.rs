//! SQLite 基础 + journal（spec §5.4）：M1 T2 由核心 lane 实现。
//!
//! - [`Db::open`]：WAL + `foreign_keys=ON` + `busy_timeout=5s`（多连接并发）
//!   + 幂等执行单版本建表脚本 [`schema::SCHEMA`]（2026-10-09 单数据库多
//!   照片库定案：migrate 体系/user_version/并发首开迁移锁全部删除——
//!   建表脚本即唯一版本，全部对象 IF NOT EXISTS，并发首开由 DDL 逐语句
//!   原子性 + busy_timeout 收敛，无需进程级锁）。
//! - photos_libraries / jobs / job_files（断点恢复 journal）/ assets / logs
//!   的仓储方法。
//!
//! [`FileState`]/[`AssetKind`] 复用 events 模块的领域枚举，列存储格式与其
//! serde camelCase 字符串严格一致（手写 rusqlite To/FromSql 映射，不引 derive 扩展 crate）。

/// 单版本建表脚本（唯一 schema 真值）。
pub(crate) mod schema;

/// 选片会话仓储（0024 三表；独立子模块——并行 lane 常改本文件，缩小冲突面）。
pub mod culling;

/// photos_libraries 登记表仓储（单数据库多照片库）。
pub mod libraries;

mod albums;
mod assets;
mod editor;
mod groups;
mod import;
mod indexing;
mod maintenance;
mod people;
mod selection;

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
    /// WAL（读写不互斥）+ foreign_keys + 5s busy_timeout + 单版本 schema。
    /// busy_timeout 必须最先设置：journal_mode 的 WAL 转换在无忙等时
    /// 会瞬时返回 BUSY（多连接并发打开的真实竞态）。
    /// schema 全对象 IF NOT EXISTS，重开/并发首开均幂等（每连接一次
    /// execute_batch 的解析开销即可，无需迁移锁与 user_version）。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "wal")?;
        // NORMAL 是与 WAL 配套的常规同步档位（断电最多丢最后事务，不损坏库）。
        conn.pragma_update(None, "synchronous", "normal")?;
        conn.pragma_update(None, "foreign_keys", "on")?;
        conn.execute_batch(schema::SCHEMA)?;
        Ok(Self(conn))
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
    /// 'external' 原地索引入册（原片不由库清理或移动；用户主动修改评分等
    /// 信息时允许在原片旁同步 XMP 边车）。
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
    /// 0=pending 1=done 2=permanent-none（不可解码/生成失败）。
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
    // —— 单数据库多照片库（2026-10-09 定案，计划 §二/§三/§五）——
    /// 归属照片库（photos_libraries.id；静态属性：仅整库重定位改库 root，
    /// 归属不变。无外键——移除登记是否连记录删由应用层决定）。
    #[serde(default)]
    pub library_id: Option<String>,
    /// 单文件缺失标记（布尔语义 0/1；整库离线走 photos_libraries.status）。
    #[serde(default)]
    pub missing: i64,
    /// 离线期间元数据改动待补写边车（布尔语义 0/1；库恢复在线后
    /// reconcile 补写 XMP 边车并清标志）。
    #[serde(default)]
    pub xmp_dirty: i64,
    /// 登记指纹：卷序列号（增量扫描两级识别——同卷同 file id = 硬链接
    /// 零哈希直跳；exFAT/FAT 无 file id 时指纹列为 NULL 走哈希路径）。
    #[serde(default)]
    pub volume_serial: Option<i64>,
    /// 登记指纹：卷内文件 id（NTFS FILE_ID 128-bit 小写十六进制；
    /// 硬链接两侧同 id，rename 不变）。
    #[serde(default)]
    pub file_id: Option<String>,
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

/// export_job 行（0022 导出任务账）：状态机 queued→running→done|error；
/// result 四元组 + album 模式新资产 id（folder 模式 NULL）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportJobRow {
    pub id: i64,
    pub asset_id: i64,
    /// "folder" | "album"
    pub mode: String,
    /// "queued" | "running" | "done" | "error"
    pub status: String,
    pub output_path: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub bytes: Option<u64>,
    pub new_asset_id: Option<i64>,
    pub error: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
}

/// album_export_job 行（M6 相册导出为文件夹任务账，§六 LR 互操作）：
/// queued→running→done|cancelled|error；total/done/linked 为进度计数
///（源 missing 跳过不计 done——前端以 total-done 差值出「跳过」总结）。
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumExportJobRow {
    pub id: i64,
    pub album_id: i64,
    /// 子分组名（None = 整个相册）。
    pub subgroup: Option<String>,
    pub output_dir: String,
    /// "queued" | "running" | "done" | "cancelled" | "error"
    pub status: String,
    /// 相册内待导出成员数。
    pub total: u64,
    /// 已导出数。
    pub done: u64,
    /// 其中硬链接落盘数（其余为拷贝）。
    pub linked: u64,
    pub error: Option<String>,
    pub created_at: String,
    pub finished_at: Option<String>,
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
    /// 相册子分组（0019）：精确名匹配——「album_assets_page + album_id」
    /// 下只看该子分组（EXISTS album_item 命中）；无 album_id 时按任意
    /// 相册的同名子分组匹配。仅 album 视图有意义。
    pub subgroup: Option<String>,
    /// 相册根散照片（0019）：true → 只看 album_id 相册中 subgroup IS NULL
    /// 的引用（文件夹树的「根」视图）。false/None = 不限。
    pub subgroup_is_null: Option<bool>,
    /// 闭眼筛选（0021）："closed" | "maybe"——命中 ai_analysis('eyes') 的
    /// 对应 value。其他值容错不过滤（建议标签，非定罪）。
    pub eyes: Option<String>,
    /// 失焦筛选（0021）："soft"——命中 ai_analysis('blur') 的 soft 判定。
    /// 其他值容错不过滤。
    pub blur: Option<String>,
    /// 所属照片库多选（OR，§一「来自哪个库」可筛选不可操作）：画廊默认
    /// 全局跨库混排，按需过滤。空 = 不过滤（2026-10-09 M2c）。
    pub library_ids: Vec<String>,
    /// 缺失三态（§五 M2c）：true → 仅 missing=1（缺失角标筛选）；
    /// false → 仅在线；None = 不过滤。整库离线走 photos_libraries.status，
    /// 不在本条件语义内。
    pub missing: Option<bool>,
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
    /// RAW/JPG 展示组 id（双方资产 ID 的较小值，无配对为 None）。
    pub pair_id: Option<i64>,
    /// 缩略图状态（0 pending / 1 done / 2 permanent-none）。
    pub thumb_state: i32,
    /// 连拍组 id（M6；未入组 None）。
    pub burst_id: Option<i64>,
    pub flagged: bool,
    pub rating: i64,
    /// 颜色标签（0016；无标签 None）。
    pub color_label: Option<String>,
    /// 拒绝状态：接受/拒绝状态（布尔语义）。
    pub rejected: bool,
    /// 单文件缺失标记（§五 M2c；画廊缺失角标数据源）。
    pub missing: bool,
    pub library_id: Option<String>,
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
/// 的元数据面——name 全库唯一；cover_asset_id 只是封面引用（资产永久删除
/// 时 FK SET NULL 自动解除）；item_count 为引用数（相册为空合法）。
/// dir_name 为历史物理目录名残留列（相册物理目录化已随 2026-10-09 纯时间
/// 布局退役，见 schema.rs 注释；仅为显示名净化唯一性的载体保留）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumRow {
    pub id: i64,
    pub name: String,
    pub cover_asset_id: Option<i64>,
    pub item_count: u64,
    pub created_at: String,
    /// 物理主目录名（0018；存量行 = `album-{id}` 回填）。
    pub dir_name: String,
}

/// 相册导出成员投影（album_export_members 行，M6 §六；仅导出所需列）。
#[derive(Debug, Clone, PartialEq)]
pub struct AlbumExportMember {
    pub asset_id: i64,
    pub path: String,
    pub filename: String,
    /// 单文件缺失标记（§五：缺失成员跳过导出并在总结报告）。
    pub missing: bool,
    /// 评分 0-5（边车投影用）。
    pub rating: i64,
    /// 颜色标签（LR 五色小写 token；None=无）。
    pub color_label: Option<String>,
    /// 拒绝旗标（边车评分投影为 -1）。
    pub rejected: bool,
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

/// ai_analysis 行（0021：kind/value/score/model_version）。
pub type AiAnalysisRow = (String, Option<String>, Option<f64>, String);

/// 分页行投影列（[`map_asset_page`] 消费顺序；各查询共用，防列序漂移）。
const ASSET_PAGE_COLS: &str = "id, path, filename, size, kind, captured_at, camera, \
     width, height, iso, f_number, exposure_time, focal_length, lens, pair_asset_id, \
     thumb_state, burst_id, flagged, rating, color_label, rejected, missing, library_id";
/// 同 [`ASSET_PAGE_COLS`] 的 `a.` 别名前缀形态（内层子查询用）。
const ASSET_PAGE_COLS_A: &str = "a.id, a.path, a.filename, a.size, a.kind, a.captured_at, \
     a.camera, a.width, a.height, a.iso, a.f_number, a.exposure_time, a.focal_length, a.lens, \
     a.pair_asset_id, a.thumb_state, a.burst_id, a.flagged, a.rating, a.color_label, \
     a.rejected, a.missing, a.library_id";

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

pub(crate) fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 路径前缀归一字符（大小写/斜杠方向不敏感比较用）。
fn root_norm_char(c: char) -> char {
    if c == '/' {
        '\\'
    } else {
        c.to_ascii_lowercase()
    }
}

/// path 前缀 == root（忽略大小写与 `/`\\` 方向；root 结尾后必须是分隔符
/// 或恰好用尽，防 `I:\\x` 误匹配 `I:\\xy`）→ 返回 path 中前缀之后的原始
/// 尾段（保留原分隔符形态；根自带尾分隔符时吃掉一层）。
/// 库重定位（[`Db::rewrite_asset_roots`]）共用。
pub fn strip_root_prefix<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    // 根尾分隔符归一吃掉（`I:\x\` 与 `I:\x` 同一语义）
    let root = root.trim_end_matches(|c| c == '/' || c == '\\');
    let mut p = path.chars();
    let mut r = root.chars();
    loop {
        match (r.next(), p.next()) {
            (Some(rc), Some(pc)) => {
                if root_norm_char(rc) != root_norm_char(pc) {
                    return None;
                }
            }
            // root 用尽：p 剩余首字符必须是分隔符（或 path 也用尽——防御）
            (None, Some(pc)) => {
                if pc == '/' || pc == '\\' {
                    // 尾段从分隔符之后起（分隔符不吃进尾段）
                    return Some(p.as_str());
                }
                return None;
            }
            (None, None) => return Some(""),
            // root 比 path 长
            (Some(_), None) => return None,
        }
    }
}

/// 资产入库核（连接/事务通用）：同路径重复导入整行覆盖（REPLACE 换 id 时
/// 自动重指 pair_asset_id 既有引用）→ 按目录/配对规则双向写 pair →
/// 索引待办 → （可选）同事务挂相册。见 [`Db::insert_asset`] /
/// [`Db::insert_asset_with_album`]。
fn insert_asset_on(
    conn: &Connection,
    a: &AssetRow,
    album_id: Option<i64>,
    album_subgroup: Option<&str>,
) -> Result<()> {
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
         rating, flagged, color_label, rejected, library_id, missing, xmp_dirty, \
         volume_serial, file_id) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, \
         ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, \
         ?33, ?34, ?35, ?36, ?37, ?38)",
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
            a.library_id,
            a.missing,
            a.xmp_dirty,
            a.volume_serial,
            a.file_id,
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

    // 索引待办（导入/索引任务分离）：photo/raw 写 thumb 任务；
    // other 无缩略图可言直接永久占位。REPLACE 旧资产行时
    // 其任务行随 ON DELETE CASCADE 消失，这里只补新行。
    // 0021：photo/raw 同时登记 eyes/blur 分析任务（闭眼/疑似失焦）。
    if matches!(a.kind, AssetKind::Photo | AssetKind::Raw) {
        let now = now_rfc3339();
        for kind in ["thumb", "eyes", "blur"] {
            conn.execute(
                "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
                 VALUES (?2, ?1, 'pending', 0, ?3, ?3)",
                params![id, kind, now],
            )?;
        }
    } else {
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
                "INSERT OR IGNORE INTO album_item (album_id, asset_id, added_at, subgroup) \
                 VALUES (?1, ?2, ?3, ?4)",
                params![album, id, now_rfc3339(), album_subgroup],
            )?;
        }
    }
    Ok(())
}

/// 按 (同目录, 同 stem, 异扩展名) 找配对伙伴并双向写 pair_asset_id；
/// 多候选取 id 最小（先入册者）。目录前缀 = 原样前缀（含分隔符）匹配，
/// stem 大小写折叠（Windows 路径大小写不敏感）。找到 RAW+照片孪生时
/// 同步建组（0017 photo_group：raw+sooc 同组，见 [`group_pair_on`]）。
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
        // RAW+JPEG 孪生 → photo_group 归组（角色按 kind：raw/sooc）
        let kind_of = |aid: i64| -> Result<String> {
            conn.query_row("SELECT kind FROM assets WHERE id = ?1", [aid], |r| {
                r.get::<_, String>(0)
            })
        };
        let (ka, kb) = (kind_of(id)?, kind_of(p)?);
        let role_of = |k: &str| -> Option<&'static str> {
            match k {
                "raw" => Some("raw"),
                "photo" => Some("sooc"),
                _ => None,
            }
        };
        if let (Some(ra), Some(rb)) = (role_of(&ka), role_of(&kb)) {
            group_pair_on(conn, id, ra, p, rb)?;
        }
    }
    Ok(())
}

/// 一次快门的 RAW+照片孪生入组（0017）：两边任一已有组则归入该组（两组
/// 并存时把另一组合并进来，防 pair 拓扑演化出分叉组）；都无组则建新组。
/// 幂等：重复配对只校正 role 不重复建组。
fn group_pair_on(
    conn: &Connection,
    a_id: i64,
    a_role: &str,
    b_id: i64,
    b_role: &str,
) -> Result<()> {
    let group_of = |aid: i64| -> Result<Option<i64>> {
        conn.query_row(
            "SELECT group_id FROM group_asset WHERE asset_id = ?1",
            [aid],
            |r| r.get(0),
        )
        .map(Some)
        .or_else(|e| match e {
            Error::QueryReturnedNoRows => Ok(None),
            other => Err(other),
        })
    };
    let (ga, gb) = (group_of(a_id)?, group_of(b_id)?);
    let target = match (ga, gb) {
        (Some(x), Some(y)) if x == y => x,
        (Some(x), _) => x,
        (_, Some(y)) => y,
        (None, None) => {
            conn.execute("INSERT INTO photo_group DEFAULT VALUES", [])?;
            conn.last_insert_rowid()
        }
    };
    // 合并另一组（组员整体并入 target 后删除空壳；UPDATE OR REPLACE 处理
    // 组员已在 target 的主键冲突）
    if let Some(g) = gb {
        if g != target {
            conn.execute(
                "UPDATE OR REPLACE group_asset SET group_id = ?2 WHERE group_id = ?1",
                params![g, target],
            )?;
            conn.execute("DELETE FROM photo_group WHERE id = ?1", [g])?;
        }
    }
    // 防御：两资产的其他历史归属一律清除（一资产至多属一组）
    conn.execute(
        "DELETE FROM group_asset WHERE asset_id IN (?1, ?2) AND group_id != ?3",
        params![a_id, b_id, target],
    )?;
    for (aid, role) in [(a_id, a_role), (b_id, b_role)] {
        conn.execute(
            "INSERT INTO group_asset (group_id, asset_id, role) VALUES (?1, ?2, ?3) \
             ON CONFLICT (group_id, asset_id) DO UPDATE SET role = excluded.role",
            params![target, aid, role],
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
        dir_name: row.get(5)?,
    })
}

/// 默认相册（系统级保底）：导入必落相册的归宿；禁删禁改名（用户定案
/// 2026-09-27）——完全固定，永远是兜底落点。
pub const DEFAULT_ALBUM_NAME: &str = "未分组";

/// 相册显示名 → 物理目录名（0018，跨平台安全）：Windows 非法字符
/// `< > : " / \ | ? *` 与控制符折叠为 `-`；去掉结尾的点/空格（Win32 路径
/// 语义）；保留中文等 Unicode 字母；Windows 保留设备名（CON/PRN/NUL/
/// COM1-9/LPT1-9）加 `album-` 前缀；超长截断 80 字符；空结果回退 `album`。
pub fn sanitize_dir_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.trim().chars() {
        if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() {
            out.push('-');
        } else {
            out.push(c);
        }
    }
    let mut out = out.trim().trim_end_matches(['.', ' ']).to_string();
    if out.chars().count() > 80 {
        out = out.chars().take(80).collect();
        out = out.trim_end_matches(['.', ' ', '-']).to_string();
    }
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&out.to_ascii_uppercase().as_str()) {
        out = format!("album-{out}");
    }
    if out.is_empty() {
        out = "album".to_string();
    }
    out
}

/// SQL CAST alone accepts nonnumeric labels as zero and numeric prefixes as real
/// values. Require the entire decimal to be valid and positive; NULL then never
/// matches a range or a bucket. Column expressions are internal, never user input.
fn positive_decimal_sql(column: &str) -> String {
    let value = format!("trim({column})");
    format!(
        "(CASE WHEN {value} <> '' AND {value} NOT GLOB '*[^0-9.]*' \
        AND {value} GLOB '*[0-9]*' \
        AND length({value}) - length(replace({value}, '.', '')) <= 1 \
        AND CAST({value} AS REAL) > 0 THEN CAST({value} AS REAL) END)"
    )
}

fn exposure_seconds_sql(column: &str) -> String {
    let numerator = positive_decimal_sql(&format!("substr({column}, 1, instr({column}, '/') - 1)"));
    let denominator = positive_decimal_sql(&format!("substr({column}, instr({column}, '/') + 1)"));
    let decimal = positive_decimal_sql(column);
    format!(
        "(CASE WHEN instr({column}, '/') > 0 THEN {numerator} / {denominator} ELSE {decimal} END)"
    )
}

fn equipment_text_sql(column: &str) -> String {
    let missing = crate::metadata::exif_lite::MISSING_EQUIPMENT_TEXT
        .iter()
        .map(|value| format!("'{value}'"))
        .collect::<Vec<_>>()
        .join(", ");
    let value = format!("trim({column}, char(9) || char(10) || char(13) || ' ')");
    format!("(CASE WHEN lower({value}) NOT IN ({missing}) THEN {value} END)")
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
    conds.push("a.kind IN ('photo', 'raw')".to_string());
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
            .map(|c| slot(params_vec, V::from(c.trim().to_string())))
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("{} IN ({slots})", equipment_text_sql("camera")));
    }
    if !lenses.is_empty() {
        let slots = lenses
            .iter()
            .map(|l| slot(params_vec, V::from(l.trim().to_string())))
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("{} IN ({slots})", equipment_text_sql("lens")));
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
        &positive_decimal_sql("focal_length"),
        filters.focal_min,
        filters.focal_max,
    );
    range(
        &mut conds,
        params_vec,
        &positive_decimal_sql("iso"),
        filters.iso_min.map(|v| v as f64),
        filters.iso_max.map(|v| v as f64),
    );
    range(
        &mut conds,
        params_vec,
        &positive_decimal_sql("f_number"),
        filters.aperture_min,
        filters.aperture_max,
    );
    range(
        &mut conds,
        params_vec,
        &exposure_seconds_sql("exposure_time"),
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

    // —— 相册子分组（0019）：依附 album 维度（相册视图覆写 album_id 后
    //    传到这里）；无 album_id 时按任意相册同名子分组匹配（容错）——
    let album_clause = match filters.album_id {
        Some(album) => {
            let s = slot(params_vec, V::from(album));
            format!("ai.album_id = {s}")
        }
        None => "1 = 1".to_string(),
    };
    if let Some(sub) = &filters.subgroup {
        let s = slot(params_vec, V::from(sub.clone()));
        conds.push(format!(
            "EXISTS (SELECT 1 FROM album_item ai WHERE ai.asset_id = a.id              AND {album_clause} AND ai.subgroup = {s})"
        ));
    }
    if filters.subgroup_is_null == Some(true) {
        conds.push(format!(
            "EXISTS (SELECT 1 FROM album_item ai WHERE ai.asset_id = a.id              AND {album_clause} AND ai.subgroup IS NULL)"
        ));
    }

    // —— AI 选片建议（0021）：eyes/blur 命中 ai_analysis 对应 value；
    //    只认白名单值（closed/maybe、soft），其余容错不过滤 ——
    if let Some(eyes) = &filters.eyes {
        if matches!(eyes.as_str(), "closed" | "maybe") {
            let s = slot(params_vec, V::from(eyes.clone()));
            conds.push(format!(
                "EXISTS (SELECT 1 FROM ai_analysis aa WHERE aa.asset_id = a.id                  AND aa.kind = 'eyes' AND aa.value = {s})"
            ));
        }
    }
    if let Some(blur) = &filters.blur {
        if blur == "soft" {
            conds.push(
                "EXISTS (SELECT 1 FROM ai_analysis aa WHERE aa.asset_id = a.id                  AND aa.kind = 'blur' AND aa.value = 'soft')"
                    .to_string(),
            );
        }
    }

    // —— 照片库归属（§一 M2c：画廊默认全局跨库混排，按需过滤；上限 16）——
    let library_ids: Vec<String> = filters
        .library_ids
        .iter()
        .filter(|id| !id.trim().is_empty())
        .take(16)
        .cloned()
        .collect();
    if !library_ids.is_empty() {
        let slots = library_ids
            .iter()
            .map(|id| slot(params_vec, V::from(id.clone())))
            .collect::<Vec<_>>()
            .join(", ");
        conds.push(format!("library_id IN ({slots})"));
    }

    // —— 缺失三态（§五 M2c）：缺失角标筛选 ——
    if let Some(missing) = filters.missing {
        conds.push(if missing {
            "missing = 1".into()
        } else {
            "missing = 0".into()
        });
    }

    conds
}

fn map_asset_page(row: &Row<'_>) -> Result<AssetPageRow> {
    let id: i64 = row.get(0)?;
    Ok(AssetPageRow {
        id,
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
        // 数据库保存双向伙伴引用；展示契约需要两侧共有的组 ID。
        pair_id: row
            .get::<_, Option<i64>>(14)?
            .map(|partner| partner.min(id)),
        thumb_state: row.get::<_, Option<i64>>(15)?.unwrap_or(0) as i32,
        burst_id: row.get(16)?,
        flagged: row.get::<_, i64>(17)? != 0,
        rating: row.get(18)?,
        color_label: row.get(19)?,
        rejected: row.get::<_, i64>(20)? != 0,
        missing: row.get::<_, Option<i64>>(21)?.unwrap_or(0) != 0,
        library_id: row.get(22)?,
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
        flagged: row.get::<_, i64>(30)?,
        color_label: row.get(31)?,
        rejected: row.get::<_, i64>(32)?,
        library_id: row.get(33)?,
        missing: row.get::<_, Option<i64>>(34)?.unwrap_or(0),
        xmp_dirty: row.get::<_, Option<i64>>(35)?.unwrap_or(0),
        volume_serial: row.get(36)?,
        file_id: row.get(37)?,
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

fn map_export_job(row: &Row<'_>) -> Result<ExportJobRow> {
    Ok(ExportJobRow {
        id: row.get(0)?,
        asset_id: row.get(1)?,
        mode: row.get(2)?,
        status: row.get(3)?,
        output_path: row.get(4)?,
        width: row.get::<_, Option<i64>>(5)?.map(|v| v as u32),
        height: row.get::<_, Option<i64>>(6)?.map(|v| v as u32),
        bytes: row.get::<_, Option<i64>>(7)?.map(|v| v as u64),
        new_asset_id: row.get(8)?,
        error: row.get(9)?,
        created_at: row.get(10)?,
        finished_at: row.get(11)?,
    })
}

fn map_album_export_job(row: &Row<'_>) -> Result<AlbumExportJobRow> {
    Ok(AlbumExportJobRow {
        id: row.get(0)?,
        album_id: row.get(1)?,
        subgroup: row.get(2)?,
        output_dir: row.get(3)?,
        status: row.get(4)?,
        total: row.get::<_, i64>(5)? as u64,
        done: row.get::<_, i64>(6)? as u64,
        linked: row.get::<_, i64>(7)? as u64,
        error: row.get(8)?,
        created_at: row.get(9)?,
        finished_at: row.get(10)?,
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
