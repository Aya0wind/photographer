//! assets 命令（M3 画廊数据源）：keyset 分页 / 日期分组 / 资产详情 /
//! 相机聚合。
//!
//! 排序契约（真机核对 2026-09-19）：NULL captured_at 最先，随后拍摄时间
//! 降序、id 倒序 tiebreak——db 层用 COALESCE 高哨兵归一成单键。DB 查询
//! 走 run_blocking 后台线程（铁律：大结果集不上主线程）。

use tauri::State;

use super::{run_blocking, SharedState};
pub use crate::db::AssetFilters;
use crate::events::AssetKind;

/// 画廊网格条目 DTO（camelCase）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetDto {
    pub id: i64,
    /// 绝对路径。
    pub path: String,
    /// 文件名。
    pub name: String,
    /// photo | raw | video | other（`AssetKind` camelCase 序列化）。
    pub kind: AssetKind,
    /// RFC3339；未知为 null（排序时排最前）。
    pub captured_at: Option<String>,
    pub camera: Option<String>,
    pub size_bytes: u64,
    // —— 拍摄参数（0004 起新导入有值，存量全 null）——
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub iso: Option<u32>,
    pub f_number: Option<String>,
    pub exposure_time: Option<String>,
    pub focal_length: Option<String>,
    pub lens: Option<String>,
    /// RAW/JPG 配对资产 id（无配对 null）。
    pub pair_id: Option<i64>,
    /// 缩略图状态 0 pending / 1 done / 2 permanent-none。
    pub thumb_state: i32,
}

/// 日期分组 DTO（画廊吸顶 + 跳转；date 为本地时区 `YYYY-MM-DD`，NULL 归
/// "unknown" 且置顶）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateGroupDto {
    pub date: String,
    pub count: u64,
    pub cover_asset_id: i64,
}

/// 资产详情 DTO：AssetRow 全字段（flatten）+ id + 库内同指纹重复计数
/// （(size, xxh) 相同的**其他**资产数，不含自身）+ M5 计算字段
/// （format/megapixels/aspect/pairId 显式别名，flatten 内同值字段为
/// pairAssetId——两者并存，前端按新契约取 pairId）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AssetDetailDto {
    pub id: i64,
    #[serde(flatten)]
    pub asset: crate::db::AssetRow,
    pub duplicate_count: u64,
    /// 文件格式（路径扩展名大写，如 "NEF"；无扩展名 null）。
    pub format: Option<String>,
    /// 百万像素（保留 1 位小数，如 24.2）。
    pub megapixels: Option<f64>,
    /// 宽高比：常见比归约（"3:2"/"16:9"…），否则 "W:H" 小数（"1.37:1"）。
    /// orientation 5-8（含 90° 旋转）时按显示方向取比。
    pub aspect: Option<String>,
    /// RAW/JPG 配对资产 id（无配对 null；flatten 内 pairAssetId 同值）。
    pub pair_id: Option<i64>,
}

/// 扩展名（最后一个点后的部分大写；无点 → None）。
fn format_of(path: &str) -> Option<String> {
    let ext = path.rsplit(['/', '\\']).next()?;
    let (_, ext) = ext.rsplit_once('.')?;
    (!ext.is_empty()).then(|| ext.to_ascii_uppercase())
}

/// 百万像素（1 位小数）。
fn megapixels_of(width: Option<u32>, height: Option<u32>) -> Option<f64> {
    let w = width? as f64;
    let h = height? as f64;
    Some(((w * h / 1_000_000.0) * 10.0).round() / 10.0)
}

/// 宽高比字符串：常见比（±1% 容差）归约为标准 token；否则整数比约分
/// （分母 ≤ 50）或 "x.xx:1" 小数比。orientation 5-8 交换宽高（显示方向）。
fn aspect_of(width: Option<u32>, height: Option<u32>, orientation: Option<i64>) -> Option<String> {
    let (mut w, mut h) = (width? as f64, height? as f64);
    if matches!(orientation, Some(5..=8)) {
        std::mem::swap(&mut w, &mut h);
    }
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    const COMMON: &[(&str, f64)] = &[
        ("1:1", 1.0),
        ("5:4", 5.0 / 4.0),
        ("4:3", 4.0 / 3.0),
        ("3:2", 3.0 / 2.0),
        ("16:10", 16.0 / 10.0),
        ("16:9", 16.0 / 9.0),
        ("21:9", 21.0 / 9.0),
        ("2:3", 2.0 / 3.0),
        ("3:4", 3.0 / 4.0),
        ("9:16", 9.0 / 16.0),
    ];
    let ratio = w / h;
    if let Some((token, _)) = COMMON.iter().find(|(_, r)| (ratio - r).abs() / r <= 0.01) {
        return Some(token.to_string());
    }
    // 整数比约分（gcd），分母不大时人类可读
    let (iw, ih) = (w.round() as u64, h.round() as u64);
    if iw > 0 && ih > 0 {
        let gcd = gcd(iw, ih);
        let (rw, rh) = (iw / gcd, ih / gcd);
        if rh <= 50 && rw <= 200 {
            return Some(format!("{rw}:{rh}"));
        }
    }
    // 兜底：以高为 1 的小数比（去尾零）
    let mut text = format!("{ratio:.2}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    Some(format!("{text}:1"))
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 {
        a
    } else {
        gcd(b, a % b)
    }
}

/// AssetPageRow → AssetDto（画廊/最近添加共用映射）。
pub fn page_row_to_dto(r: crate::db::AssetPageRow) -> AssetDto {
    AssetDto {
        id: r.id,
        path: r.path,
        name: r.filename,
        kind: r.kind,
        captured_at: r.captured_at,
        camera: r.camera,
        size_bytes: r.size,
        width: r.width,
        height: r.height,
        iso: r.iso,
        f_number: r.f_number,
        exposure_time: r.exposure_time,
        focal_length: r.focal_length,
        lens: r.lens,
        pair_id: r.pair_id,
        thumb_state: r.thumb_state,
    }
}

/// 日期过滤值归一：转与库内 captured_at 同构的定宽 UTC 字符串
/// （库内为 `to_rfc3339_opts(Millis, true)`；带时区偏移的入参转 UTC 瞬时）。
/// **纯日期 `YYYY-MM-DD`**：按本地时区解释——起始（after）= 当日
/// 00:00:00.000，结束（before）= 当日 23:59:59.999（含当日全 天）。
fn normalize_date_filter(value: &str, field: &str, end_of_day: bool) -> Result<String, String> {
    let value = value.trim();
    if let Ok(date) = chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d") {
        use chrono::TimeZone;
        let naive = if end_of_day {
            date.and_hms_micro_opt(23, 59, 59, 999_000)
        } else {
            date.and_hms_micro_opt(0, 0, 0, 0)
        }
        .ok_or_else(|| format!("无效的{field}日期过滤: {value}"))?;
        let dt = chrono::Local
            .from_local_datetime(&naive)
            .earliest()
            .ok_or_else(|| format!("无效的{field}日期过滤: {value}"))?;
        // 关键：转 UTC 定宽（保留 +08:00 偏移会让字典序比较错位）
        return Ok(dt
            .with_timezone(&chrono::Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    }
    chrono::DateTime::parse_from_rfc3339(value)
        .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .map_err(|_| format!("无效的{field}日期过滤（需 RFC3339 或 YYYY-MM-DD）: {value}"))
}

/// keyset 分页：after_id = 上一页末行 id（0 = 第一页；行已删按第一页）。
/// 过滤条件全参数化；任一日期过滤出现时 NULL captured_at 被排除。
pub fn fetch_assets_page(
    state: &super::AppState,
    after_id: i64,
    limit: u32,
    filters: AssetFilters,
) -> Result<Vec<AssetDto>, String> {
    let mut filters = filters;
    if let Some(after) = filters.captured_after.take() {
        filters.captured_after = Some(normalize_date_filter(&after, "起始", false)?);
    }
    if let Some(before) = filters.captured_before.take() {
        filters.captured_before = Some(normalize_date_filter(&before, "结束", true)?);
    }
    let db = super::active_library_db(state)?;
    let rows = db
        .assets_page(after_id, limit.clamp(1, 200), &filters)
        .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().map(page_row_to_dto).collect())
}

/// 本地时区日期分组（降序；unknown 组置顶）。
pub fn fetch_asset_group_dates(state: &super::AppState) -> Result<Vec<DateGroupDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.asset_group_dates().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| DateGroupDto {
            date: r.date,
            count: r.count,
            cover_asset_id: r.cover_asset_id,
        })
        .collect())
}

/// 资产详情：全字段 + 同指纹重复计数 + 计算字段；不存在返回 None。
pub fn fetch_asset_detail(
    state: &super::AppState,
    id: i64,
) -> Result<Option<AssetDetailDto>, String> {
    let db = super::active_library_db(state)?;
    let Some(asset) = db.asset_by_id(id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    let duplicate_count = db
        .asset_duplicate_count(id, asset.size, asset.xxhash)
        .map_err(|e| e.to_string())?;
    Ok(Some(AssetDetailDto {
        id,
        format: format_of(&asset.path),
        megapixels: megapixels_of(asset.width, asset.height),
        aspect: aspect_of(asset.width, asset.height, asset.orientation),
        pair_id: asset.pair_asset_id,
        duplicate_count,
        asset,
    }))
}

/// 画廊分页（DB 查询 → 后台线程）。
/// 资产分页（画廊/搜索数据源）。`filters` 缺省 = 全部
/// （Tauri 对 Option 参数允许缺键；前端不传 filters 或传 null 均可）。
#[tauri::command]
pub async fn assets_page(
    state: State<'_, SharedState>,
    after_id: i64,
    limit: u32,
    filters: Option<AssetFilters>,
) -> Result<Vec<AssetDto>, String> {
    let filters = filters.unwrap_or_default();
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_assets_page(state, after_id, limit, filters)
    })
    .await
}

/// 日期分组（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn asset_group_dates(state: State<'_, SharedState>) -> Result<Vec<DateGroupDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_asset_group_dates).await
}

/// 资产详情（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn asset_detail(
    state: State<'_, SharedState>,
    id: i64,
) -> Result<Option<AssetDetailDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_asset_detail(state, id)).await
}

/// 相机聚合 DTO（搜索页相机勾选）。
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCountDto {
    pub camera: String,
    pub count: u64,
}

/// 相机聚合（camera 非空分组计数，count 降序；搜索页勾选数据源）。
pub fn fetch_camera_list(state: &super::AppState) -> Result<Vec<CameraCountDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.camera_list().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| CameraCountDto {
            camera: r.camera,
            count: r.count,
        })
        .collect())
}

/// 相机聚合（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn camera_list(state: State<'_, SharedState>) -> Result<Vec<CameraCountDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_camera_list).await
}

/// 镜头聚合（lens 非空分组计数降序；搜索页镜头勾选数据源）。
pub fn fetch_lens_list(state: &super::AppState) -> Result<Vec<LensCountDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.lens_list().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| LensCountDto {
            lens: r.camera,
            count: r.count,
        })
        .collect())
}

/// 镜头聚合 DTO（camelCase；与前端 AssetLensCount 契约对齐——此前复用
/// CameraCountDto 的 `camera` 字段承载镜头名，前端读 `lens` 全 undefined，
/// 真机 2026-09-20 验收抓出）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LensCountDto {
    pub lens: String,
    pub count: u64,
}

/// 格式聚合 DTO（同上，`format` 承载扩展名大写）。
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatCountDto {
    pub format: String,
    pub count: u64,
}

/// 镜头聚合（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn lens_list(state: State<'_, SharedState>) -> Result<Vec<LensCountDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_lens_list).await
}

/// 格式聚合（路径扩展名大写分组计数降序；搜索页格式勾选数据源）。
/// `camera` 字段承载格式名（与 camera_list 同构载荷，前端复用同一组件）。
pub fn fetch_format_list(state: &super::AppState) -> Result<Vec<FormatCountDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db.format_list().map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| FormatCountDto {
            format: r.camera,
            count: r.count,
        })
        .collect())
}

/// 格式聚合（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn format_list(state: State<'_, SharedState>) -> Result<Vec<FormatCountDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_format_list).await
}

/// 按 id 批量取资产（语义检索命中→画廊瓦片解析；保持入参顺序，失效 id 跳过）。
pub fn fetch_assets_by_ids(state: &super::AppState, ids: &[i64]) -> Result<Vec<AssetDto>, String> {
    let db = super::active_library_db(state)?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids.iter().take(200) {
        if let Some(asset) = db.asset_by_id(*id).map_err(|e| e.to_string())? {
            out.push(AssetDto {
                id: *id,
                path: asset.path,
                name: asset.filename,
                kind: asset.kind,
                captured_at: asset.captured_at,
                camera: asset.camera,
                size_bytes: asset.size,
                width: asset.width,
                height: asset.height,
                iso: asset.iso,
                f_number: asset.f_number,
                exposure_time: asset.exposure_time,
                focal_length: asset.focal_length,
                lens: asset.lens,
                pair_id: asset.pair_asset_id,
                thumb_state: asset.thumb_state,
            });
        }
    }
    Ok(out)
}

/// 按 id 批量取资产（DB 查询 → 后台线程）。
#[tauri::command]
pub async fn assets_by_ids(
    state: State<'_, SharedState>,
    ids: Vec<i64>,
) -> Result<Vec<AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_assets_by_ids(state, &ids)).await
}
