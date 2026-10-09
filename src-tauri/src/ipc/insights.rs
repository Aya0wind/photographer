//! 洞察命令（M7）：F6 那年今天 + F9 器材统计。契约对齐前端 api.ts
//! （onThisDay → AssetDto[]；GearStats 六字段形状，label 后端生成）。

use serde::{Deserialize, Serialize};
use tauri::State;

use super::{run_blocking, SharedState};

/// 机身/镜头计数（gear_stats；count 降序）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GearNameCountDto {
    pub name: String,
    pub count: u64,
}

/// 焦段桶（min 含 / max 不含；200+ 开放桶 max=null——前端类型如此）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GearFocalBucketDto {
    pub label: String,
    pub min: i64,
    pub max: Option<i64>,
    pub count: u64,
}

/// 标签计数桶（ISO/光圈/快门共用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GearLabelBucketDto {
    pub label: String,
    pub count: u64,
}

/// 器材统计快照（无任何 EXIF 数据时 None——前端空态降级）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GearStatsDto {
    pub cameras: Vec<GearNameCountDto>,
    pub lenses: Vec<GearNameCountDto>,
    pub focal_buckets: Vec<GearFocalBucketDto>,
    pub iso_buckets: Vec<GearLabelBucketDto>,
    pub aperture_buckets: Vec<GearLabelBucketDto>,
    pub shutter_buckets: Vec<GearLabelBucketDto>,
}

/// 那年今天核：本地时区今天的 month-day（含今年），历年同月日资产
/// （年份 DESC、年内时间 ASC）。今天无历史 → 空表。
pub fn fetch_on_this_day(state: &super::AppState) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::app_database_db(state)?;
    let month_day = chrono::Local::now().format("%m-%d").to_string();
    let rows = db
        .assets_on_this_day(&month_day)
        .map_err(|e| e.to_string())?;
    let mut dtos: Vec<_> = rows
        .into_iter()
        .map(super::assets::page_row_to_dto)
        .collect();
    super::assets::attach_burst_counts_pub(&db, &mut dtos);
    Ok(dtos)
}

/// 焦段桶定义：(label, min, max)。
const FOCAL_BUCKETS: [(&str, i64, Option<i64>); 6] = [
    ("<24mm", 0, Some(24)),
    ("24-50mm", 24, Some(50)),
    ("50-85mm", 50, Some(85)),
    ("85-135mm", 85, Some(135)),
    ("135-200mm", 135, Some(200)),
    ("200+mm", 200, None),
];
/// ISO 桶标签（7 桶：≤100 / 100-200 / … / 1600-3200 / 3200+）。
const ISO_LABELS: [&str; 7] = [
    "≤100",
    "100-200",
    "200-400",
    "400-800",
    "800-1600",
    "1600-3200",
    "3200+",
];
/// 光圈桶标签（6 桶）。
const APERTURE_LABELS: [&str; 6] = [
    "≤f/1.4",
    "f/1.4-2.8",
    "f/2.8-4",
    "f/4-5.6",
    "f/5.6-8",
    ">f/8",
];
/// 快门桶标签（6 桶，按秒）。
const SHUTTER_LABELS: [&str; 6] = [
    ">1s",
    "1-1/2s",
    "1/2-1/8s",
    "1/8-1/60s",
    "1/60-1/500s",
    "≤1/500s",
];

/// 器材统计核：相机/镜头聚合 + 四维分桶（单遍 SQL）。全空 → None。
pub fn fetch_gear_stats(state: &super::AppState) -> Result<Option<GearStatsDto>, String> {
    let db = super::app_database_db(state)?;
    let cameras = db.camera_list().map_err(|e| e.to_string())?;
    let lenses = db.lens_list().map_err(|e| e.to_string())?;
    let counts = db.gear_bucket_counts()?;
    let focal: Vec<_> = FOCAL_BUCKETS
        .iter()
        .zip(counts.iter().take(6))
        .map(|((label, min, max), c)| GearFocalBucketDto {
            label: (*label).into(),
            min: *min,
            max: *max,
            count: *c as u64,
        })
        .collect();
    let iso: Vec<_> = ISO_LABELS
        .iter()
        .zip(counts.iter().skip(6).take(7))
        .map(|(label, c)| GearLabelBucketDto {
            label: (*label).into(),
            count: *c as u64,
        })
        .collect();
    let aperture: Vec<_> = APERTURE_LABELS
        .iter()
        .zip(counts.iter().skip(13).take(6))
        .map(|(label, c)| GearLabelBucketDto {
            label: (*label).into(),
            count: *c as u64,
        })
        .collect();
    let shutter: Vec<_> = SHUTTER_LABELS
        .iter()
        .zip(counts.iter().skip(19).take(6))
        .map(|(label, c)| GearLabelBucketDto {
            label: (*label).into(),
            count: *c as u64,
        })
        .collect();
    let any = !cameras.is_empty()
        || !lenses.is_empty()
        || focal.iter().any(|b| b.count > 0)
        || iso.iter().any(|b| b.count > 0)
        || aperture.iter().any(|b| b.count > 0)
        || shutter.iter().any(|b| b.count > 0);
    if !any {
        return Ok(None); // 无 EXIF 数据：前端空态
    }
    Ok(Some(GearStatsDto {
        cameras: cameras
            .into_iter()
            .map(|c| GearNameCountDto {
                name: c.camera,
                count: c.count,
            })
            .collect(),
        lenses: lenses
            .into_iter()
            .map(|l| GearNameCountDto {
                name: l.camera,
                count: l.count,
            })
            .collect(),
        focal_buckets: focal,
        iso_buckets: iso,
        aperture_buckets: aperture,
        shutter_buckets: shutter,
    }))
}

/// 那年今天（历年同月日，年份降序块）。
#[tauri::command]
pub async fn on_this_day(
    state: State<'_, SharedState>,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_on_this_day).await
}

/// 器材统计（机身/镜头 TOP + 焦段/ISO/光圈/快门分布）。
#[tauri::command]
pub async fn gear_stats(state: State<'_, SharedState>) -> Result<Option<GearStatsDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_gear_stats).await
}

// ---------------------------------------------------------------------------
// 侧栏一次性计数（2026-09-21）：五项纯 COUNT/常量，绝不拉资产行
// ---------------------------------------------------------------------------

/// 预置标签墙词表规模（与前端 AlbumsPages.tsx 的 SMART_ALBUM_TAGS 对齐
/// ——40 个中文标签；v1 词表是前端常量而非 DB 数据，改词表时两处同步）。
/// 标签墙同时就是相册页的标签分区（/albums#tags 同页同区块）；前端本地
/// 隐藏（localStorage）的标签由前端自行扣减，后端计全量。
pub const SMART_ALBUM_TAG_COUNT: i64 = 40;

/// 侧栏计数载荷（camelCase；全 i64）。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SidebarCountsDto {
    /// 库内资产总数（全 kind——侧栏「照片」即画廊全量）。
    pub assets: i64,
    /// 浏览历史行数（view_history，每资产至多一行）。
    pub recent_viewed: i64,
    /// 那年今天条数（与 on_this_day 列表同口径：本地时区同月日的
    /// photo/raw；WHERE 片段在 db 层共用，无两处漂移）。
    pub on_this_day: i64,
    /// 标签墙标签数（预置词表全量，见 SMART_ALBUM_TAG_COUNT）。
    pub tags: i64,
    /// 用户相册数（0015 起真实 COUNT(album)——相册已是 DB 实体）。
    pub albums: i64,
}

/// 侧栏计数核：四条 COUNT/常量，绝不拉资产行。库未开 → Err（与洞察命令
/// 的 app_database_db 透传语义一致——前端侧栏在库开前后都有明确状态）。
pub fn fetch_sidebar_counts(state: &super::AppState) -> Result<SidebarCountsDto, String> {
    let db = super::app_database_db(state)?;
    let assets = db.sidebar_assets_count().map_err(|e| e.to_string())?;
    let recent_viewed = db.sidebar_viewed_count().map_err(|e| e.to_string())?;
    let albums = db.sidebar_albums_count().map_err(|e| e.to_string())?;
    // 日期窗口与 fetch_on_this_day 同源：本地时区今天 "%m-%d"
    let month_day = chrono::Local::now().format("%m-%d").to_string();
    let on_this_day = db
        .count_on_this_day(&month_day)
        .map_err(|e| e.to_string())?;
    Ok(SidebarCountsDto {
        assets,
        recent_viewed,
        on_this_day,
        tags: SMART_ALBUM_TAG_COUNT,
        albums,
    })
}

/// 侧栏一次性计数（snake_case 命令；无事件推送，前端按需拉取）。
#[tauri::command]
pub async fn sidebar_counts(state: State<'_, SharedState>) -> Result<SidebarCountsDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_sidebar_counts).await
}
