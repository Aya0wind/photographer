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

/// 空闲卸载（ai::idle::release_all 调用）：清池只丢池侧引用，在途查询
/// 自持 Arc，索引用完自然释放。
pub fn release() {
    pool().lock().expect("semantic pool mutex poisoned").clear();
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
    let runtime_dir = std::env::temp_dir().join("photographer-vectors");
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

/// 取（或懒加载）某库的共享 HNSW 索引（读写同一把柄，互斥串行）。
///
/// 设计取舍（2026-09-20 两起真机事故后定案）：**load（全量读入，可变），
/// 不用 mmap view**——①view 只读，回填 add 报 immutable；②Windows 下
/// 活着的 view mmap 会挡住另一句柄对同文件的写（Permission denied）。
/// 搜索与回填共享同一互斥把柄（搜索毫秒级、写低频，争用可忽略）；
/// 20 万库 ~600MB 驻留的优化届时再议（分库 mmap+COW 或量化压缩）。
pub fn shared_index(db_dir: &Path) -> Result<Arc<Mutex<Index>>, String> {
    super::idle::touch();
    let key = pool_key(db_dir);
    let mut pool = pool().lock().expect("semantic pool mutex poisoned");
    if let Some(idx) = pool.get(&key) {
        return Ok(Arc::clone(idx));
    }
    let storage = vectors_path(db_dir);
    let runtime = runtime_vectors_path(db_dir)?;
    let index = new_index()?;
    let index = if storage.is_file() {
        if runtime != storage {
            std::fs::copy(&storage, &runtime)
                .map_err(|e| format!("准备向量索引运行时镜像失败: {}", e))?;
        }
        let loaded = index;
        loaded
            .load(runtime.to_str().ok_or("向量运行时路径含非 UTF-8 字符")?)
            .map_err(|e| format!("加载向量索引失败: {}", e))?;
        loaded
    } else {
        index
    };
    let shared = Arc::new(Mutex::new(index));
    pool.insert(key.clone(), Arc::clone(&shared));
    Ok(Arc::clone(pool.get(&key).expect("just inserted")))
}

/// 索引缓存失效（语义重建删 vectors.usearch 后调用：搜索 view 池与
/// 可写池一并逐出，否则后续 add/search 命中已删除文件的旧句柄）。
pub fn invalidate_index(db_dir: &Path) {
    pool()
        .lock()
        .expect("semantic pool mutex poisoned")
        .remove(&pool_key(db_dir));
}

pub fn writable_index(db_dir: &Path) -> Result<Arc<Mutex<Index>>, String> {
    shared_index(db_dir)
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

/// 单条向量入库（批内逐条调用）：usearch 插入 → 落盘 → 记账。幂等/回滚
/// 语义与旧单条路径一致：save 成功、SQLite 记账前崩溃 → 重放任务命中
/// contains 直接补记账；save 失败回滚内存 key（防重试 Duplicate keys）。
fn commit_vector(db: &Db, db_dir: &Path, asset_id: i64, vector: &[f32]) -> Result<(), String> {
    let index = writable_index(db_dir)?;
    let mut idx = index.lock().expect("semantic index mutex poisoned");
    if idx.contains(asset_id as u64) {
        return db.set_ai_indexed(asset_id).map_err(|e| e.to_string());
    }
    let needed = idx.size() + 1;
    ensure_capacity(&mut idx, needed)?;
    idx.add(asset_id as u64, vector)
        .map_err(|e| e.to_string())?;
    // 每条落盘（防崩溃丢账；批内逐条调用，锁持有时间 = 单次 save）
    if let Err(error) = save_index(&idx, db_dir) {
        let _ = idx.remove(asset_id as u64);
        return Err(error);
    }
    drop(idx);
    db.set_ai_indexed(asset_id).map_err(|e| e.to_string())
}

/// 一批 ai 任务处理（2026-09-21 批量化，实测单图→批 16 吞吐 2-4x）：
/// 批内逐条解析资产路径 → **一次 embed_images 批推理**（多图单次
/// session.run）→ 逐条 commit_vector 入库记账。批推理失败自动降级逐图
/// 重试——单条坏图（缩略图缺失/解码失败）只废自己，不连坐整批。
/// 返回批内每条任务的成败（与 tasks 下标对齐）。
fn process_ai_batch(
    db: &Db,
    db_dir: &Path,
    embedder: &dyn SemanticEmbedder,
    asset_ids: &[i64],
) -> Vec<Result<(), String>> {
    let mut paths: Vec<Option<String>> = Vec::with_capacity(asset_ids.len());
    for &id in asset_ids {
        paths.push(db.thumb_info_by_id(id).ok().flatten().map(|(path, _)| path));
    }
    let alive: Vec<(usize, &str)> = paths
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_deref().map(|p| (i, p)))
        .collect();
    // 256 档缩略图备齐（缺失生成；失败该条按失败结算，不连坐批）——
    // 嵌入器契约不保证自带缩略图（桩/自定义实现），此处显式把关
    let mut thumb_failed = vec![false; asset_ids.len()];
    for &(i, path) in &alive {
        if crate::thumbs::thumb_file(db_dir, Path::new(path), 256).is_none() {
            eprintln!(
                "语义索引失败 asset_id={}: 缩略图生成失败: {path}",
                asset_ids[i]
            );
            thumb_failed[i] = true;
        }
    }
    let embeddable: Vec<(usize, &str)> = alive
        .iter()
        .copied()
        .filter(|(i, _)| !thumb_failed[*i])
        .collect();
    let mut vectors: Vec<Option<Vec<f32>>> = vec![None; asset_ids.len()];
    if !embeddable.is_empty() {
        let srcs: Vec<std::path::PathBuf> = embeddable
            .iter()
            .map(|&(_, p)| std::path::PathBuf::from(p))
            .collect();
        let batch: Result<Vec<Vec<f32>>, String> = embedder.embed_images(&srcs, db_dir);
        match batch {
            Ok(rows) => {
                for ((slot, _), vec) in embeddable.iter().zip(rows) {
                    vectors[*slot] = Some(vec);
                }
            }
            Err(batch_err) => {
                // 批失败降级逐图：隔离坏图（解码失败等），其余照常产出向量
                eprintln!("语义批推理失败，降级逐图: {batch_err}");
                for &(i, p) in &embeddable {
                    vectors[i] = embedder
                        .embed_image(Path::new(p), db_dir)
                        .map_err(|e| {
                            eprintln!("语义嵌入失败 asset_id={}: {e}", asset_ids[i]);
                            e
                        })
                        .ok();
                }
            }
        }
    }
    (0..asset_ids.len())
        .map(|i| match (&paths[i], &vectors[i], thumb_failed[i]) {
            (None, _, _) => Err("资产不存在".into()),
            (Some(_), _, true) => Err("缩略图生成失败".into()),
            (Some(_), None, false) => Err("嵌入失败".into()),
            (Some(_), Some(v), false) => commit_vector(db, db_dir, asset_ids[i], v),
        })
        .collect()
}

/// 语义回填批大小上限（2026-09-21 实测定值，RTX 5070 Ti DML / 24 核）：
/// **批量只在 DML 激活时启用**（[`super::prefer_batch_inference`]）——
/// DML：批16 = 24.6 img/s vs 单图 20.0（+23%）；CPU：批16 = 12.5 vs
/// 单图 14.1（**-12%**，单图 intra-op 已吃满核）→ CPU 走批 1。
/// 完整矩阵见 tests/dml_bench_test.rs 基准记录。
pub const AI_BATCH: usize = 16;

/// Workers and claimed batches obey the environment CPU/2 semantic budget.
pub fn worker_count_for_ai() -> usize {
    crate::tasks::index_parallelism()
}

/// 语义回填 + 并发执行（模型齐备 && enable_clip 时由调用方触发）：
/// ① 为 `ai_indexed_at IS NULL` 且没有未完成 ai 任务的库内照片建任务
/// ② worker 按批认领（DML 激活 = AI_BATCH 条，CPU = 1 条/批）→ 批推理
///   （单次 session.run）→ 逐条入库记账，逐条发布 indexTaskProgress
///   {kind:"ai"}（严格单调）。返回本轮成功嵌入数。
/// `vision_model` = 当前档位的 vision 模型 id（int8/fp16 二选一，批量化
/// 决策按它查 DML 毒化位——档位解析见调用方 kick_semantic_if_ready）。
pub fn run_semantic_backfill(
    db_dir: &Path,
    embedder: Arc<dyn SemanticEmbedder>,
    bus: &EventBus,
    workers: usize,
    vision_model: &str,
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
    let vision_model = vision_model.to_string();
    let mut done = 0u64;
    let mut handles = Vec::new();
    for n in 0..workers.max(1) {
        let db_dir = db_dir.to_path_buf();
        let bus = bus.clone();
        let embedder = Arc::clone(&embedder);
        let progress_done = Arc::clone(&progress_done);
        let vision_model = vision_model.clone();
        handles.push(
            std::thread::Builder::new()
                .name(format!("index-ai-{n}"))
                .spawn(move || -> u64 {
                    let Ok(db) = crate::ipc::open_library_db(&db_dir) else {
                        return 0;
                    };
                    let mut count = 0u64;
                    // 批大按 EP 运行态定：DML 激活 = 16（快 23%），CPU = 1
                    // （批化反而 -12%，见 prefer_batch_inference 注释）；
                    // DML 中途毒化后后续批次自动落 1（批量化是语义通道决策
                    // → 按当前档位的 siglip vision 标签查毒化位）
                    loop {
                        let requested_batch = if embedder.prefer_batch(&vision_model) {
                            AI_BATCH
                        } else {
                            1
                        };
                        let _permit = crate::tasks::index_budget::budget(
                            crate::tasks::index_budget::Domain::Semantic,
                        )
                        .acquire(requested_batch);
                        let batch_size = _permit.count;
                        // 批量认领 → 单次批推理 → 逐条结算
                        let tasks = match db.claim_index_tasks("ai", batch_size) {
                            Ok(t) => t,
                            Err(e) => {
                                eprintln!("语义回填：认领任务失败，worker 退出: {e}");
                                break;
                            }
                        };
                        if tasks.is_empty() {
                            break;
                        }
                        let asset_ids: Vec<i64> = tasks.iter().map(|t| t.asset_id).collect();
                        let results = process_ai_batch(&db, &db_dir, &*embedder, &asset_ids);
                        for (task, result) in tasks.iter().zip(&results) {
                            if let Err(error) = result {
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
                                    // 导入仍在追加照片，总数不能冻结在本轮启动时。
                                    total: db
                                        .ai_index_progress()
                                        .map(|(_, total)| total)
                                        .unwrap_or(total)
                                        .max(*global_done),
                                });
                            }
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
    // 收尾逐出双池：搜索 view 从最新落盘重映射（否则检索读旧 mmap）；
    // 可写池释放全量驻留内存（下次回填按需再 load）
    invalidate_index(db_dir);
    // 一轮收尾通知（done>0 才发）：前端据此自动重建智能相册标签索引
    if done > 0 {
        bus.publish(AppEvent::AiIndexFinished { done });
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
    let vision_model = super::semantic_model_ids(manager.ai_params().quality_tier)[0].to_string();
    let bus = bus.clone();
    let _ = supervisor.spawn_coalesced(
        "index",
        format!("semantic-backfill:{}", db_dir.display()),
        move |_| {
            run_semantic_backfill(
                &db_dir,
                Arc::clone(&embedder),
                &bus,
                worker_count_for_ai(),
                &vision_model,
            );
        },
    );
}

/// 语义分数显示标定（经验锚点；阈值过滤仍用原始分数，见
/// [`semantic_default_min_score`] 注释的各轮实测分布）：
/// - **int8**（fast/normal，2026-09-21 于 162 张真库）：无关 top ≤0.096、
///   相关簇 0.09-0.12、长尾地板 ≈0.04 → floor 0.04 / ceiling 0.125
///   （工作点 0.09 显示为 ~52%）。
/// - **fp16**（accurate，2026-09-28 于 497 张真库）：cos 带整体压低居中于
///   0（中位 ≈-0.03）、荒谬 top ∈[-0.001,0.032]、内容 top ∈[0.010,0.046]
///   （日落 0.0463 浮出带外）→ floor 0.0 / ceiling 0.06（负分归 0；工作点
///   0.03 显示为 50%，日落 0.0463 显示为 ~77%——与 int8 档同位观感）。
/// 返回前端前做线性拉伸到 [0,1]，否则窄带原始分会显示成「3%」。
pub const SEMANTIC_SCORE_FLOOR: f32 = 0.04;
pub const SEMANTIC_SCORE_CEILING: f32 = 0.125;
/// fp16 变体（accurate 档）的显示拉伸带（标定依据见上）。
pub const SEMANTIC_SCORE_FLOOR_FP16: f32 = 0.0;
pub const SEMANTIC_SCORE_CEILING_FP16: f32 = 0.06;

/// 阈值 auto 默认值（settings.ai.semantic_min_score = null 时按当前语义
/// 模型变体取，2026-09-28 三档画质引入）：
/// - **int8**（fast/normal）：0.09——2026-09-21 标定轮工作点（方法：162 张
///   真库全量重嵌入后，荒谬词查询（「手术台」「无人机航拍」等 5+ 个库内
///   无对应内容的词）top-1 分 vs 内容词查询（「猫」「海边」等）相关簇
///   分数的分隔带取值：无关 top ≤0.096、相关簇 0.09-0.12，工作点落在
///   带内偏保守侧 0.09）。
/// - **fp16**（accurate）：0.03——2026-09-28 真机标定（RTX 5070 Ti DML 批
///   16，497 张真库 `I:\SmartPhoto\主库`，方法同 int8 轮：荒谬词 5 个 ×
///   内容词 5 个的 top-1 分布取分隔带）。实测 fp16 双塔 cos 带整体比 int8
///   压低并居中于 0（中位 ≈-0.03；int8 中位 ≈+0.06）：
///   荒谬 top-1 ∈ [-0.001, 0.032]（手术台 -0.0008 / 无人机航拍 0.0222 /
///   税务审计报表 -0.0004 / 深海钻井平台 0.0081 / 考古发掘现场 0.0315），
///   内容 top-1 ∈ [0.010, 0.046]（猫 0.0104 / 狗 0.0174 / 人像 0.0118 /
///   海边 0.0377 / 日落 0.0463——强相关内容明确浮出带外）。0.03 滤掉
///   荒谬词主体（4/5 低于 0.03）并放行强内容命中，与 int8 轮 0.09 的
///   「带内偏保守」取点同位。相对排序两变体一致（日落 > 海边 > 其余）。
pub fn semantic_default_min_score(tier: super::QualityTier) -> f32 {
    match tier {
        super::QualityTier::Accurate => 0.03, // fp16 标定（2026-09-28，数值见上）
        _ => 0.09,                            // int8 标定工作点（2026-09-21）
    }
}

/// 原始 cos 相似度 → 显示分数 [0,1]（按当前语义模型变体取拉伸带）：
/// floor 以下归 0，ceiling 以上饱和 1。
pub fn calibrated_display_score(raw: f32, tier: super::QualityTier) -> f32 {
    let (floor, ceiling) = match tier {
        super::QualityTier::Accurate => (SEMANTIC_SCORE_FLOOR_FP16, SEMANTIC_SCORE_CEILING_FP16),
        _ => (SEMANTIC_SCORE_FLOOR, SEMANTIC_SCORE_CEILING),
    };
    ((raw - floor) / (ceiling - floor)).clamp(0.0, 1.0)
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
