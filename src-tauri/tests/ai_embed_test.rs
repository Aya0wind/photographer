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
