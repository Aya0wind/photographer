//! 地区索引回填：树入库（regions 表）+ GPS 资产增量挂接（asset_regions）+
//! 树缓存 JSON 导出 + 编辑联动单点重索引。
//!
//! 幂等策略：
//! - `regions` 表每次管线运行全量重刷（几百~几千行毫秒级；id 显式按
//!   Arena 索引+1 分配，内存 GeoIndex 与库 id 恒同构）
//! - `asset_regions` 只在数据包指纹变化时清空全量回填；否则 NOT IN 增量
//!   （新导入资产自动补，编辑联动单点覆盖）
//! - 缓存 `geo/regions-cache.json` 指纹不符即重建（前端直读，不走 IPC）

use std::path::Path;
use std::sync::Arc;

use crate::db::Db;
use crate::events::{AppEvent, EventBus};
use rusqlite::params;

use super::{geo_dir, geo_state, index_snapshot, GeoIndex, GeoPhase};

const FINGERPRINT_FILE: &str = "fingerprint.txt";
pub const CACHE_FILE: &str = "regions-cache.json";
const BATCH: i64 = 500;

/// 包就绪时的完整管线（下载完成 / 启动钩子调用）：load → 入库 → 回填 → 缓存。
/// cancel 观测在批次间（单批毫秒级，粒度足够）。
pub fn run_pipeline(db_dir: &Path, config_dir: &Path, bus: &EventBus, cancel: &dyn Fn() -> bool) {
    let dir = geo_dir(config_dir);
    {
        let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
        state.phase = GeoPhase::Loading;
    }
    bus.publish(AppEvent::MapGeoProgress {
        stage: "loading".into(),
        done: 0,
        total: 0,
        message: None,
    });

    let index = match GeoIndex::load(&dir) {
        Ok(index) => Arc::new(index),
        Err(err) => return set_failed(bus, err),
    };

    let db = match crate::ipc::open_library_db(db_dir) {
        Ok(db) => db,
        Err(err) => return set_failed(bus, format!("打开库失败: {err}")),
    };

    // 指纹对比：变化 → asset_regions 全清（连坐重刷）
    let fp_path = dir.join(FINGERPRINT_FILE);
    let old_fp = std::fs::read_to_string(&fp_path).unwrap_or_default();
    let index_fingerprint = index.fingerprint.clone();
    let schema_changed = old_fp != index_fingerprint;

    if let Err(err) = rewrite_regions(&db, &index, schema_changed) {
        return set_failed(bus, format!("regions 入库失败: {err}"));
    }
    std::fs::write(&fp_path, &index_fingerprint).ok();

    {
        let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
        state.index = Some(Arc::clone(&index));
    }

    if let Err(err) = run_backfill(&db, bus, &index, cancel) {
        return set_failed(bus, format!("回填失败: {err}"));
    }

    // 库真值导出树缓存（前端直读）
    export_cache(&db, &dir.join(CACHE_FILE)).ok();

    {
        let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
        state.phase = GeoPhase::Ready;
    }
    bus.publish(AppEvent::MapRegionsUpdated);
    bus.publish(AppEvent::MapGeoProgress {
        stage: "ready".into(),
        done: 0,
        total: 0,
        message: None,
    });
}

fn set_failed(bus: &EventBus, err: String) {
    let mut state = geo_state().lock().unwrap_or_else(|e| e.into_inner());
    state.phase = GeoPhase::Failed(err.clone());
    bus.publish(AppEvent::MapGeoProgress {
        stage: "failed".into(),
        done: 0,
        total: 0,
        message: Some(err),
    });
}

/// regions 表维护：指纹未变且行数同构（库未被重建）→ 跳过（保住增量挂接）；
/// 否则全清重插（先 asset_regions 后 regions，FK 删除顺序）。id 显式 =
/// Arena 索引+1（孤儿跳过留空洞），父先子后天然满足 push 序——内存树与库 id 恒同构。
fn rewrite_regions(db: &Db, index: &GeoIndex, schema_changed: bool) -> Result<(), String> {
    let reachable = index.reachable();
    let reachable_count = reachable.iter().filter(|r| **r).count() as i64;
    let row_count: i64 =
        db.0.query_row("SELECT COUNT(*) FROM regions", [], |r| r.get(0))
            .unwrap_or(-1);
    if !schema_changed && row_count == reachable_count {
        return Ok(()); // 数据包与库同构：跳过重刷，asset_regions 增量续跑
    }
    let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM asset_regions", [])
        .map_err(|e| e.to_string())?;
    tx.execute("DELETE FROM regions", [])
        .map_err(|e| e.to_string())?;
    // parent 关系一次预填（O(n·children)；比逐节点反查省一个数量级）
    let mut parent_of = vec![None::<usize>; index.nodes.len()];
    for (i, node) in index.nodes.iter().enumerate() {
        for &child in &node.children {
            parent_of[child] = Some(i);
        }
    }
    // 自引用 FK 逐行插入：parent 总在 child 之前（Arena push 序）；
    // 孤儿（DataV 替换后的 NE 中国省）不入库
    {
        let mut stmt = tx
            .prepare(
                "INSERT INTO regions(id, parent_id, level, name, code, lat, lon, source)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            )
            .map_err(|e| e.to_string())?;
        for (i, node) in index.nodes.iter().enumerate() {
            if !reachable[i] {
                continue;
            }
            let parent = parent_of[i].map(|p| p as i64 + 1);
            // 根节点 parent 为 NULL；找不到父（跨层孤儿）防御性也置 NULL
            stmt.execute(params![
                i as i64 + 1,
                parent,
                node.level,
                node.name,
                node.code,
                node.lat,
                node.lon,
                node.source
            ])
            .map_err(|e| e.to_string())?;
        }
    }
    tx.commit().map_err(|e| e.to_string())
}

/// 增量回填：GPS 非空且未挂接的资产分批 resolve 写行。
fn run_backfill(
    db: &Db,
    bus: &EventBus,
    index: &GeoIndex,
    cancel: &dyn Fn() -> bool,
) -> Result<(), String> {
    // 一次性捞全量待处理（GPS 非空且未挂接）：海上/未覆盖坐标 resolve 为空、
    // 写不进挂接表，若按「NOT IN 再查一轮」分批会永远选中它们（死循环）——
    // 内存分批天然收敛（10 万级资产 × 24B/行，内存无压力）。
    let rows: Vec<(i64, f64, f64)> = {
        let mut stmt =
            db.0.prepare(
                "SELECT id, gps_lat, gps_lon FROM assets
                 WHERE gps_lat IS NOT NULL AND gps_lon IS NOT NULL
                 AND id NOT IN (SELECT asset_id FROM asset_regions)",
            )
            .map_err(|e| e.to_string())?;
        let mapped = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, f64>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        mapped.filter_map(|r| r.ok()).collect()
    };
    let total = rows.len() as i64;
    if total == 0 {
        return Ok(());
    }
    let throttle = std::sync::Mutex::new(crate::events::Throttle::new(
        std::time::Duration::from_millis(400),
    ));
    let mut done: i64 = 0;
    for chunk in rows.chunks(BATCH as usize) {
        if cancel() {
            return Err("已取消".into());
        }
        let rows = chunk;
        let tx = db.0.unchecked_transaction().map_err(|e| e.to_string())?;
        {
            let mut stmt = tx
                .prepare(
                    "INSERT OR REPLACE INTO asset_regions(asset_id, region_id, level)
                     VALUES(?1,?2,?3)",
                )
                .map_err(|e| e.to_string())?;
            for (asset_id, lat, lon) in rows {
                for region in index.resolve(*lat, *lon) {
                    // db id = Arena 索引+1（rewrite_regions 保证同构）
                    stmt.execute(params![asset_id, region.node as i64 + 1, region.level])
                        .map_err(|e| e.to_string())?;
                }
            }
        }
        tx.commit().map_err(|e| e.to_string())?;
        done += rows.len() as i64;
        if throttle.lock().map(|mut t| t.should_fire()).unwrap_or(true) {
            bus.publish(AppEvent::MapGeoProgress {
                stage: "backfilling".into(),
                done: done as u64,
                total: total as u64,
                message: None,
            });
        }
    }
    Ok(())
}

/// 编辑联动单点重索引（exif/metadata GPS 修改后调用，毫秒级）。
/// 索引未就绪（包未装/加载中）时静默跳过返回 false——下次管线增量补。
pub fn reindex_asset(db: &Db, asset_id: i64, lat: Option<f64>, lon: Option<f64>) -> bool {
    let Some(index) = index_snapshot() else {
        return false;
    };
    match (lat, lon) {
        (Some(lat), Some(lon)) => {
            let tx = match db.0.unchecked_transaction() {
                Ok(tx) => tx,
                Err(_) => return false,
            };
            let _ = tx.execute(
                "DELETE FROM asset_regions WHERE asset_id=?1",
                params![asset_id],
            );
            if let Ok(mut stmt) = tx.prepare(
                "INSERT OR REPLACE INTO asset_regions(asset_id, region_id, level) VALUES(?1,?2,?3)",
            ) {
                for region in index.resolve(lat, lon) {
                    let _ = stmt.execute(params![asset_id, region.node as i64 + 1, region.level]);
                }
            }
            tx.commit().is_ok()
        }
        _ => {
            // GPS 清除：挂接全删
            db.0.execute(
                "DELETE FROM asset_regions WHERE asset_id=?1",
                params![asset_id],
            )
            .is_ok()
        }
    }
}

/// 库真值导出树缓存 JSON（嵌套 children；前端 map_geo_cache_url 直读）。
pub fn export_cache(db: &Db, path: &Path) -> Result<(), String> {
    #[derive(serde::Serialize)]
    struct Row {
        id: i64,
        parent: Option<i64>,
        level: u8,
        name: String,
        code: String,
        lat: f64,
        lon: f64,
    }
    let mut rows: Vec<Row> = Vec::new();
    let mut stmt = db
        .0
        .prepare("SELECT id, parent_id, level, name, COALESCE(code,''), lat, lon FROM regions ORDER BY id")
        .map_err(|e| e.to_string())?;
    let mapped = stmt
        .query_map([], |r| {
            Ok(Row {
                id: r.get(0)?,
                parent: r.get(1)?,
                level: r.get::<_, i64>(2)? as u8,
                name: r.get(3)?,
                code: r.get(4)?,
                lat: r.get(5)?,
                lon: r.get(6)?,
            })
        })
        .map_err(|e| e.to_string())?;
    for row in mapped.filter_map(|r| r.ok()) {
        rows.push(row);
    }
    let tmp = path.with_extension("part");
    let json = serde_json::to_vec(&rows).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, json).map_err(|e| format!("写缓存失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("缓存落盘失败: {e}"))
}
