//! 语义嵌入与向量检索（M4）：确定性随机向量桩验证 HNSW 索引/检索/
//! 持久化/过滤契约；#[ignore] 真 smoke（真实模型 + 真实 NEF/照片）与
//! 20 万规模 benchmark。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
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
                "INSERT OR IGNORE INTO assets (id, path, filename, size, mtime, xxhash, sha256,                  kind, captured_at, camera, source, created_at)                  VALUES (?1, ?2, 'x.jpg', 1, '2026', 1, x'00', 'photo', NULL, NULL, 'imported', '2026')",
                rusqlite::params![id as i64, format!("X:/p/{id}.jpg")],
            )
            .unwrap();
    }
    database
        .0
        .execute(
            "INSERT OR IGNORE INTO assets (id, path, filename, size, mtime, xxhash, sha256,              kind, captured_at, camera, source, created_at)              VALUES (10000, 'X:/p/near.jpg', 'near.jpg', 1, '2026', 1, x'00', 'photo', NULL, NULL, 'imported', '2026')",
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
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, sha256, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, 'X:/p/1.jpg', '1.jpg', 1, '2026', 1, x'00', 'photo', \
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
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, sha256, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, ?1, 'source.jpg', 1, '2026', 1, x'00', 'photo', \
             NULL, NULL, 'imported', '2026')",
            rusqlite::params![source.to_string_lossy()],
        )
        .unwrap();

    let done = ai::semantic::run_semantic_backfill(
        &lib,
        std::sync::Arc::new(StubEmbedder),
        &events::EventBus::new(),
        1,
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
    );
    assert_eq!(replayed, 1);
}

/// worker 封顶契约：AI 推理 worker ≤ 4（ort 会话线程重，多 worker × 全核
/// 会话平方级超订阅——「在跑但极慢」的预防性约束）。
#[test]
fn ai_worker_count_capped() {
    let n = ai::semantic::worker_count_for_ai();
    assert!((1..=4).contains(&n), "AI worker 应封顶 4，实际 {n}");
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
            "INSERT INTO assets (path, filename, size, mtime, xxhash, sha256, \
             kind, captured_at, camera, source, created_at) \
             VALUES (?1, 'DSC_0177.JPG', 1, '2026', 1, x'00', 'photo', \
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
        eprintln!(
            "step5 usearch save ({}): ok={} exists={}",
            save_path.display(),
            save.is_ok(),
            save_path.is_file()
        );
        assert!(save.is_ok(), "usearch 保存中文路径失败");
    }

    // 全链路：run_semantic_backfill（含任务建账/消费/记账）
    let done = ai::semantic::run_semantic_backfill(
        &db_dir,
        std::sync::Arc::new(manager),
        &events::EventBus::new(),
        1,
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
            "INSERT INTO assets (id, path, filename, size, mtime, xxhash, sha256, \
             kind, captured_at, camera, source, created_at) \
             VALUES (1, ?1, 'DSC_0176.NEF', 1, '2026', 1, x'00', 'raw', \
             NULL, NULL, 'imported', '2026')",
            rusqlite::params![source.to_string_lossy()],
        )
        .unwrap();

    let done = ai::semantic::run_semantic_backfill(
        lib.path(),
        std::sync::Arc::new(manager),
        &events::EventBus::new(),
        2,
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
#[test]
fn semantic_min_score_priority() {
    use common::ipc;
    assert_eq!(ipc::ai::effective_min_score(None, 0.09), Some(0.09));
    assert_eq!(ipc::ai::effective_min_score(Some(0.2), 0.09), Some(0.2));
    // 显式 0 = 用户明确要求不过滤，不能被设置值覆盖
    assert_eq!(ipc::ai::effective_min_score(Some(0.0), 0.09), Some(0.0));
    assert!((settings::AiSettings::default().semantic_min_score - 0.09).abs() < 1e-6);
}
