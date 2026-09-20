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
use std::hash::{DefaultHasher, Hash, Hasher};
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

/// usearch 的 Windows 高层封装使用窄字符串路径，库目录含中文时 save/view 会
/// 返回 `No such file or directory`。这类库使用系统临时目录中的纯 ASCII 镜像
/// 承载 mmap/保存，每次保存后再由 Rust 的 Unicode 文件 API 复制回库目录。
fn runtime_vectors_path(db_dir: &Path) -> Result<PathBuf, String> {
    let storage = vectors_path(db_dir);
    if storage.to_string_lossy().is_ascii() {
        return Ok(storage);
    }
    let mut hasher = DefaultHasher::new();
    pool_key(db_dir).hash(&mut hasher);
    let runtime_dir = std::env::temp_dir().join("smart-photo-vectors");
    std::fs::create_dir_all(&runtime_dir).map_err(|e| e.to_string())?;
    Ok(runtime_dir.join(format!("{:016x}.usearch", hasher.finish())))
}

fn save_index(index: &Index, db_dir: &Path) -> Result<(), String> {
    let storage = vectors_path(db_dir);
    let runtime = runtime_vectors_path(db_dir)?;
    let runtime_str = runtime.to_str().ok_or("向量运行时路径含非 UTF-8 字符")?;
    index.save(runtime_str).map_err(|e| e.to_string())?;
    if runtime != storage {
        std::fs::copy(&runtime, &storage).map_err(|e| format!("同步向量索引到库目录失败: {e}"))?;
    }
    Ok(())
}

/// 取（或懒加载）某库的共享 HNSW 索引。
pub fn shared_index(db_dir: &Path) -> Result<Arc<Mutex<Index>>, String> {
    let key = pool_key(db_dir);
    let mut pool = pool().lock().expect("semantic pool mutex poisoned");
    if let Some(idx) = pool.get(&key) {
        return Ok(Arc::clone(idx));
    }
    let storage = vectors_path(db_dir);
    let runtime = runtime_vectors_path(db_dir)?;
    let index = if storage.is_file() {
        if runtime != storage {
            std::fs::copy(&storage, &runtime)
                .map_err(|e| format!("准备向量索引运行时镜像失败: {e}"))?;
        }
        // 已有索引文件：view（mmap 零拷贝加载）
        let idx = new_index()?;
        idx.view(runtime.to_str().ok_or("向量运行时路径含非 UTF-8 字符")?)
            .map_err(|e| e.to_string())?;
        idx
    } else {
        new_index()?
    };
    let shared = Arc::new(Mutex::new(index));
    pool.insert(key.clone(), Arc::clone(&shared));
    Ok(Arc::clone(pool.get(&key).expect("just inserted")))
}

/// 索引缓存失效（语义重建删 vectors.usearch 后调用：池内 mmap 的旧索引
/// 必须逐出，否则后续 add/search 命中已删除文件的旧视图）。
pub fn invalidate_index(db_dir: &Path) {
    pool()
        .lock()
        .expect("semantic pool mutex poisoned")
        .remove(&pool_key(db_dir));
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
fn process_ai_task(
    db: &Db,
    db_dir: &Path,
    embedder: &dyn SemanticEmbedder,
    asset_id: i64,
) -> Result<(), String> {
    let Some((path, _thumb_state)) = db.thumb_info_by_id(asset_id).ok().flatten() else {
        return Err("资产不存在".into()); // 资产已删除（级联清任务前的防御兜底）
    };
    // 256 档缩略图（缺失则顺手生成；这同时是 embed 的输入）
    crate::thumbs::thumb_file(db_dir, Path::new(&path), 256)
        .ok_or_else(|| format!("缩略图生成失败: {path}"))?;
    let vector = embedder.embed_image(Path::new(&path), db_dir)?;
    let index = shared_index(db_dir)?;
    let mut idx = index.lock().expect("semantic index mutex poisoned");
    // save 成功、SQLite 记账前若进程崩溃，重启后任务仍会重放；索引中已有
    // key 时直接补记账，保证回填幂等而不是报 Duplicate keys。
    if idx.contains(asset_id as u64) {
        return db.set_ai_indexed(asset_id).map_err(|e| e.to_string());
    }
    let needed = idx.size() + 1;
    ensure_capacity(&mut idx, needed)?;
    idx.add(asset_id as u64, &vector)
        .map_err(|e| e.to_string())?;
    // 每条落盘（防崩溃丢账；调用方为批量时由 worker 循环天然降频）
    if let Err(error) = save_index(&idx, db_dir) {
        // 保存失败必须回滚内存 key，否则本轮自动重试只会得到重复键错误。
        let _ = idx.remove(asset_id as u64);
        return Err(error);
    }
    drop(idx);
    db.set_ai_indexed(asset_id).map_err(|e| e.to_string())
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
        eprintln!("语义回填：库打开失败（{}），本轮跳过", db_dir.display());
        return 0;
    };
    // 补种只做幂等兜底；派 worker 与否看存量 pending（修复：此前以「本轮
    // 新建数」为闸——任务已存在时（重复 kick/启动恢复）新建数为 0，worker
    // 从不派出，119 条存量 pending 永远无人消费，点击立即索引假成功）。
    if let Err(e) = db.create_ai_tasks_for_unindexed() {
        eprintln!("语义回填：补种任务失败: {e}");
    }
    let pending = db.pending_index_task_count("ai").unwrap_or(0);
    if pending == 0 {
        return 0;
    }
    let (indexed_before, total) = db.ai_index_progress().unwrap_or((0, pending));
    // 所有 worker 共用一个完成计数，并在锁内发布事件，保证事件严格单调；
    // 不能让每个 worker 各报 1..N，最后把 UI 留在 30/119。
    let progress_done = Arc::new(Mutex::new(indexed_before));
    let mut done = 0u64;
    let mut handles = Vec::new();
    for n in 0..workers.max(1) {
        let db_dir = db_dir.to_path_buf();
        let bus = bus.clone();
        let embedder = Arc::clone(&embedder);
        let progress_done = Arc::clone(&progress_done);
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
                            Ok(None) => break,
                            Err(e) => {
                                eprintln!("语义回填：认领任务失败，worker 退出: {e}");
                                break;
                            }
                        };
                        let result = process_ai_task(&db, &db_dir, &*embedder, task.asset_id);
                        if let Err(error) = &result {
                            eprintln!("语义索引失败 asset_id={}: {error}", task.asset_id);
                        }
                        let ok = result.is_ok();
                        let _ = db.finish_index_task(task.id, ok);
                        count += u64::from(ok);
                        {
                            let mut global_done = progress_done
                                .lock()
                                .expect("semantic progress mutex poisoned");
                            if ok {
                                *global_done += 1;
                            }
                            bus.publish(AppEvent::IndexTaskProgress {
                                kind: "ai".into(),
                                done: *global_done,
                                total,
                            });
                        }
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
    let _ = supervisor.spawn_unique("index", "semantic-backfill".into(), move |_| {
        run_semantic_backfill(&db_dir, embedder, &bus, worker_count_for_ai());
    });
}

/// AI 推理 worker 数（测试断言用）：ort 会话内存/线程重，封顶 4——
/// embed.rs 会话 intra 线程 ≈ 核心数/4，总线程 ≈ 全核不超订。
pub fn worker_count_for_ai() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 4)
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
