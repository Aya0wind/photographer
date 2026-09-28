//! 语义嵌入与向量检索（M4）：确定性随机向量桩验证 HNSW 索引/检索/
//! 持久化/过滤契约；#[ignore] 真 smoke（真实模型 + 真实 NEF/照片）与
//! 20 万规模 benchmark。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::Path;

use ai::embed::{SemanticEmbedder, EMBED_DIM};

/// 确定性伪随机向量桩（xorshift64 种子派生；归一化 768 维）。
/// 相似文本/路径没有语义，仅用于验证索引与过滤逻辑。
pub struct StubEmbedder;

impl StubEmbedder {
    pub fn vector(seed: u64) -> Vec<f32> {
        let mut x = seed | 1;
        let mut v = Vec::with_capacity(EMBED_DIM);
        for _ in 0..EMBED_DIM {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            v.push((x % 20001) as f32 / 10000.0 - 1.0);
        }
        let norm = v.iter().map(|a| a * a).sum::<f32>().sqrt();
        v.iter().map(|a| a / norm).collect()
    }
}

impl SemanticEmbedder for StubEmbedder {
    fn embed_image(&self, _src: &Path, _db_dir: &Path) -> Result<Vec<f32>, String> {
        Ok(Self::vector(42))
    }

    fn embed_text(&self, text: &str) -> Result<Vec<f32>, String> {
        let seed = text.bytes().fold(0x9E3779B97F4A7C15u64, |acc, b| {
            acc.wrapping_mul(31).wrapping_add(u64::from(b))
        });
        Ok(Self::vector(seed))
    }
}

#[test]
fn embed_dim_contract() {
    assert_eq!(ai::embed::EMBED_DIM, 768);
    assert_eq!(StubEmbedder::vector(1).len(), 768);
    assert_eq!(StubEmbedder::vector(2).len(), 768);
    // 归一化：模长 1
    let v = StubEmbedder::vector(7);
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-4, "模长应≈1: {norm}");
}

#[test]
fn hnsw_add_search_and_persistence() {
    let lib = tempfile::tempdir().unwrap();
    let embedder = StubEmbedder;

    // 造 32 个"资产"：随机向量 + 1 个查询向量的近邻（加噪声）
    let mut keys = Vec::new();
    for i in 0..32u64 {
        let v = StubEmbedder::vector(1000 + i);
        keys.push((i, v));
    }
    // 近邻副本必须基于文本查询的真实嵌入（stub 按文本哈希确定性生成）
    let query = embedder.embed_text("海边日落").unwrap();
    let near: Vec<f32> = query
        .iter()
        .map(|x| x + (StubEmbedder::vector(999)[0]) * 0.01)
        .collect();

    // 直接操作共享索引（模拟 process_ai_task 的插入侧）
    let index = ai::semantic::shared_index(lib.path()).unwrap();
    {
        let idx = index.lock().unwrap();
        idx.reserve(64).unwrap();
        for (i, (key, v)) in keys.iter().enumerate() {
            let _ = i;
            idx.add(*key, v).unwrap();
        }
        idx.add(10_000, &near).unwrap();
        idx.save(lib.path().join("vectors.usearch").to_str().unwrap())
            .unwrap();
    }

    // 检索：query 的最近邻应是 10_000（其加噪副本），cos ≈ 1
    let hits = ai::semantic::search(
        lib.path(),
        &open_search_db(lib.path()),
        &embedder,
        "海边日落",
        5,
        None,
    )
    .unwrap();
    assert!(!hits.is_empty());
    assert_eq!(hits[0].0, 10_000, "最近邻应为加噪副本");
    assert!(hits[0].1 > 0.99, "自身副本 cos≈1: {}", hits[0].1);
    // score 降序
    assert!(hits.windows(2).all(|w| w[0].1 >= w[1].1));

    // min_score 过滤
    let strict = ai::semantic::search(
        lib.path(),
        &open_search_db(lib.path()),
        &embedder,
        "海边日落",
        5,
        Some(0.999),
    )
    .unwrap();
    assert!(strict.iter().all(|(_, s)| *s >= 0.999));
}

/// 检索测试用的最小 assets 表（join 需要资产账：给 0..32 与 10_000 建
/// photo 资产行）。
fn open_search_db(dir: &Path) -> db::Db {
    let database = common::open_db(dir);
    for id in 0..32u64 {
        database
            .0
            .execute(
                "INSERT OR IGNORE INTO assets (id, path, filename, size, mtime, xxhash, kind, captured_at, camera, source, created_at)                  VALUES (?1, ?2, 'x.jpg', 1, '2026', 1, 'photo', NULL, NULL, 'imported', '2026')",
                rusqlite::params![id as i64, format!("X:/p/{id}.jpg")],
            )
            .unwrap();
    }
    database
        .0
        .execute(
            "INSERT OR IGNORE INTO assets (id, path, filename, size, mtime, xxhash, kind, captured_at, camera, source, created_at)              VALUES (10000, 'X:/p/near.jpg', 'near.jpg', 1, '2026', 1, 'photo', NULL, NULL, 'imported', '2026')",
            [],
        )
        .unwrap();
    database
}

#[test]
fn persistence_view_reloads_vectors_from_disk() {
    let lib = tempfile::tempdir().unwrap();
    let index = ai::semantic::shared_index(lib.path()).unwrap();
    {
        let idx = index.lock().unwrap();
        idx.reserve(16).unwrap();
        idx.add(7, &StubEmbedder::vector(11)).unwrap();
        idx.add(9, &StubEmbedder::vector(12)).unwrap();
        idx.save(lib.path().join("vectors.usearch").to_str().unwrap())
            .unwrap();
    }
    drop(index);

    // 新"库目录"放置同一文件 → view 恢复全部向量（持久化契约）
    let lib2 = tempfile::tempdir().unwrap();
    std::fs::copy(
        lib.path().join("vectors.usearch"),
        lib2.path().join("vectors.usearch"),
    )
    .unwrap();
    let index2 = ai::semantic::shared_index(lib2.path()).unwrap();
    let idx = index2.lock().unwrap();
    assert_eq!(idx.size(), 2, "view 应恢复 2 条向量");
    let matches = idx.search(&StubEmbedder::vector(11), 1).unwrap();
    assert_eq!(matches.keys[0], 7);
}

#[test]
fn empty_index_search_returns_empty() {
    let lib = tempfile::tempdir().unwrap();
    let hits = ai::semantic::search(
        lib.path(),
        &open_search_db(lib.path()),
        &StubEmbedder,
        "query",
        5,
        None,
    )
    .unwrap();
    assert!(hits.is_empty());
}

/// 回归（真机修复 2026-09-19）：回填 worker 的启动判据必须是「存量 pending」
/// 而非「本轮新建任务数」。此前任务已存在（重复 kick/启动恢复）时新建数为
/// 0 → 直接空转返回，存量 pending 永远无人消费，点击立即索引假成功。
#[test]
fn semantic_backfill_consumes_existing_pending_tasks() {
    let lib = tempfile::tempdir().unwrap();
    let database = common::open_db(lib.path());
    database
        .0
        .execute(
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, 'X:/p/1.jpg', '1.jpg', 1, '2026', 1, 'photo', \
             NULL, NULL, 'imported', '2026')",
            [],
        )
        .unwrap();
    // 历史遗留场景：任务已入库但 worker 从未消费
    assert_eq!(database.create_ai_tasks_for_unindexed().unwrap(), 1);
    assert_eq!(database.pending_index_task_count("ai").unwrap(), 1);

    // 二次回填：新建数为 0（幂等补种），存量 pending 也必须被消费
    let done = ai::semantic::run_semantic_backfill(
        lib.path(),
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        2,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    assert_eq!(
        database.pending_index_task_count("ai").unwrap(),
        0,
        "存量 pending 必须被 worker 消费"
    );
    // 资产路径不存在 → 缩略图生成失败 → 处理必败（attempts 封顶进 failed），
    // 不可能凭空成功；关键断言是上面「pending 被消费」
    assert_eq!(done, 0);
    let failed: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'ai' AND state = 'failed'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed, 1);
}

#[test]
fn semantic_backfill_persists_in_non_ascii_library_path() {
    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("中文库");
    std::fs::create_dir_all(&lib).unwrap();
    let source = root.path().join("source.jpg");
    image::RgbImage::from_pixel(8, 8, image::Rgb([120, 80, 40]))
        .save(&source)
        .unwrap();
    let database = common::open_db(&lib);
    database
        .0
        .execute(
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, ?1, 'source.jpg', 1, '2026', 1, 'photo', \
             NULL, NULL, 'imported', '2026')",
            rusqlite::params![source.to_string_lossy()],
        )
        .unwrap();

    let done = ai::semantic::run_semantic_backfill(
        &lib,
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        1,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );

    assert_eq!(done, 1);
    assert!(lib.join("vectors.usearch").is_file());
    assert_eq!(database.pending_index_task_count("ai").unwrap(), 0);

    // 模拟“向量已保存、SQLite 记账前崩溃”：重放同一 asset_id 应补记账，
    // 不能因 duplicate key 再次失败。
    database
        .0
        .execute("UPDATE assets SET ai_indexed_at = NULL WHERE id = 1", [])
        .unwrap();
    database
        .0
        .execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0 WHERE kind = 'ai' AND asset_id = 1",
            [],
        )
        .unwrap();
    let replayed = ai::semantic::run_semantic_backfill(
        &lib,
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        1,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    assert_eq!(replayed, 1);
}

/// worker 契约（2026-09-21 批量化后）：≥16 核 4 / ≥8 核 2 / 其余 1——
/// 多 worker 的预处理与 GPU Run 流水线重叠（实测 1w=24.5→4w=57.7
/// img/s），embed 会话 intra 线程 4×(核/4)=全核不超订。见 semantic.rs
/// 注释与 tests/dml_bench_test.rs 基准。
#[test]
fn ai_worker_count_capped() {
    let n = ai::semantic::worker_count_for_ai();
    assert!((1..=4).contains(&n), "AI worker 应 ∈ [1,4]，实际 {n}");
}

// ---------------------------------------------------------------------------
// 批量认领 + 批推理路径（2026-09-21 并发优化）
// ---------------------------------------------------------------------------

/// FK 合规的占位资产行（index_tasks.asset_id 外键指向 assets）。
fn seed_placeholder_assets(database: &db::Db, ids: std::ops::Range<i64>) {
    for id in ids {
        database
            .0
            .execute(
                "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
                 kind, captured_at, camera, source, created_at) \
                 VALUES (?1, ?2, ?3, 1, '2026', 1, 'photo', \
                 NULL, NULL, 'imported', '2026')",
                rusqlite::params![id, format!("X:/p/{id}.jpg"), format!("{id}.jpg")],
            )
            .unwrap();
    }
}

/// 批量认领：limit 内原子置 running、id 升序；不足 limit 小批；空批为空；
/// 全部 running 后单条认领也认不出（同表互斥）。
#[test]
fn claim_index_tasks_batches_ordered_and_atomic() {
    let lib = tempfile::tempdir().unwrap();
    let database = common::open_db(lib.path());
    seed_placeholder_assets(&database, 1..6);
    let now = "2026-09-21T00:00:00Z";
    for id in 1..=5 {
        database
            .0
            .execute(
                "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
                 VALUES ('ai', ?1, 'pending', 0, ?2, ?2)",
                rusqlite::params![id, now],
            )
            .unwrap();
    }
    let first = database.claim_index_tasks("ai", 3).unwrap();
    assert_eq!(
        first.iter().map(|t| t.asset_id).collect::<Vec<_>>(),
        vec![1, 2, 3],
        "id 升序小批"
    );
    assert!(first.iter().all(|t| t.state == "running"));
    let second = database.claim_index_tasks("ai", 8).unwrap();
    assert_eq!(
        second.iter().map(|t| t.asset_id).collect::<Vec<_>>(),
        vec![4, 5],
        "剩余不足 limit 照常小批"
    );
    assert!(database.claim_index_tasks("ai", 4).unwrap().is_empty());
    assert!(database.claim_index_task("ai").unwrap().is_none());
}

/// 并发批量认领不重不漏（两线程交错抢同 kind，并集=全集、无重复）。
#[test]
fn concurrent_batch_claims_are_disjoint() {
    let lib = tempfile::tempdir().unwrap();
    {
        let database = common::open_db(lib.path());
        seed_placeholder_assets(&database, 1..21);
        let now = "2026-09-21T00:00:00Z";
        for id in 1..=20 {
            database
                .0
                .execute(
                    "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
                     VALUES ('ai', ?1, 'pending', 0, ?2, ?2)",
                    rusqlite::params![id, now],
                )
                .unwrap();
        }
    }
    let path = lib.path().to_path_buf();
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            std::thread::spawn(move || {
                let db = common::open_db(&path);
                let mut got = Vec::new();
                loop {
                    let batch = db.claim_index_tasks("ai", 4).unwrap();
                    if batch.is_empty() {
                        break;
                    }
                    got.extend(batch.iter().map(|t| t.asset_id));
                }
                got
            })
        })
        .collect();
    let mut all = Vec::new();
    for h in handles {
        all.extend(h.join().unwrap());
    }
    all.sort_unstable();
    assert_eq!(all, (1..=20).collect::<Vec<_>>(), "并集=全集");
    let mut dedup = all.clone();
    dedup.dedup();
    assert_eq!(dedup.len(), all.len(), "无重复认领");
}

/// 批推理失败自动降级逐图 + 坏图只废自己（批内失败隔离）：
/// 桩的 embed_images 恒失败（模拟批 Run 报错），embed_image 对坏路径
/// 报错、好路径成功——回填后好资产完成索引，坏资产任务 failed。
struct FlakyBatchEmbedder {
    bad_marker: String,
}

impl SemanticEmbedder for FlakyBatchEmbedder {
    fn embed_image(&self, src: &Path, _db_dir: &Path) -> Result<Vec<f32>, String> {
        if src.to_string_lossy().contains(&self.bad_marker) {
            Err("坏图".into())
        } else {
            Ok(StubEmbedder::vector(42))
        }
    }
    fn embed_images(
        &self,
        _srcs: &[std::path::PathBuf],
        _db_dir: &Path,
    ) -> Result<Vec<Vec<f32>>, String> {
        Err("批推理炸了（模拟 DML/显存错误）".into())
    }
    fn embed_text(&self, _text: &str) -> Result<Vec<f32>, String> {
        Ok(StubEmbedder::vector(7))
    }
}

#[test]
fn backfill_batch_failure_degrades_to_per_image_and_isolates_bad_apple() {
    let root = tempfile::tempdir().unwrap();
    let database = common::open_db(root.path());
    // 3 个真实图片资产（真 JPEG 才能过缩略图关）+ 1 个坏路径资产
    let mut ids = Vec::new();
    for n in 0..3 {
        let p = root.path().join(format!("ok{n}.jpg"));
        image::RgbImage::from_pixel(8, 8, image::Rgb([90, 90, 90]))
            .save(&p)
            .unwrap();
        database
            .0
            .execute(
                "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
                 kind, captured_at, camera, source, created_at) \
                 VALUES (?1, ?2, ?3, 1, '2026', 1, 'photo', \
                 NULL, NULL, 'imported', '2026')",
                rusqlite::params![n + 1, p.to_string_lossy(), format!("ok{n}.jpg")],
            )
            .unwrap();
        ids.push(n + 1);
    }
    database
        .0
        .execute(
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
             kind, captured_at, camera, source, created_at) \
             VALUES (9, 'X:/p/bad.jpg', 'bad.jpg', 1, '2026', 1, 'photo', \
             NULL, NULL, 'imported', '2026')",
            [],
        )
        .unwrap();
    assert_eq!(database.create_ai_tasks_for_unindexed().unwrap(), 4);

    let done = ai::semantic::run_semantic_backfill(
        root.path(),
        std::sync::Arc::new(FlakyBatchEmbedder {
            bad_marker: "bad.jpg".into(),
        }),
        &events::EventBus::new(),
        1, // 单 worker：一批 4 条（AI_BATCH=16 > 4）走批失败→降级路径
        "siglip2-visual", // 档位解析：标签只影响批决策
    );
    assert_eq!(done, 3, "3 好图经降级逐图成功，坏图只废自己");
    let failed: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'ai' AND state = 'failed' \
             AND asset_id = 9",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(failed, 1, "坏图任务 failed");
    assert_eq!(database.pending_index_task_count("ai").unwrap(), 0);
    for id in ids {
        let indexed: Option<String> = database
            .0
            .query_row(
                "SELECT ai_indexed_at FROM assets WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap();
        assert!(indexed.is_some(), "asset {id} 应已记账");
    }
}

/// 诊断（真机 2026-09-19：主库 119 条 ai 任务 75ms/次快失败）：复现 app 真实
/// 环境——**中文库路径** + 真模型 + 真实回填链路（thumb → embed → usearch
/// add/save → 记账）。usearch 是 C++ 库，Windows 下非 ASCII 路径有 ANSI 转换
/// 失败风险。逐步断言，失败点直接暴露。#[ignore]：依赖已下载模型与样例图。
#[test]
#[ignore = "诊断用真机 smoke：需模型与样例图；复现主库中文路径回填失败"]
fn real_backfill_chinese_dbdir_diagnosis() {
    let Some(models) = models_dir() else {
        eprintln!("skip: 模型目录不存在");
        return;
    };
    let jpg = Path::new(r"I:\SmartPhoto-test-e2e\收纳\2025\06-07\DSC_0177.JPG");
    if !jpg.is_file() {
        eprintln!("skip: 样例不存在 {}", jpg.display());
        return;
    }
    // 中文目录名复现主库路径（I:\SmartPhoto\主库）
    let lib = tempfile::tempdir().unwrap();
    let db_dir = lib.path().join("主库");
    std::fs::create_dir_all(&db_dir).unwrap();
    let database = common::open_db(&db_dir);
    let target = db_dir.join("照片").join("DSC_0177.JPG");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::copy(jpg, &target).unwrap();
    database
        .0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, \
             kind, captured_at, camera, source, created_at) \
             VALUES (?1, 'DSC_0177.JPG', 1, '2026', 1, 'photo', \
             NULL, NULL, 'imported', '2026')",
            rusqlite::params![target.to_string_lossy()],
        )
        .unwrap();

    let manager = ai::ModelManager::new(
        models,
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );
    assert!(manager.semantic_ready(), "语义模型应就绪");

    // 逐步拆解链路，失败点在哪一步直接暴露
    let thumb = thumbs::thumb_file(&db_dir, &target, 256);
    eprintln!("step1 thumb_file: ok={}", thumb.is_some());
    assert!(thumb.is_some(), "256 档缩略图生成失败");

    let v = manager.embed_image(&target, &db_dir);
    match &v {
        Ok(vec) => eprintln!("step2 embed_image: ok dim={}", vec.len()),
        Err(e) => eprintln!("step2 embed_image: Err={e}"),
    }
    assert!(v.is_ok(), "embed_image 失败");
    assert_eq!(v.as_ref().unwrap().len(), ai::embed::EMBED_DIM);

    let id: i64 = database
        .0
        .query_row("SELECT id FROM assets LIMIT 1", [], |r| r.get(0))
        .unwrap();
    let index = ai::semantic::shared_index(&db_dir);
    match &index {
        Ok(_) => eprintln!("step3 shared_index: ok"),
        Err(e) => eprintln!("step3 shared_index: Err={e}"),
    }
    let index = index.unwrap();
    {
        let idx = index.lock().unwrap();
        let add = idx.add(id as u64, v.as_ref().unwrap());
        eprintln!("step4 usearch add: ok={}", add.is_ok());
        assert!(add.is_ok());
        let save_path = db_dir.join("vectors.usearch");
        let save = idx.save(save_path.to_str().unwrap());
        // 已知行为（semantic.rs runtime_vectors_path 的存在理由）：usearch
        // Windows 窄字符串路径对中文目录 save 报错——生产走 save_index 的
        // ASCII 镜像。此处仅记录不判死（本诊断的价值在 step6 全链路）。
        eprintln!(
            "step5 usearch 原生 save ({}): ok={} exists={}（预期失败，生产经 save_index 镜像绕开）",
            save_path.display(),
            save.is_ok(),
            save_path.is_file()
        );
        let _ = save;
    }

    // 全链路：run_semantic_backfill（含任务建账/消费/记账）
    let done = ai::semantic::run_semantic_backfill(
        &db_dir,
        std::sync::Arc::new(manager),
        &events::EventBus::new(),
        1,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    eprintln!("step6 run_semantic_backfill done={done}");
    assert!(done >= 1, "回填应成功至少 1 条（中文库路径）");
}

// ---------------------------------------------------------------------------
// 真 smoke（#[ignore]）：需先通过设置页/ai_model_download 下载三件套到
// %APPDATA%\com.smartphoto.app\models\。手动跑：
// cargo test --test ai_embed_test real -- --ignored --nocapture
// ---------------------------------------------------------------------------

fn models_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var("APPDATA").ok()?;
    let dir = std::path::PathBuf::from(base)
        .join("com.smartphoto.app")
        .join("models");
    dir.is_dir().then_some(dir)
}

/// 真实模型推理 smoke：视觉 768 维嵌入 + 文本查询确定性。
#[test]
#[ignore = "可选 smoke：需已下载 SigLIP2 三件套；交付验证由真机回归执行"]
fn real_embed_image_and_text() {
    let Some(models) = models_dir() else {
        eprintln!("skip: 模型目录不存在（先下载模型）");
        return;
    };
    if !models.join("siglip2-visual.onnx").is_file() {
        eprintln!("skip: siglip2-visual 未下载");
        return;
    }
    let manager = ai::ModelManager::new(
        models,
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );

    // 真实 NEF 的 256 档缩略图 → embed
    let nef = Path::new(r"I:\SmartPhoto-test-e2e\收纳\2025\06-07\DSC_0176.NEF");
    if !nef.is_file() {
        eprintln!("skip: 样例不存在 {}", nef.display());
        return;
    }
    let lib = tempfile::tempdir().unwrap();
    let v = manager.embed_image(nef, lib.path()).expect("视觉嵌入");
    assert_eq!(v.len(), ai::embed::EMBED_DIM, "输出维度 768");
    let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    assert!((norm - 1.0).abs() < 1e-3, "归一化模长≈1: {norm}");

    // 中英文查询（观察用打印 + 维度/确定性断言）
    for q in [
        "海边日落",
        "a cat sleeping on a sofa",
        "城市夜景车流",
        "雪山上的人们",
        "food on a plate",
        "花卉特写",
    ] {
        let t = manager.embed_text(q).expect("文本嵌入");
        assert_eq!(t.len(), 768);
        let t2 = manager.embed_text(q).expect("文本嵌入重复");
        let dot: f32 = t.iter().zip(&t2).map(|(a, b)| a * b).sum();
        assert!(dot > 0.999, "同文本嵌入应确定性: {q} dot={dot}");
        let cos: f32 = t.iter().zip(&v).map(|(a, b)| a * b).sum();
        eprintln!("query={q:?} cos(文本, NEF图)={cos:.4}");
    }

    // 差异化：同查询对两张不同照片的 cos 必须不同（嵌入有区分度）
    let jpg2 = nef.with_file_name("DSC_0177.JPG");
    if jpg2.is_file() {
        let v2 = manager.embed_image(&jpg2, lib.path()).expect("第二张嵌入");
        let d_same: f32 = v.iter().zip(&v).map(|(a, b)| a * b).sum();
        let d_cross: f32 = v.iter().zip(&v2).map(|(a, b)| a * b).sum();
        eprintln!("cos(图,图自身)={d_same:.4} cos(图,DSC_0177)={d_cross:.4}");
        assert!(d_same > d_cross, "自身相似度应高于他图");
    }
}

/// 真实单资产端到端回填：覆盖任务创建、缩略图、视觉推理、usearch 落盘和
/// SQLite 记账。与上面的纯推理 smoke 分开，便于定位“模型可运行但索引全败”。
#[test]
#[ignore = "可选 smoke：需已下载 SigLIP2 三件套和本机样例照片"]
fn real_semantic_backfill_one_asset() {
    let Some(models) = models_dir() else {
        eprintln!("skip: 模型目录不存在（先下载模型）");
        return;
    };
    let source = Path::new(r"I:\SmartPhoto-test-e2e\收纳\2025\06-07\DSC_0176.NEF");
    if !source.is_file() {
        eprintln!("skip: 样例不存在 {}", source.display());
        return;
    }
    let manager = ai::ModelManager::new(
        models,
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );
    let lib = tempfile::tempdir().unwrap();
    let database = common::open_db(lib.path());
    database
        .0
        .execute(
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, ?1, 'DSC_0176.NEF', 1, '2026', 1, 'raw', \
             NULL, NULL, 'imported', '2026')",
            rusqlite::params![source.to_string_lossy()],
        )
        .unwrap();

    let done = ai::semantic::run_semantic_backfill(
        lib.path(),
        std::sync::Arc::new(manager),
        &events::EventBus::new(),
        2,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    assert_eq!(done, 1, "真实单资产回填应成功");
    let indexed: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE ai_indexed_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(indexed, 1);
    assert!(lib.path().join("vectors.usearch").is_file());
}

/// 1 万规模正确性验证（随机向量插入 + KNN 自检索；性能验证留 M8 真库实测）。
#[test]
fn hnsw_10k_insert_knn_correctness() {
    let lib = tempfile::tempdir().unwrap();
    let index = ai::semantic::shared_index(lib.path()).unwrap();
    let n = 10_000usize;
    {
        let idx = index.lock().unwrap();
        idx.reserve(n).unwrap();
        for key in 0..n as u64 {
            idx.add(
                key,
                &StubEmbedder::vector(key.wrapping_mul(2685821657736338717)),
            )
            .unwrap();
        }
    }
    // KNN 自检索：以 8 个存储向量为查询，最近邻必须是自身
    let idx = index.lock().unwrap();
    for probe in 0..8u64 {
        let q = StubEmbedder::vector(probe.wrapping_mul(2685821657736338717));
        let matches = idx.search(&q, 1).unwrap();
        assert_eq!(matches.keys[0], probe, "KNN 最近邻应为自身");
        assert!(
            matches.distances[0] < 1e-5,
            "自检索距离应≈0: {}",
            matches.distances[0]
        );
    }
}

/// 阈值合成（真机修复 2026-09-20「进哪个智能相册都是全部照片」根因）：
/// 显式参数 > 设置值；不传参数必须回落设置项——此前 None 直通检索层，
/// 119 张库 limit=100 时任何查询都返回全库。
/// 2026-09-28 三档画质：设置值改 Option（null = auto 随模型变体）——
/// null 时按档位默认（int8/fp16 同为标定值 0.09，见 semantic_default_min_score）。
#[test]
fn semantic_min_score_priority() {
    use common::ipc;
    let normal = ai::QualityTier::Normal;
    let accurate = ai::QualityTier::Accurate;
    // 设置显式值：自定义 > 一切
    assert_eq!(
        ipc::ai::effective_min_score(None, Some(0.15), normal),
        Some(0.15)
    );
    assert_eq!(
        ipc::ai::effective_min_score(Some(0.2), Some(0.15), normal),
        Some(0.2)
    );
    // 显式 0 = 用户明确要求不过滤，不能被设置值覆盖
    assert_eq!(
        ipc::ai::effective_min_score(Some(0.0), Some(0.15), normal),
        Some(0.0)
    );
    // null = auto：按当前档位语义模型变体取默认
    assert_eq!(
        ipc::ai::effective_min_score(None, None, normal),
        Some(ai::semantic::semantic_default_min_score(normal))
    );
    assert_eq!(
        ipc::ai::effective_min_score(None, None, accurate),
        Some(ai::semantic::semantic_default_min_score(accurate))
    );
    // 默认档位 = normal
    assert_eq!(ai::QualityTier::default(), normal);
    assert_eq!(settings::AiSettings::default().semantic_min_score, None);
    assert_eq!(settings::AiSettings::default().quality_tier, "normal");
}

/// 回归（真机 2026-09-20 事故）：vectors.usearch 已存在且池内持有搜索侧
/// mmap view（只读）时，回填 worker 旧实现复用同一句柄 add →
/// "Can't add to an immutable index"（71 条任务三连败全灭）。
/// 修复：写路径独立 writable_index（load 全量可变）。此测试锁定该序列。
/// 回归（真机 2026-09-20 事故）：vectors.usearch 已存在且池内持有搜索侧
/// mmap view（只读）时，回填 worker 旧实现复用同一句柄 add →
/// "Can't add to an immutable index"（71 条任务三连败全灭）。
/// 修复：写路径独立 writable_index（load 全量可变）。此测试锁定该序列。
#[test]
fn backfill_survives_pooled_immutable_view() {
    let lib = tempfile::tempdir().unwrap();
    let database = common::open_db(lib.path());
    // 真实 JPEG 资产（process_ai_task 先生成 256 档缩略图——假路径过不了这关）
    let seed = |name: &str| {
        let path = lib.path().join(name);
        let img = image::RgbImage::new(64, 64);
        let mut jpg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 90);
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(encoder)
            .unwrap();
        std::fs::write(&path, jpg).unwrap();
        database
            .0
            .execute(
                "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, captured_at, \
                 camera, source, created_at) VALUES (?1, ?1, 1, '2026', 1, 'photo', \
                 NULL, NULL, 'imported', '2026')",
                rusqlite::params![path.to_string_lossy()],
            )
            .unwrap();
    };
    seed("a.jpg");
    // 第一轮回填落盘 vectors.usearch
    let done1 = ai::semantic::run_semantic_backfill(
        lib.path(),
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        1,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    assert_eq!(done1, 1, "第一轮应成功");
    assert!(lib.path().join("vectors.usearch").is_file());

    // 复现事故前置：池逐出后搜索侧重进（文件在 → mmap view 只读句柄入池）
    ai::semantic::invalidate_index(lib.path());
    let _search_handle = ai::semantic::shared_index(lib.path()).unwrap();

    // 新资产 + 回填：旧实现在此处拿 view add 报 immutable
    seed("b.jpg");
    let done2 = ai::semantic::run_semantic_backfill(
        lib.path(),
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        1,
        "siglip2-visual", // 档位解析：stub 嵌入器不感知模型，标签只影响批决策
    );
    assert_eq!(done2, 1, "池内 view 存在时回填必须照常成功（可写句柄路径）");
    let idx = ai::semantic::shared_index(lib.path()).unwrap();
    let locked = idx.lock().unwrap();
    assert_eq!(locked.size(), 2, "两轮向量都应在索引里");
}

/// 显示分数标定（2026-09-21 真库实测定案）：SigLIP2 统一重嵌入后 cos 压缩
/// 在 0.04-0.125 窄带，原始分直接当百分比显示会把 top 命中显示成「12%」。
/// 线性拉伸锚点锁定 semantic.rs 常量；阈值过滤保持原始分（0.09 工作点）。
/// fp16 变体带（2026-09-28）：0.0-0.06，工作点 0.03 显示 50%。
#[test]
fn semantic_display_calibration_anchors() {
    let f = |raw| ai::semantic::calibrated_display_score(raw, ai::QualityTier::Normal);
    // 边界：floor 归零、ceiling 饱和，两侧钳制
    assert_eq!(f(ai::semantic::SEMANTIC_SCORE_FLOOR), 0.0);
    assert!((f(ai::semantic::SEMANTIC_SCORE_CEILING) - 1.0).abs() < 1e-6);
    assert_eq!(f(0.0), 0.0);
    assert_eq!(f(0.5), 1.0);
    // 真库锚点（真机采样值）：top 命中 0.1187 → ~93%；无关内容 top
    // 0.0957（「无人机航拍」）→ ~66%；长尾地板 0.0558 → ~19%
    assert!((f(0.1187) - 0.926).abs() < 0.005);
    assert!((f(0.0957) - 0.655).abs() < 0.005);
    assert!((f(0.0558) - 0.186).abs() < 0.005);
    // 单调性：拉伸后排序不变（前端徽标与排序一致性依赖此性质）
    assert!(f(0.10) > f(0.09) && f(0.09) > f(0.08));

    // fp16 变体（accurate 档）：负分归 0；工作点 0.03 → 50%；日落锚点
    // 0.0463 → ~77%（同位观感与 int8 档对齐）
    let g = |raw| ai::semantic::calibrated_display_score(raw, ai::QualityTier::Accurate);
    assert_eq!(g(-0.03), 0.0);
    assert!((g(0.03) - 0.5).abs() < 1e-6);
    assert!((g(0.0463) - 0.772).abs() < 0.005);
    assert!(g(0.04) > g(0.035) && g(0.035) > g(0.03));
}

// ---------------------------------------------------------------------------
// DML 毒化位按模型隔离（2026-09-28）：某模型 DML 运行时故障只毒化该模型，
// 其他模型（scrfd/arcface/siglip vision/text）保住 DML
// ---------------------------------------------------------------------------

/// env 覆盖（基准/诊断）会改写 dml_intended 真值——被覆盖的进程跳过真值表。
fn ep_overridden() -> bool {
    std::env::var("SMARTPHOTO_AI_EP").is_ok()
}

#[test]
fn dml_poison_isolated_per_model() {
    if ep_overridden() {
        eprintln!("skip: SMARTPHOTO_AI_EP 已设置，真值表仅在无覆盖时成立");
        return;
    }
    // 毒化位是进程级全局——与 fallback 用例并发会互相改写彼此的断言
    // 前置（真值表必须独占运行），用锁把两个用例串行化。
    static SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
    ai::reset_dml_poison_for_test();
    // 基线：use_gpu=true → 各模型 DML 优先；false → 全 CPU
    for m in ["scrfd", "arcface", "siglip2-visual", "siglip2-text"] {
        assert!(ai::dml_intended_for_test(true, m), "{m} 基线应 DML 优先");
        assert!(!ai::dml_intended_for_test(false, m), "{m} 关 GPU 应纯 CPU");
    }
    // scrfd 毒化 → 只 scrfd 落 CPU，其余保住 DML
    ai::poison_dml_for_test("scrfd");
    assert!(!ai::dml_intended_for_test(true, "scrfd"), "scrfd 已毒化");
    for m in ["arcface", "siglip2-visual", "siglip2-text"] {
        assert!(ai::dml_intended_for_test(true, m), "{m} 不应被 scrfd 连坐");
    }
    // 再毒化 siglip vision → 文本塔仍 DML
    ai::poison_dml_for_test("siglip2-visual");
    assert!(!ai::dml_intended_for_test(true, "siglip2-visual"));
    assert!(
        ai::dml_intended_for_test(true, "siglip2-text"),
        "双塔各自独立"
    );
    // 复位 → 全部恢复 DML
    ai::reset_dml_poison_for_test();
    for m in ["scrfd", "arcface", "siglip2-visual", "siglip2-text"] {
        assert!(ai::dml_intended_for_test(true, m), "{m} 复位后应恢复");
    }
}

#[test]
fn dml_fallback_poisons_only_failing_model_and_recovers_once() {
    if ep_overridden() {
        eprintln!("skip: SMARTPHOTO_AI_EP 已设置");
        return;
    }
    static SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = SERIALIZE.lock().unwrap_or_else(|e| e.into_inner());
    ai::reset_dml_poison_for_test();

    // 模型 A（arcface）首跑失败 → 毒化 A + reset（丢 DML 会话）+ 重跑一次成功
    let mut calls = 0;
    let mut resets = 0;
    let out = ai::run_with_dml_fallback_for_test(
        true,
        "arcface",
        || {
            calls += 1;
            if calls == 1 {
                Err("E_INVALIDARG".into())
            } else {
                Ok(7u32)
            }
        },
        || resets += 1,
    );
    assert_eq!(out, Ok(7));
    assert_eq!(
        (calls, resets),
        (2, 1),
        "失败一次 → 重跑一次 + 会话重建一次"
    );
    assert!(
        !ai::dml_intended_for_test(true, "arcface"),
        "arcface 已毒化落 CPU"
    );

    // 其他模型不受连坐：scrfd 单跑成功（run 恰一次、零 reset）
    let mut scrfd_calls = 0;
    let ok = ai::run_with_dml_fallback_for_test(
        true,
        "scrfd",
        || {
            scrfd_calls += 1;
            Ok(1u32)
        },
        || panic!("scrfd 不应重建会话"),
    );
    assert_eq!(ok, Ok(1));
    assert_eq!(scrfd_calls, 1, "scrfd 保住 DML，单跑即成功");

    // 已毒化模型再失败：不再二次重建（run 恰一次），错误原样上抛
    let mut again = 0;
    let err = ai::run_with_dml_fallback_for_test(
        true,
        "arcface",
        || {
            again += 1;
            Err::<u32, _>("boom".into())
        },
        || panic!("已落 CPU 不应再 reset"),
    );
    assert!(err.is_err(), "二次失败原样上抛");
    assert_eq!(again, 1, "非 DML 会话不重跑");
    ai::reset_dml_poison_for_test();
}
