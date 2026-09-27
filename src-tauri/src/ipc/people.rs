//! people 命令（M4 人物页数据）：人物簇列表 / 簇内资产 / 重命名 / 删除。
//!
//! 全部走 active_library_db + run_blocking（铁律：DB 查询不上主线程）；
//! 载荷 camelCase。人物数据由 face 通道在线聚类产出（crate::ai::face）。

use tauri::State;

use super::{run_blocking, SharedState};
use crate::db::PersonRow;

/// 人物簇列表（face_count 降序；名称 NULL = 未命名；空簇不返回）。
pub fn fetch_people_list(state: &super::AppState) -> Result<Vec<PersonRow>, String> {
    let db = super::active_library_db(state)?;
    db.people_list().map_err(|e| e.to_string())
}

/// 某人物簇的资产页（去重，captured_at DESC；画廊同款排序契约）。
pub fn fetch_people_assets(
    state: &super::AppState,
    cluster_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let db = super::active_library_db(state)?;
    let rows = db
        .assets_by_cluster(cluster_id, limit.clamp(1, 200))
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|r| super::assets::AssetDto {
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
            burst_id: r.burst_id,
            burst_count: None,
            flagged: r.flagged,
            rating: r.rating,
            color_label: r.color_label,
            rejected: r.rejected,
        })
        .collect())
}

/// 人物重命名（trim；空串 = 清回未命名）。
pub fn fetch_person_rename(
    state: &super::AppState,
    cluster_id: i64,
    name: &str,
) -> Result<(), String> {
    let db = super::active_library_db(state)?;
    db.rename_person(cluster_id, name)
        .map_err(|e| e.to_string())
}

/// 删除人物簇（只解除归属，faces 数据保留）。
pub fn fetch_person_delete(state: &super::AppState, cluster_id: i64) -> Result<(), String> {
    let db = super::active_library_db(state)?;
    db.delete_person(cluster_id).map_err(|e| e.to_string())
}

/// 人物簇列表。
#[tauri::command]
pub async fn people_list(state: State<'_, SharedState>) -> Result<Vec<PersonRow>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_people_list).await
}

/// 某人物簇的资产页（limit 上限 200）。
#[tauri::command]
pub async fn people_assets(
    state: State<'_, SharedState>,
    cluster_id: i64,
    limit: u32,
) -> Result<Vec<super::assets::AssetDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_people_assets(state, cluster_id, limit)
    })
    .await
}

/// 人物重命名。
#[tauri::command]
pub async fn person_rename(
    state: State<'_, SharedState>,
    cluster_id: i64,
    name: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_person_rename(state, cluster_id, &name)
    })
    .await
}

/// 删除人物簇（人脸数据保留，可重新聚类找回）。
#[tauri::command]
pub async fn person_delete(state: State<'_, SharedState>, cluster_id: i64) -> Result<(), String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| fetch_person_delete(state, cluster_id)).await
}
