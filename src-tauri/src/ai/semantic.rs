//! 语义向量索引（M4）：usearch HNSW（768 维 cos 距离）+ 库级懒加载 +
//! 索引任务 ai 通道处理 + 语义回填。
//!
//! 一致性策略（用户定案）：**usearch 为向量真值**，SQLite `assets.ai_indexed_at`
//! 只记嵌入时间账：backfill 只为 `ai_indexed_at IS NULL` 的资产建任务；
//! usearch add 对已有 key 为覆盖语义，崩溃重放不产生重复向量。
//!
//! 文件：`dbDir/vectors.usearch`；库切换懒加载（全局池按 dbDir 缓存，
//! cxx 绑定内部自带并发控制，外层 Mutex 串行化 add/save）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use usearch::{Index, IndexOptions, MetricKind, ScalarKind};

use crate::db::Db;
use crate::events::{AppEvent, EventBus};
use crate::tasks::TaskSupervisor;

use super::embed::{SemanticEmbedder, EMBED_DIM};
use super::ModelManager;

/// 索引文件名（落各库 dbDir）。
const VECTORS_FILE: &str = "vectors.usearch";
/// 容量预留步长（reserve 一次性预留，减少扩容重分配）。
const RESERVE_CHUNK: usize = 4096;

/// 全局索引池：dbDir → 共享索引（库切换懒加载；Arc 供多 worker 并发查询）。
fn pool() -> &'static Mutex<HashMap<PathBuf, Arc<Mutex<Index>>>> {
    static POOL: std::sync::OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Index>>>>> =
        std::sync::OnceLock::new();
    POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 规范化库根（池键；canonicalize 失败退回原样）。
fn pool_key(db_dir: &Path) -> PathBuf {
    std::fs::canonicalize(db_dir).unwrap_or_else(|_| db_dir.to_path_buf())
}

fn vectors_path(db_dir: &Path) -> PathBuf {
    db_dir.join(VECTORS_FILE)
}

/// 取（或懒加载）某库的共享 HNSW 索引。
pub fn shared_index(db_dir: &Path) -> Result<Arc<Mutex<Index>>, String> {
    let key = pool_key(db_dir);
    let mut pool = pool().lock().expect("semantic pool mutex poisoned");
    if let Some(idx) = pool.get(&key) {
        return Ok(Arc::clone(idx));
    }
    let path = vectors_path(db_dir);
    let index = if path.is_file() {
        // 已有索引文件：view（mmap 零拷贝加载）
        let idx = new_index()?;
        idx.view(path.to_str().ok_or("库路径含非 UTF-8 字符")?)
            .map_err(|e| e.to_string())?;
        idx
    } else {
        new_index()?
    };
    let shared = Arc::new(Mutex::new(index));
    pool.insert(key.clone(), Arc::clone(&shared));
    Ok(Arc::clone(pool.get(&key).expect("just inserted")))
}

fn new_index() -> Result<Index, String> {
    let idx = Index::new(&IndexOptions {
        dimensions: EMBED_DIM,
        metric: MetricKind::Cos,
        quantization: ScalarKind::F32,
        ..Default::default()
    })
    .map_err(|e| e.to_string())?;
    idx.reserve(RESERVE_CHUNK).map_err(|e| e.to_string())?;
    Ok(idx)
}

/// 容量保障：超限时 reserve 扩容。
fn ensure_capacity(index: &mut Index, needed: usize) -> Result<(), String> {
    if needed > index.capacity() {
        let target = index.capacity() + RESERVE_CHUNK.max(needed - index.capacity());
        index.reserve(target).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 单条 ai 任务处理：256 档缩略图 → embed → usearch 插入 → 落盘 → 记账。
/// 返回成功与否（失败走 attempts 封顶策略）。
fn process_ai_task(db: &Db, db_dir: &Path, embedder: &dyn SemanticEmbedder, asset_id: i64) -> bool {
    let Some((path, _thumb_state)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return false; // 资产已删除（级联清任务前的防御兜底）
    };
    // 256 档缩略图（缺失则顺手生成；这同时是 embed 的输入）
    let Some(_) = crate::thumbs::thumb_file(db_dir, Path::new(&path), 256) else {
        return false;
    };
    let vector = match embedder.embed_image(Path::new(&path), db_dir) {
        Ok(v) => v,
        Err(_) => return false,
    };
    let index = match shared_index(db_dir) {
        Ok(i) => i,
        Err(_) => return false,
    };
    let mut idx = index.lock().expect("semantic index mutex poisoned");
    let needed = idx.size() + 1;
    if ensure_capacity(&mut idx, needed).is_err() {
        return false;
    }
    if idx.add(asset_id as u64, &vector).is_err() {
        return false;
    }
    // 每条落盘（防崩溃丢账；调用方为批量时由 worker 循环天然降频）
    let path = vectors_path(db_dir);
    let Some(path_str) = path.to_str() else {
        return false;
    };
    if idx.save(path_str).is_err() {
        return false;
    }
    drop(idx);
    db.set_ai_indexed(asset_id).is_ok()
}

/// 语义回填 + 全核执行（模型齐备 && enable_clip 时由调用方触发）：
/// ① 为 `ai_indexed_at IS NULL` 且没有未完成 ai 任务的库内照片建任务
/// ② 全核 worker 跑 ai 通道，逐条发布 indexTaskProgress{kind:"ai"}。
/// 返回本轮成功嵌入数。
pub fn run_semantic_backfill(
    db_dir: &Path,
    embedder: Arc<dyn SemanticEmbedder>,
    bus: &EventBus,
    workers: usize,
) -> u64 {
    let Ok(db) = crate::ipc::open_library_db(db_dir) else {
        return 0;
    };
    let total = db.create_ai_tasks_for_unindexed().unwrap_or(0);
    if total == 0 {
        return 0;
    }
    let mut done = 0u64;
    let mut handles = Vec::new();
    for n in 0..workers.max(1) {
        let db_dir = db_dir.to_path_buf();
        let bus = bus.clone();
        let embedder = Arc::clone(&embedder);
        handles.push(
            std::thread::Builder::new()
                .name(format!("index-ai-{n}"))
                .spawn(move || -> u64 {
                    let Ok(db) = crate::ipc::open_library_db(&db_dir) else {
                        return 0;
                    };
                    let mut count = 0u64;
                    loop {
                        let task = match db.claim_index_task("ai") {
                            Ok(Some(t)) => t,
                            Ok(None) | Err(_) => break,
                        };
                        let ok = process_ai_task(&db, &db_dir, &*embedder, task.asset_id);
                        let _ = db.finish_index_task(task.id, ok);
                        count += u64::from(ok);
                        bus.publish(AppEvent::IndexTaskProgress {
                            kind: "ai".into(),
                            done: count,
                            total,
                        });
                    }
                    count
                })
                .expect("spawn ai index worker"),
        );
    }
    for h in handles {
        if let Ok(n) = h.join() {
            done += n;
        }
    }
    done
}

/// 便利入口：导入钩子/启动恢复/模型装好后的统一触发。门槛：模型三件套
/// 齐备（未齐静默跳过——模型下载完成钩子会再触发）。
pub fn kick_semantic_if_ready(
    db_dir: PathBuf,
    manager: &ModelManager,
    bus: &EventBus,
    supervisor: &std::sync::Arc<TaskSupervisor>,
) {
    if !manager.semantic_ready() {
        return;
    }
    let embedder: Arc<dyn SemanticEmbedder> = Arc::new(manager.clone());
    let bus = bus.clone();
    supervisor.spawn("index", "semantic-backfill".into(), move |_| {
        run_semantic_backfill(&db_dir, embedder, &bus, worker_count_for_ai());
    });
}

fn worker_count_for_ai() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
}

/// 语义检索：embed 查询 → HNSW KNN → 资产账 join（过滤失效 id 与
/// min_score），按相似度降序。
pub fn search(
    db_dir: &Path,
    db: &Db,
    embedder: &dyn SemanticEmbedder,
    query: &str,
    limit: u32,
    min_score: Option<f32>,
) -> Result<Vec<(i64, f32)>, String> {
    let query_vec = embedder.embed_text(query)?;
    let index = shared_index(db_dir)?;
    let idx = index.lock().expect("semantic index mutex poisoned");
    if idx.size() == 0 {
        return Ok(Vec::new());
    }
    let candidates = (limit as usize * 3).clamp(16, 256);
    let matches = idx
        .search(&query_vec, candidates)
        .map_err(|e| format!("向量检索失败: {e}"))?;
    drop(idx);
    // usearch Cos 返回的是距离（1 - cos 相似度）：转相似度并降序
    let mut scored: Vec<(i64, f32)> = matches
        .keys
        .iter()
        .map(|k| *k as i64)
        .zip(matches.distances.iter().map(|d| 1.0 - d))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut hits = Vec::with_capacity(scored.len());
    for (asset_id, score) in scored {
        // 资产账 join：失效 id（资产被删/已损坏）过滤
        if !db.asset_searchable(asset_id).map_err(|e| e.to_string())? {
            continue;
        }
        if let Some(min) = min_score {
            if score < min {
                continue;
            }
        }
        hits.push((asset_id, score));
        if hits.len() >= limit as usize {
            break;
        }
    }
    Ok(hits)
}
