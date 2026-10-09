//! 拍摄地图 IPC：数据包状态/安装触发/树缓存路径 + 分层聚合气泡查询。
//!
//! 聚合形态（map-module.md 定案）：`level` 选层（0 国 /1 省 /2 市 /3 县），
//! `parent_region_id` 可选限定下钻范围（该节子的子节点集合）。每组带随机
//! N 张代表图（ORDER BY random() LIMIT；组行数大时先 COUNT 再随机 offset——
//! v1 直接 random()，压测不达标再换）。

use serde::Serialize;
use tauri::{Manager, State};

use crate::geo::{self, backfill, GeoPhase};
use crate::ipc::{active_library_db, run_blocking, SharedState};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GeoStatusDto {
    /// 数据包三根文件是否就绪
    pub installed: bool,
    /// notInstalled / loading / ready / backfilling / failed
    pub phase: String,
    pub done: u64,
    pub total: u64,
    pub message: Option<String>,
    /// 树缓存 JSON 是否可读（前端直读提速）
    pub cache_ready: bool,
    /// DataV 市县文件已装数（<250 = 上次中断可补全）
    pub datav_files: usize,
}

fn phase_parts(phase: &GeoPhase) -> (String, u64, u64, Option<String>) {
    match phase {
        GeoPhase::NotInstalled => ("notInstalled".into(), 0, 0, None),
        GeoPhase::Loading => ("loading".into(), 0, 0, None),
        GeoPhase::Ready => ("ready".into(), 0, 0, None),
        GeoPhase::Backfilling { done, total } => ("backfilling".into(), *done, *total, None),
        GeoPhase::Failed(msg) => ("failed".into(), 0, 0, Some(msg.clone())),
    }
}

#[tauri::command]
pub fn map_geo_status(state: State<'_, SharedState>) -> GeoStatusDto {
    let config_dir = state.config_dir.clone();
    let installed = geo::packages_installed(&geo::geo_dir(&config_dir));
    let cache_ready = geo::geo_dir(&config_dir)
        .join(backfill::CACHE_FILE)
        .is_file();
    let datav_files = geo::datav_count(&geo::geo_dir(&config_dir));
    let (phase, done, total, message) = phase_parts(&geo::phase_snapshot());
    GeoStatusDto {
        installed,
        phase,
        done,
        total,
        message,
        cache_ready,
        datav_files,
    }
}

/// 安装内置地理数据（幂等）：未解压则后台解压一次（config_dir/geo，
/// 常驻磁盘）→ 自动接力回填管线。已在跑/已就位时为快路径。
#[tauri::command]
pub fn map_geo_install(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<SharedState>();
    let db_dir = {
        let settings = state
            .settings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        settings
            .active_library()
            .map(|l| std::path::PathBuf::from(&l.db_dir))
    };
    let Some(db_dir) = db_dir else {
        return Err("尚未创建库".into());
    };
    let bus = std::sync::Arc::new(state.bus.clone());
    geo::install::ensure_installed(state.config_dir.clone(), db_dir, bus, &state.supervisor);
    Ok(())
}

/// 树缓存文件绝对路径（前端 convertFileSrc 直读；未就绪返回 None）。
#[tauri::command]
pub fn map_geo_cache_url(state: State<'_, SharedState>) -> Option<String> {
    let path = geo::geo_dir(&state.config_dir).join(backfill::CACHE_FILE);
    path.is_file().then(|| path.to_string_lossy().into_owned())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterSample {
    pub id: i64,
    pub path: String,
    pub kind: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterDto {
    pub region_id: i64,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub count: i64,
    pub samples: Vec<ClusterSample>,
}

/// 每组随机代表图张数（气泡卡一排 3 张视觉平衡）。
const SAMPLES_PER_CLUSTER: i64 = 3;
/// 同屏气泡上限（防极端分布刷出海量 marker）。
const MAX_CLUSTERS: i64 = 400;

#[tauri::command]
pub async fn map_clusters(
    state: State<'_, SharedState>,
    level: u8,
    parent_region_id: Option<i64>,
) -> Result<Vec<ClusterDto>, String> {
    run_blocking(state.inner().clone(), move |state| {
        let db = active_library_db(state)?;
        clusters(&db, level, parent_region_id)
    })
    .await
}

pub fn clusters(
    db: &crate::db::Db,
    level: u8,
    parent: Option<i64>,
) -> Result<Vec<ClusterDto>, String> {
    if level > 3 {
        return Err("层级越界（0-3）".into());
    }
    // 分组计数（parent 限定 = 子节点集合下钻）
    let mut groups: Vec<(i64, i64)> = Vec::new();
    {
        let sql = match parent {
            Some(_) => "SELECT ar.region_id, COUNT(*) FROM asset_regions ar
                        WHERE ar.level=?1 AND ar.region_id IN (SELECT id FROM regions WHERE parent_id=?2)
                        GROUP BY ar.region_id ORDER BY COUNT(*) DESC LIMIT ?3",
            None => "SELECT ar.region_id, COUNT(*) FROM asset_regions ar
                     WHERE ar.level=?1 GROUP BY ar.region_id ORDER BY COUNT(*) DESC LIMIT ?2",
        };
        let mut stmt = db.0.prepare(sql).map_err(|e| e.to_string())?;
        let args: Vec<&dyn rusqlite::ToSql> = match parent.as_ref() {
            Some(p) => vec![&level as &dyn rusqlite::ToSql, p, &MAX_CLUSTERS],
            None => vec![&level as &dyn rusqlite::ToSql, &MAX_CLUSTERS],
        };
        let mapped = stmt
            .query_map(rusqlite::params_from_iter(args), |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })
            .map_err(|e| e.to_string())?;
        for row in mapped.filter_map(|r| r.ok()) {
            groups.push(row);
        }
    }
    // 地区元数据 + 随机样本
    let mut out = Vec::with_capacity(groups.len());
    for (region_id, count) in groups {
        let (name, lat, lon): (String, f64, f64) =
            db.0.query_row(
                "SELECT name, lat, lon FROM regions WHERE id=?1",
                [region_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(|e| e.to_string())?;
        let mut samples = Vec::new();
        {
            let mut stmt =
                db.0.prepare(
                    "SELECT a.id, a.path, a.kind FROM assets a
                     JOIN asset_regions ar ON ar.asset_id=a.id AND ar.region_id=?1
                     ORDER BY random() LIMIT ?2",
                )
                .map_err(|e| e.to_string())?;
            let rows = stmt
                .query_map([region_id, SAMPLES_PER_CLUSTER], |r| {
                    Ok(ClusterSample {
                        id: r.get(0)?,
                        path: r.get(1)?,
                        kind: r.get(2)?,
                    })
                })
                .map_err(|e| e.to_string())?;
            for row in rows.filter_map(|r| r.ok()) {
                samples.push(row);
            }
        }
        out.push(ClusterDto {
            region_id,
            name,
            lat,
            lon,
            count,
            samples,
        });
    }
    Ok(out)
}
