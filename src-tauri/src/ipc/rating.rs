//! 评分 / 收藏 / 最近添加命令（M5）：评分入库 + XMP 边车异步同步（LR 互通）。
//!
//! 铁律：评分写库快（单行 UPDATE）在 run_blocking 内完成；XMP 边车是磁盘
//! IO → supervisor 后台线程，**失败不阻塞评分入库**（AppError 事件上报）。
//! 仅导入时不会写源目录；用户主动修改评分后，外部照片也同步 XMP 边车。
//! XMP → DB 的反向回填在 exif 索引通道
//! （index::process_exif_task，只读边车不回写，无循环）。

use std::path::PathBuf;

use tauri::State;

use super::sidecar::{spawn_xmp_sync, XmpSync};
use super::{run_blocking, SharedState};

/// XMP 评分投影（真值定义在 metadata::xmp，与库扫描边车补写共用）。
pub fn projected_rating(rating: i64, rejected: bool) -> i8 {
    crate::metadata::xmp::projected_rating(rating, rejected)
}

/// 评分写入核：0-5 校验 → DB 更新（同语句置 `xmp_dirty`，§五 M2c 写方向
/// 闭环）→ 按投影（含拒绝态）派 XMP 边车同步（成功后清脏）。
/// 资产缺失 / 所在照片库离线 → 照常入库、保持脏标志、不派边车任务
/// （库恢复在线后由库扫描补写，[`crate::scan`]）。资产不存在 / 评分越界
/// 返回明确错误。
pub fn fetch_asset_rating_set(
    state: &super::AppState,
    asset_id: i64,
    rating: i64,
) -> Result<(), String> {
    if !(0..=5).contains(&rating) {
        return Err(format!("评分必须在 0-5：{rating}"));
    }
    let (path, rejected, missing, library_offline) = {
        let db = super::app_database_db(state)?;
        db.0.query_row(
            "SELECT a.path, a.rejected != 0, a.missing != 0, \
             COALESCE((SELECT l.status = 'offline' FROM photos_libraries l \
              WHERE l.id = a.library_id), 0) != 0 \
             FROM assets a WHERE a.id = ?1",
            [asset_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )
        .map_err(|e| e.to_string())?
    };
    let db = super::app_database_db(state)?;
    let ok = db
        .set_asset_rating_mark_dirty(asset_id, rating)
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err(format!("资产 {asset_id} 不存在"));
    }
    // 离线/缺失期间：入库 + 置脏即止——边车补写交给库恢复在线后的扫描。
    if missing || library_offline {
        return Ok(());
    }

    // 用户主动评分时，即使原文件是外部引用，也同步旁边的 XMP 边车。
    // 成功后清脏（xmp_dirty 归零）；失败保持脏 → 下轮库扫描兜底补写。
    let projected = projected_rating(rating, rejected);
    let db_dir = super::app_database_dir(state)?;
    spawn_xmp_sync(
        state,
        XmpSync {
            task: format!("rating-{asset_id}"),
            subject: "评分",
            failure: "同步失败",
        },
        vec![(path, projected)],
        move |(path, projected)| {
            crate::metadata::xmp::sync_rating_to_sidecar(&PathBuf::from(path), projected)?;
            if let Ok(db) = super::open_library_db(&db_dir) {
                let _ = db.clear_asset_xmp_dirty(asset_id);
            }
            Ok(())
        },
    );
    Ok(())
}

/// 收藏旗标写入核（旗标是 Smart Photo 侧标记，XMP 无对应标准字段——LR 的
/// flag 是会话态，v1 只写库）。
pub fn fetch_asset_flag_set(
    state: &super::AppState,
    asset_id: i64,
    flagged: bool,
) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    let ok = db
        .set_asset_flagged(asset_id, flagged)
        .map_err(|e| e.to_string())?;
    if !ok {
        return Err(format!("资产 {asset_id} 不存在"));
    }
    Ok(())
}

/// 最近添加分页核（created_at DESC keyset；AssetDto 同画廊契约）。
pub fn fetch_recent_assets(
    state: &super::AppState,
    after_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::app_database_db(state)?;
    let rows = db
        .recent_assets_page(after_id, limit.clamp(1, 200))
        .map_err(|e| e.to_string())?;
    let mut dtos: Vec<_> = rows
        .into_iter()
        .map(super::assets::page_row_to_dto)
        .collect();
    super::assets::attach_burst_counts_pub(&db, &mut dtos);
    Ok(dtos)
}

/// 设置评分（0-5；0 = 清除）。成功后 XMP 边车后台同步。
#[tauri::command]
pub async fn asset_rating_set(
    state: State<'_, SharedState>,
    asset_id: i64,
    rating: i64,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_asset_rating_set(state, asset_id, rating)
    })
    .await
}

/// 设置收藏旗标。
#[tauri::command]
pub async fn asset_flag_set(
    state: State<'_, SharedState>,
    asset_id: i64,
    flagged: bool,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_asset_flag_set(state, asset_id, flagged)
    })
    .await
}

/// 最近添加分页（after_id = 上一页末行 id，0 = 第一页）。
#[tauri::command]
pub async fn recent_assets(
    state: State<'_, SharedState>,
    after_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_recent_assets(state, after_id, limit)
    })
    .await
}

/// 浏览记账核（查看器打开照片时调用；无效资产静默 Ok）。
pub fn fetch_asset_view_mark(state: &super::AppState, asset_id: i64) -> Result<(), String> {
    let db = super::app_database_db(state)?;
    // 资产不存在 → 静默（mark 对无效 id 不构成用户可见错误）
    let exists: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE id = ?1",
            [asset_id],
            |r| r.get(0),
        )
        .map_err(|e| e.to_string())?;
    if exists == 0 {
        return Ok(());
    }
    db.mark_asset_viewed(asset_id).map_err(|e| e.to_string())
}

/// 最近浏览分页核（viewed_at DESC；每资产一行天然去重）。
pub fn fetch_recent_viewed(
    state: &super::AppState,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::app_database_db(state)?;
    let rows = db
        .recently_viewed(limit.clamp(1, 200))
        .map_err(|e| e.to_string())?;
    let mut dtos: Vec<_> = rows
        .into_iter()
        .map(super::assets::page_row_to_dto)
        .collect();
    super::assets::attach_burst_counts_pub(&db, &mut dtos);
    Ok(dtos)
}

/// 浏览记账（查看器打开照片时调用；幂等刷新时间）。
#[tauri::command]
pub async fn asset_view_mark(state: State<'_, SharedState>, asset_id: i64) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_asset_view_mark(state, asset_id)).await
}

/// 最近浏览列表（「最近浏览」页数据源；limit 上限 200）。
#[tauri::command]
pub async fn recent_viewed(
    state: State<'_, SharedState>,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_recent_viewed(state, limit)).await
}
