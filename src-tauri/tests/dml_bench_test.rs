//! GPU（DirectML）vs CPU 嵌入吞吐基准（2026-09-21，#[ignore] 手动跑）：
//! 真库已缓存 256 档缩略图批量过 embed_image / embed_images，分别记录
//! 单图与批量的总耗时/张均耗时；SCRFD 人脸检测顺带一组。
//!
//! 用法（EP 由生产覆盖 env 控制，每次进程一种 EP）：
//! ```text
//! SMARTPHOTO_AI_EP=cpu  cargo test --test dml_bench_test -- --ignored --nocapture
//! SMARTPHOTO_AI_EP=dml  cargo test --test dml_bench_test -- --ignored --nocapture
//! # 可调：SMARTPHOTO_BENCH_N（默认 64）/ SMARTPHOTO_BENCH_BATCH（默认 16）
//! #      / SMARTPHOTO_BENCH_WORKERS（默认 1，>1 = 并发 worker 各自组批，
//! #      推理经全局会话互斥串行——评估互斥是否瓶颈）
//! ```
//! 数据定策（结论见 ai/mod.rs execution_providers 与 semantic.rs AI_BATCH）：
//! DML 快/持平 → 默认开（use_gpu=true 已是默认）；明显慢 → 默认仍 CPU。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::PathBuf;
use std::time::Instant;

use ai::embed::SemanticEmbedder;
use ai::ModelManager;
use events::EventBus;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 收集基准输入：真库已缓存 256 档缩略图列表（推理输入与生产同形态）。
/// 计时区绝不混入缩略图生成/解码原图时间——warmup 全量预生成缓存。
fn bench_inputs() -> Option<Vec<PathBuf>> {
    let thumbs_dir = PathBuf::from(env_or(
        "SMARTPHOTO_BENCH_THUMBS",
        r"I:\SmartPhoto\主库\thumbs\256",
    ));
    if !thumbs_dir.is_dir() {
        eprintln!("skip: 缩略图目录不存在 {}", thumbs_dir.display());
        return None;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&thumbs_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("jpg")))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("skip: 缩略图目录为空");
        return None;
    }
    Some(files)
}

fn setup_manager() -> Option<ModelManager> {
    let models = PathBuf::from(env_or(
        "SMARTPHOTO_BENCH_MODELS",
        r"C:\Users\12003\AppData\Roaming\com.smartphoto.app\models",
    ));
    let manager = ModelManager::new(
        models.clone(),
        EventBus::new(),
        tasks::TaskSupervisor::new(EventBus::new()),
    );
    if !manager.semantic_models_ready() {
        eprintln!("skip: 语义模型未齐备（{}）", models.display());
        return None;
    }
    manager.set_ai_params(ai::AiIndexParams {
        use_gpu: true, // 实际 EP 由 SMARTPHOTO_AI_EP 覆盖决定
        ..ai::AiIndexParams::default()
    });
    Some(manager)
}

fn report(phase: &str, n: usize, elapsed_ms: f64) {
    let per = if n > 0 { elapsed_ms / n as f64 } else { 0.0 };
    let tput = if elapsed_ms > 0.0 {
        n as f64 * 1000.0 / elapsed_ms
    } else {
        0.0
    };
    eprintln!(
        "BENCH | {phase:<26} | n={n:<4} | total={elapsed_ms:>9.1}ms | ms/img={per:>7.2} | img/s={tput:>7.1}"
    );
}

fn norm_ok(v: &[f32]) -> bool {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    (n - 1.0).abs() < 1e-2 && v.iter().all(|x| x.is_finite())
}

#[test]
#[ignore = "真机基准：需真库缩略图 + 已下载模型，手动 --ignored 运行"]
fn embed_throughput_cpu_vs_dml() {
    let Some(files) = bench_inputs() else {
        return;
    };
    let Some(manager) = setup_manager() else {
        return;
    };
    // 缓存隔离：tempdir 作 db_dir（thumb_file 的缓存根），不污染真库
    let cache_dir = tempfile::tempdir().unwrap();
    let db_dir = cache_dir.path().to_path_buf();
    let n_total: usize = env_or("SMARTPHOTO_BENCH_N", "64").parse().unwrap_or(64);
    let batch: usize = env_or("SMARTPHOTO_BENCH_BATCH", "16").parse().unwrap_or(16);
    let workers: usize = env_or("SMARTPHOTO_BENCH_WORKERS", "1").parse().unwrap_or(1);
    // 输入循环取用（同一文件可重复，模拟真实 256 JPEG 输入）
    let inputs: Vec<PathBuf> = (0..n_total)
        .map(|i| files[i % files.len()].clone())
        .collect();
    let ep = env_or("SMARTPHOTO_AI_EP", "auto");
    eprintln!(
        "== 基准启动：ep={ep} n={n_total} batch={batch} workers={workers} thumbs={} cores={:?} ==",
        files.len(),
        std::thread::available_parallelism().map(|n| n.get())
    );

    // —— 预跑：全量预生成缩略图缓存（不计时）+ 会话就绪 + 向量健全性 ——
    for p in inputs.iter().take(4) {
        let v = manager.embed_image(p, &db_dir).expect("warmup embed");
        assert!(norm_ok(&v), "向量未归一化/含 NaN");
    }
    for chunk in inputs.chunks(batch.max(1)) {
        let _ = manager.embed_images(chunk, &db_dir).expect("cache warm");
    }

    // —— ① 单图逐次（生产旧路径形态）——
    let t0 = Instant::now();
    let mut single_first: Option<Vec<f32>> = None;
    for p in &inputs {
        let v = manager.embed_image(p, &db_dir).expect("single embed");
        if single_first.is_none() {
            single_first = Some(v);
        }
    }
    let single_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report(&format!("single[{ep}]"), n_total, single_ms);

    // —— ② 批量（生产新路径形态：B 图一次 Run）——
    let t0 = Instant::now();
    let mut batch_first: Option<Vec<f32>> = None;
    for chunk in inputs.chunks(batch.max(1)) {
        let vs = manager.embed_images(chunk, &db_dir).expect("batch embed");
        assert_eq!(vs.len(), chunk.len());
        if batch_first.is_none() {
            batch_first = vs.first().cloned();
        }
        for v in &vs {
            assert!(norm_ok(v), "批量向量未归一化");
        }
    }
    let batch_ms = t0.elapsed().as_secs_f64() * 1000.0;
    report(&format!("batch{batch}[{ep}]"), n_total, batch_ms);

    // 同输入单图 vs 批量输出一致性：int8 动态量化按批定 scale，批内漂移
    // 是已知特性（自相似 cos 会低于 1）——只记录不做硬断言；排序影响看
    // 下面的成对相似度偏差（那才是检索语义的真指标）。
    if let (Some(a), Some(b)) = (&single_first, &batch_first) {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        eprintln!("BENCH | cos(single,batch)={dot:.6}（自相似漂移，int8 批量化特性）");
    }

    // —— ②b 成对相似度结构（批量化是否伤检索排序的真指标）：同一组图
    //    分别走单图/批嵌入，两两 cos 的最大偏差——SigLIP2 相关簇判读带
    //    ~0.05（floor 0.04 / ceiling 0.125），偏差 ≤0.01 排序基本无损 ——
    {
        let probe: Vec<PathBuf> = files.iter().take(8).cloned().collect();
        let singles: Vec<Vec<f32>> = probe
            .iter()
            .map(|p| manager.embed_image(p, &db_dir).unwrap())
            .collect();
        let batched = manager.embed_images(&probe, &db_dir).unwrap();
        let mut max_dev = 0f32;
        let mut max_pair = 0f32;
        for a_idx in 0..probe.len() {
            for b_idx in (a_idx + 1)..probe.len() {
                let cs: f32 = singles[a_idx]
                    .iter()
                    .zip(&singles[b_idx])
                    .map(|(x, y)| x * y)
                    .sum();
                let cb: f32 = batched[a_idx]
                    .iter()
                    .zip(&batched[b_idx])
                    .map(|(x, y)| x * y)
                    .sum();
                max_dev = max_dev.max((cs - cb).abs());
                max_pair = max_pair.max(cs);
            }
        }
        eprintln!(
            "BENCH | pair-cos max |single-batch| = {max_dev:.4}（组内最高相似度 {max_pair:.4}；判读带宽 ~0.05）"
        );
        // 文本→图像检索分漂移（真实查询量）：同图单图嵌入 vs 批嵌入对同一
        // 文本查询的 cos 差——生产 min_score 工作点 0.09，漂移若 ≥0.01
        // 会实质移动阈值行为
        let queries = [
            "海边的日落",
            "夜晚的城市街道",
            "一群人的合影",
            "森林里的徒步",
        ];
        let mut max_qdev = 0f32;
        for q in queries {
            let t = manager.embed_text(q).unwrap();
            for i in 0..probe.len() {
                let cs: f32 = t.iter().zip(&singles[i]).map(|(x, y)| x * y).sum();
                let cb: f32 = t.iter().zip(&batched[i]).map(|(x, y)| x * y).sum();
                max_qdev = max_qdev.max((cs - cb).abs());
            }
        }
        eprintln!(
            "BENCH | text-img score max |single-batch| = {max_qdev:.4}（min_score 工作点 0.09）"
        );
    }

    // —— ③ 并发 worker（>1 时）：各线程组批，推理经全局会话互斥串行
    //    ——评估互斥+批量化后的多 worker 扩展性（数据定 worker 数）——
    if workers > 1 {
        let t0 = Instant::now();
        let mut handles = Vec::new();
        for w in 0..workers {
            let manager = manager.clone();
            let db_dir = db_dir.clone();
            let part: Vec<PathBuf> = inputs
                .iter()
                .enumerate()
                .filter(|(i, _)| i % workers == w)
                .map(|(_, p)| p.clone())
                .collect();
            handles.push(std::thread::spawn(move || {
                for chunk in part.chunks(batch.max(1)) {
                    let vs = manager
                        .embed_images(chunk, &db_dir)
                        .expect("concurrent embed");
                    assert_eq!(vs.len(), chunk.len());
                }
            }));
        }
        let mut done = 0usize;
        for (i, h) in handles.into_iter().enumerate() {
            h.join().expect("bench worker");
            done += inputs.iter().filter(|_| i < inputs.len()).count() / workers;
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let _ = done;
        report(&format!("batch{batch}x{workers}w[{ep}]"), n_total, ms);
    }

    // —— ④ 人脸 SCRFD 检测一组（同 EP 会话；单图形态=生产形态）——
    if manager.face_models_ready() {
        let m: usize = n_total.min(32);
        let imgs: Vec<image::RgbImage> = inputs
            .iter()
            .take(m)
            .map(|p| {
                image::ImageReader::open(p)
                    .unwrap()
                    .decode()
                    .unwrap()
                    .to_rgb8()
            })
            .collect();
        let _ = ai::face::detect_faces(&manager, &imgs[0]).expect("detect warm");
        let t0 = Instant::now();
        let mut faces = 0usize;
        for img in &imgs {
            faces += ai::face::detect_faces(&manager, img).expect("detect").len();
        }
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        report(&format!("scrfd[{ep}]"), m, ms);
        eprintln!("BENCH | scrfd faces_total={faces}");
    }

    eprintln!("== 基准结束（ep={ep}）==");
}

// ---------------------------------------------------------------------------
// face 全管线吞吐（2026-09-28 性能三件套真机验收）：检测源降档 + 两阶段
// 并行 + 归一化坐标落库，与 index_rebuild(face) IPC 同核的清理+重排后计时。
// ---------------------------------------------------------------------------

/// face 全管线基准（写真库）：
/// ① rebuild_channel_core("face")——faces/people 全清 + face_indexed_at
///   复位 + 任务重排（旧 v1 像素坐标数据由此重建为 v2 归一化，幂等）
/// ② run_face_backfill 全管线计时（认领→取图[缓存档优先]→SCRFD→对齐
///   →ArcFace→在线聚类→归一化落库→记账）。
/// env：SMARTPHOTO_BENCH_LIB（默认 I:\SmartPhoto\主库）、
/// SMARTPHOTO_BENCH_MODELS、SMARTPHOTO_BENCH_N（默认 0=全部 pending；
/// 大于 0 时只保留按 id 升序的前 N 个资产的任务，其余删行——下次 kick
/// 自动补种，无损）、SMARTPHOTO_AI_EP（cpu|dml 覆盖）。
/// 手动：cargo test --test dml_bench_test face_pipeline -- --ignored --nocapture
#[test]
#[ignore = "真机验收基准：重建并计时真库人脸索引，手动 --ignored 运行"]
fn face_pipeline_throughput_real_library() {
    let lib = PathBuf::from(env_or("SMARTPHOTO_BENCH_LIB", r"I:\SmartPhoto\主库"));
    let models = PathBuf::from(env_or(
        "SMARTPHOTO_BENCH_MODELS",
        r"C:\Users\12003\AppData\Roaming\com.smartphoto.app\models",
    ));
    if !lib.is_dir() {
        eprintln!("skip: 真库不存在 {}", lib.display());
        return;
    }
    let manager = ModelManager::new(
        models.clone(),
        EventBus::new(),
        tasks::TaskSupervisor::new(EventBus::new()),
    );
    if !manager.face_models_ready() {
        eprintln!("skip: scrfd/arcface 未齐备（{}）", models.display());
        return;
    }
    manager.set_ai_params(ai::AiIndexParams {
        use_gpu: true, // 实际 EP 由 SMARTPHOTO_AI_EP 覆盖决定
        ..ai::AiIndexParams::default()
    });
    let n_cap: usize = env_or("SMARTPHOTO_BENCH_N", "0").parse().unwrap_or(0);

    // ① 重建（幂等；与 index_rebuild(face) 同核：清产物+重排，不 kick UI 侧）
    let db = ipc::open_library_db(&lib).expect("打开真库");
    let _ = ipc::indexing::rebuild_channel_core(&db, &lib, "face").expect("face 重建");
    if n_cap > 0 {
        db.0.execute(
            "DELETE FROM index_tasks WHERE kind = 'face' AND asset_id NOT IN \
             (SELECT id FROM assets WHERE kind IN ('photo','raw') ORDER BY id LIMIT ?1)",
            rusqlite::params![n_cap as i64],
        )
        .expect("截断基准任务集");
    }
    let total = db.pending_index_task_count("face").unwrap_or(0);
    let ep = env_or("SMARTPHOTO_AI_EP", "auto");
    eprintln!(
        "== face 全管线基准：lib={} ep={} pending={total}（重建后，v2 归一化坐标）==",
        lib.display(),
        ep
    );

    // ② 全管线计时（两阶段：3 worker 推理 + 单线程聚类落库）
    let t0 = Instant::now();
    let done = ai::face::run_face_backfill(&lib, std::sync::Arc::new(manager), &EventBus::new());
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    report(&format!("face-pipeline[{ep}]"), done as usize, ms);

    // 对账：落库脸数 / 簇数 / 坐标空间抽查（v2 归一化 ∈ 0..1）
    let faces: i64 =
        db.0.query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
            .unwrap_or(0);
    let people: i64 =
        db.0.query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
            .unwrap_or(0);
    let bad_boxes: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM faces WHERE box_x < 0 OR box_y < 0 OR \
             box_w <= 0 OR box_h <= 0 OR box_x + box_w > 1.01 OR box_y + box_h > 1.01",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    let leftovers: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'face' AND state != 'done'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(-1);
    eprintln!(
        "BENCH | face-pipeline faces={faces} people={people} bad_boxes={bad_boxes} leftovers={leftovers}（done={done}/{total}）"
    );
    assert_eq!(leftovers, 0, "任务应收口（failed/pending 残留=异常）");
    assert_eq!(bad_boxes, 0, "v2 归一化坐标应在 0..1");
    eprintln!("== face 全管线基准结束（ep={ep}）==");
}
