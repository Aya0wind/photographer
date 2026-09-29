//! 三档画质（快速/普通/精准，2026-09-28 定案）后端契约：
//! - 档位解析真值表（三档 × 检测模型 id / 检测源策略 / 语义件组）
//! - 指纹联动矩阵（fast↔normal 只重建 face；normal↔accurate 重建
//!   face+semantic；normal 档与升级前旧指纹逐位一致——不误重建存量库）
//! - ready 判定随当前档位（缺当前档位件 false，切回已装档位恢复）
//! - detection_source accurate 分支（2048 优先：命中缓存零生成 / 未命中
//!   同步生成 2048；512 只做兜底）
//! - 清单/状态序列化携带 tier 字段（camelCase）
//! - 阈值 auto 默认值（int8=0.09 / fp16=标定值）与 settings 迁移见
//!   settings_test / ai_embed_test。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::path::Path;
use std::time::Duration;

use ai::QualityTier;
use ipc::indexing::{face_params_fingerprint, semantic_params_fingerprint};

fn tier_settings(tier: &str) -> settings::AiSettings {
    settings::AiSettings {
        enable_clip: true,
        enable_face: true,
        quality_tier: tier.to_string(),
        ..settings::AiSettings::default()
    }
}

// ---------------------------------------------------------------------------
// 档位解析真值表
// ---------------------------------------------------------------------------

#[test]
fn tier_resolution_truth_table() {
    // settings 字符串 ↔ 枚举
    for (s, t) in [
        ("fast", QualityTier::Fast),
        ("normal", QualityTier::Normal),
        ("accurate", QualityTier::Accurate),
    ] {
        assert_eq!(QualityTier::from_setting(s), Some(t));
        assert_eq!(t.as_str(), s);
    }
    assert_eq!(QualityTier::from_setting("turbo"), None, "非法档位 None");
    assert_eq!(QualityTier::default(), QualityTier::Normal, "默认 normal");

    // 人脸检测模型：fast 换 10G 小件，normal/accurate 共用 34G 件
    assert_eq!(ai::face_detect_model_id(QualityTier::Fast), "scrfd-10g");
    assert_eq!(ai::face_detect_model_id(QualityTier::Normal), "scrfd");
    assert_eq!(ai::face_detect_model_id(QualityTier::Accurate), "scrfd");

    // 语义件组：fast/normal 共用 int8（互相切换不动语义索引）；accurate 换 fp16
    assert_eq!(
        ai::semantic_model_ids(QualityTier::Fast),
        ["siglip2-visual", "siglip2-text"]
    );
    assert_eq!(
        ai::semantic_model_ids(QualityTier::Normal),
        ["siglip2-visual", "siglip2-text"]
    );
    assert_eq!(
        ai::semantic_model_ids(QualityTier::Accurate),
        ["siglip2-visual-fp16", "siglip2-text-fp16"]
    );

    // 解析结果与清单条目对齐（id 必须真实存在于 catalog）
    let ids: Vec<&str> = ai::catalog().iter().map(|m| m.id.as_str()).collect();
    for tier in [
        QualityTier::Fast,
        QualityTier::Normal,
        QualityTier::Accurate,
    ] {
        assert!(ids.contains(&ai::face_detect_model_id(tier)));
        for id in ai::semantic_model_ids(tier) {
            assert!(ids.contains(&id), "{id} 必须在清单内");
        }
    }
}

// ---------------------------------------------------------------------------
// 指纹联动矩阵
// ---------------------------------------------------------------------------

/// 升级前的旧指纹公式（2026-09-27 及之前）：normal 档必须与之逐位一致，
/// 否则升级即全库误重建（真机事故 2026-09-20 同族风险）。
fn legacy_semantic_fingerprint(ai_settings: &settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai_settings.index_params_version as u64,
        u64::from(ai_settings.embed_input_size),
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn legacy_face_fingerprint(ai_settings: &settings::AiSettings) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for part in [
        ai_settings.index_params_version as u64,
        u64::from(ai_settings.embed_input_size),
        ai_settings.face_detect_threshold.to_bits() as u64,
        ai_settings.face_cluster_threshold.to_bits() as u64,
    ] {
        hash ^= part;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[test]
fn fingerprint_linkage_matrix() {
    let (fast, normal, accurate) = (
        tier_settings("fast"),
        tier_settings("normal"),
        tier_settings("accurate"),
    );

    // 升级兼容：normal（默认档）与旧公式逐位一致——存量 marker 命中，
    // 不触发重建；fast 的语义档与 normal 同件 → 语义指纹也一致
    assert_eq!(
        semantic_params_fingerprint(&normal),
        legacy_semantic_fingerprint(&normal),
        "normal 语义指纹必须与升级前一致"
    );
    assert_eq!(
        semantic_params_fingerprint(&fast),
        semantic_params_fingerprint(&normal),
        "fast↔normal 语义同件（int8）→ 指纹不变 → 不重建语义"
    );
    assert_ne!(
        semantic_params_fingerprint(&accurate),
        semantic_params_fingerprint(&normal),
        "normal→accurate 换 fp16 件 → 语义指纹必变 → 重建语义"
    );
    assert_ne!(
        face_params_fingerprint(&normal),
        legacy_face_fingerprint(&normal),
        "人脸质量规则升级必须重建旧的低质量归类"
    );

    // 联动矩阵：切换方向 × 受影响通道
    // fast→normal：只 face（换回 34G 检测件）
    assert_ne!(
        face_params_fingerprint(&fast),
        face_params_fingerprint(&normal),
        "fast↔normal 换检测件 → face 指纹必变 → 重建 face"
    );
    // normal→accurate：只 semantic（fp16 双塔）；人脸同件同源零重算
    //（2026-09-28 用户定规：检测源三档统一，2048 优先策略退役）
    assert_eq!(
        face_params_fingerprint(&accurate),
        face_params_fingerprint(&normal),
        "normal↔accurate 人脸同件同源 → 指纹不变 → 不重建 face"
    );
    // fast→accurate：face（换回 34G 检测件）+ semantic（fp16）
    assert_ne!(
        face_params_fingerprint(&accurate),
        face_params_fingerprint(&fast),
        "fast→accurate 检测件变 → face 指纹必变"
    );
    // 人脸指纹分两簇：fast 独立（换检测件）；normal=accurate 同簇
    //（2026-09-28 检测源统一后同件同源，切档零重算人脸）
    let mut faces = vec![
        face_params_fingerprint(&fast),
        face_params_fingerprint(&normal),
    ];
    faces.sort_unstable();
    faces.dedup();
    assert_eq!(faces.len(), 2, "fast 与 normal/accurate 互异");
    assert_eq!(
        face_params_fingerprint(&accurate),
        face_params_fingerprint(&normal),
        "normal=accurate 人脸同簇"
    );
}

// ---------------------------------------------------------------------------
// ready 判定随当前档位
// ---------------------------------------------------------------------------

/// 在 models 根下放假件（ready 只看 is_file）。
fn install_fake_models(models_dir: &Path, ids: &[&str]) {
    std::fs::create_dir_all(models_dir).unwrap();
    for id in ids {
        std::fs::write(models_dir.join(format!("{id}.onnx")), b"fake").unwrap();
    }
}

#[test]
fn ready_follows_current_tier_and_recovers_on_switch_back() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let src = tempfile::tempdir().unwrap();
    let state = common::state_with_library(&db_dir, src.path(), Duration::from_millis(1));
    let models = db_dir.join("models");

    // 只装 normal 档件（int8 双塔 + tokenizer + scrfd + arcface）
    install_fake_models(
        &models,
        &[
            "siglip2-visual",
            "siglip2-text",
            "siglip2-tokenizer",
            "scrfd",
            "arcface",
        ],
    );

    let set_tier = |tier: QualityTier| {
        state.ai.set_ai_params(ai::AiIndexParams {
            quality_tier: tier,
            ..ai::AiIndexParams::default()
        });
    };

    set_tier(QualityTier::Normal);
    assert!(state.ai.semantic_ready(), "normal 件齐 → 语义 ready");
    assert!(state.ai.face_models_ready(), "normal 件齐 → 人脸 ready");

    // 切 fast：scrfd-10g 缺 → 人脸 not ready；语义仍 int8 → ready
    set_tier(QualityTier::Fast);
    assert!(
        !state.ai.face_models_ready(),
        "缺 scrfd-10g → 人脸 not ready"
    );
    assert!(state.ai.semantic_ready(), "fast 语义同 int8 → 仍 ready");

    // 切 accurate：fp16 双塔缺 → 语义 not ready；人脸同 34G 件 → ready
    set_tier(QualityTier::Accurate);
    assert!(!state.ai.semantic_ready(), "缺 fp16 双塔 → 语义 not ready");
    assert!(state.ai.face_models_ready(), "accurate 人脸同件 → ready");

    // 切回已装档位恢复
    set_tier(QualityTier::Normal);
    assert!(state.ai.semantic_ready());
    assert!(state.ai.face_models_ready());

    // 装齐 fast/accurate 件后全部档位 ready
    install_fake_models(
        &models,
        &["scrfd-10g", "siglip2-visual-fp16", "siglip2-text-fp16"],
    );
    for tier in [
        QualityTier::Fast,
        QualityTier::Normal,
        QualityTier::Accurate,
    ] {
        set_tier(tier);
        assert!(state.ai.semantic_ready(), "{:?} 语义 ready", tier);
        assert!(state.ai.face_models_ready(), "{:?} 人脸 ready", tier);
    }
}

// ---------------------------------------------------------------------------
// detection_source accurate 分支（2048 优先）
// ---------------------------------------------------------------------------

fn write_jpeg(dir: &Path, name: &str, w: u32, h: u32) -> std::path::PathBuf {
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        image::Rgb([((x * 7) % 256) as u8, ((y * 13) % 256) as u8, 128])
    });
    let path = dir.join(name);
    img.save_with_format(&path, image::ImageFormat::Jpeg)
        .unwrap();
    path
}

fn tier_dir_of(path: &str) -> String {
    Path::new(path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn tier_file_count(db_dir: &Path, tier: &str) -> usize {
    std::fs::read_dir(db_dir.join("thumbs").join(tier))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0)
}

#[test]
fn detection_source_unified_across_tiers_no_2048_generation() {
    // 2026-09-28 用户定规：检测源三档统一（缓存优先 [512,2048]，全未命中
    // 生成最便宜 512）；2048 优先策略退役——detection_source_tiered 已删。
    let src_dir = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();

    // ① 全未命中 → 生成 512（不再为任何档位同步生成 2048）
    let a = write_jpeg(src_dir.path(), "a.jpg", 800, 600);
    let got = ai::face::detection_source(db_dir.path(), &a).expect("全未命中应生成 512");
    assert_eq!(tier_dir_of(&got), "512", "三档统一落 512 档: {got}");

    // ② 仅 512 缓存 → 命中直返零生成
    let b = write_jpeg(src_dir.path(), "b.jpg", 800, 600);
    assert!(thumbs::thumb_file(db_dir.path(), &b, 512).is_some());
    let before = tier_file_count(db_dir.path(), "512");
    let got = ai::face::detection_source(db_dir.path(), &b).expect("512 命中");
    assert_eq!(tier_dir_of(&got), "512");
    assert_eq!(tier_file_count(db_dir.path(), "512"), before);

    // ③ 2048 命中缓存 → 直接用（零生成）
    let c = write_jpeg(src_dir.path(), "c.jpg", 800, 600);
    assert!(thumbs::thumb_file(db_dir.path(), &c, 2048).is_some());
    let before512 = tier_file_count(db_dir.path(), "512");
    let before2048 = tier_file_count(db_dir.path(), "2048");
    let got = ai::face::detection_source(db_dir.path(), &c).expect("2048 命中");
    assert_eq!(tier_dir_of(&got), "2048");
    assert_eq!(tier_file_count(db_dir.path(), "512"), before512);
    assert_eq!(tier_file_count(db_dir.path(), "2048"), before2048);

    // ④ 源不存在 → None
    let ghost = src_dir.path().join("ghost.jpg");
    assert!(ai::face::detection_source(db_dir.path(), &ghost).is_none());
}

// ---------------------------------------------------------------------------
// 切档自动重建（kick 的通道集合）
// ---------------------------------------------------------------------------

fn asset_row(path: &str) -> db::AssetRow {
    db::AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-28T00:00:00.000Z".into(),
        xxhash: 1,
        kind: events::AssetKind::Photo,
        captured_at: Some("2026-09-01T10:00:00.000Z".into()),
        camera: Some("Sony A7M4".into()),
        source: "imported".into(),
        created_at: "2026-09-28T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: Some(6000),
        height: Some(4000),
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 1,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
    }
}

fn task_rows(db: &db::Db, kind: &str) -> i64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM index_tasks WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

fn indexed_flag(db: &db::Db, column: &str) -> i64 {
    db.0.query_row(
        &format!("SELECT COUNT(*) FROM assets WHERE {column} IS NOT NULL"),
        [],
        |r| r.get(0),
    )
    .unwrap()
}

/// 切档后自动重建的通道集合（check_params_and_rebuild 的指纹联动）：
/// - normal 首跑只写 marker（不重建）
/// - normal→fast：只重排 face（语义账不动）
/// - fast→accurate：face + semantic 双通道重排
#[test]
fn tier_switch_rebuilds_expected_channel_set() {
    let src = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    // 全档位件装齐（ready 门槛全开；假件足够——本测试不断言推理结果）
    install_fake_models(
        &db_dir.join("models"),
        &[
            "siglip2-visual",
            "siglip2-text",
            "siglip2-tokenizer",
            "siglip2-visual-fp16",
            "siglip2-text-fp16",
            "scrfd",
            "scrfd-10g",
            "arcface",
        ],
    );
    let state = common::state_with_library(&db_dir, src.path(), Duration::from_millis(1));
    {
        let mut s = state.settings.lock().unwrap();
        s.ai.enable_clip = true;
        s.ai.enable_face = true;
    }
    let db = common::open_db(&db_dir);
    for n in 1..=3 {
        db.insert_asset(&asset_row(&format!("X:/p/n{n}.jpg")))
            .unwrap();
    }
    db.set_ai_indexed(1).unwrap();
    db.set_face_indexed(1).unwrap();

    // 首跑（normal，marker 不存在）：只写标记不重建
    ipc::indexing::check_params_and_rebuild(&state, &db_dir, &tier_settings("normal"));
    let marker = db_dir.join("index-params.marker");
    assert!(marker.is_file(), "首跑写 marker");
    assert_eq!(task_rows(&db, "ai"), 0, "首跑不重排语义");
    assert_eq!(task_rows(&db, "face"), 0, "首跑不重排人脸");

    // 二跑同档：no-op（marker 命中）
    ipc::indexing::check_params_and_rebuild(&state, &db_dir, &tier_settings("normal"));
    assert_eq!(task_rows(&db, "ai"), 0);
    assert_eq!(task_rows(&db, "face"), 0);

    // normal→fast：只 face 重排（scrfd-10g 换件）；语义账保持
    // （重排会 spawn 假件回填 worker，face 行被失败回收后仍在账上——
    // 这里以「语义零重排 + 人脸账翻起」为通道集合判据）
    ipc::indexing::check_params_and_rebuild(&state, &db_dir, &tier_settings("fast"));
    assert_eq!(
        task_rows(&db, "ai"),
        0,
        "fast 切档不得重排语义（int8 同件）"
    );
    assert_eq!(indexed_flag(&db, "ai_indexed_at"), 1, "语义时间账不动");
    assert!(
        task_rows(&db, "face") > 0 || indexed_flag(&db, "face_indexed_at") == 0,
        "fast 切档必须触发 face 重排（任务账或时间账至少其一翻起）"
    );

    // fast→accurate：双通道重排（fp16 语义 + 源策略人脸）
    db.0.execute(
        "UPDATE assets SET ai_indexed_at = '2026-09-28T01:00:00.000Z'",
        [],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET face_indexed_at = '2026-09-28T01:00:00.000Z'",
        [],
    )
    .unwrap();
    db.0.execute("DELETE FROM index_tasks WHERE kind IN ('ai', 'face')", [])
        .unwrap();
    ipc::indexing::check_params_and_rebuild(&state, &db_dir, &tier_settings("accurate"));
    assert_eq!(
        indexed_flag(&db, "ai_indexed_at"),
        0,
        "accurate 切档语义账必须复位（重建）"
    );
    assert!(
        task_rows(&db, "ai") > 0 || indexed_flag(&db, "ai_indexed_at") == 0,
        "语义重排账翻起"
    );
    assert!(
        task_rows(&db, "face") > 0 || indexed_flag(&db, "face_indexed_at") == 0,
        "accurate 切档必须触发 face 重排"
    );
    // marker 已更新为 accurate 档指纹（同档再跑 no-op 的前提）
    let content = std::fs::read_to_string(&marker).unwrap();
    let accurate = tier_settings("accurate");
    assert!(content.contains(&format!(
        "semantic={}",
        semantic_params_fingerprint(&accurate)
    )));
    assert!(content.contains(&format!("face={}", face_params_fingerprint(&accurate))));
}

// ---------------------------------------------------------------------------
// 序列化契约（tier 字段 camelCase 透出）
// ---------------------------------------------------------------------------

#[test]
fn model_dtos_serialize_tier_field() {
    let entry = serde_json::to_value(ai::ModelEntry {
        id: "scrfd-10g".into(),
        url: "https://example.invalid/a.onnx".into(),
        mirror_url: "https://mirror.invalid/a.onnx".into(),
        sha256: "0".repeat(64),
        bytes_total: 1,
        version: "v1".into(),
        feature: "face".into(),
        tier: Some("fast".into()),
    })
    .unwrap();
    assert_eq!(entry["tier"], "fast");
    assert_eq!(entry["bytesTotal"], 1, "camelCase 保持");

    // null 共用件
    let shared = serde_json::to_value(ai::ModelEntry {
        id: "arcface".into(),
        url: "https://example.invalid/b.onnx".into(),
        mirror_url: "https://mirror.invalid/b.onnx".into(),
        sha256: "0".repeat(64),
        bytes_total: 1,
        version: "v1".into(),
        feature: "face".into(),
        tier: None,
    })
    .unwrap();
    assert!(shared.get("tier").is_some_and(|v| v.is_null()));
    // 缺 tier 字段的旧 JSON 也能解析（serde default）
    let parsed: ai::ModelEntry = serde_json::from_value(serde_json::json!({
        "id": "x", "url": "u", "mirrorUrl": "m", "sha256": "0".repeat(64),
        "bytesTotal": 1, "version": "v1", "feature": "face"
    }))
    .unwrap();
    assert_eq!(parsed.tier, None);

    let status = serde_json::to_value(ai::ModelStatusDto {
        id: "siglip2-visual-fp16".into(),
        installed: false,
        bytes_total: 186131676,
        downloaded_bytes: 0,
        version: "siglip2-base-patch16-256-fp16-v1".into(),
        feature: "semantic".into(),
        state: "idle".into(),
        tier: Some("accurate".into()),
    })
    .unwrap();
    assert_eq!(status["tier"], "accurate");
    assert_eq!(status["downloadedBytes"], 0);
}

// ---------------------------------------------------------------------------
// 阈值 auto 默认
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// 真机标定（默认忽略）：fp16 语义档分数带 + 索引吞吐
// ---------------------------------------------------------------------------

/// fp16 双塔真机标定 + 吞吐（方法同 2026-09-21 int8 标定轮）：
/// - 模型根 = 应用真实 models 目录（%APPDATA%\com.smartphoto.app\models，
///   SMARTPHOTO_FP16_MODELS 可覆盖）；库 = 真实主库 `I:\SmartPhoto\主库`
///   （SMARTPHOTO_FP16_LIBRARY 可覆盖）。
/// - 吞吐：全库 256 档缩略图按批 16 嵌入计时，前两批预热不计时（对比
///   int8 基线 ≈24 img/s，DML 批 16 / RTX 5070 Ti）。
/// - 分数带：荒谬词（库内无对应内容）×5 与内容词 ×5 的 top-1 / top-5 /
///   中位 cos 分布 → 工作点建议写回 semantic_default_min_score 的
///   accurate 分支（int8 轮工作点 0.09 = 无关 top≤0.096 / 相关簇
///   0.09-0.12 分隔带内保守值）。
/// 只读不写库（thumbs 缓存可顺手补生成；不动 vectors.usearch / 任务账）。
///
/// 跑法：`cargo test --test ai_quality_tier_test fp16_ -- --ignored --nocapture`
#[test]
#[ignore = "真机标定：需 fp16 双塔已下载到应用 models 目录 + 真实主库"]
fn fp16_semantic_calibration_and_throughput() {
    use ai::embed::SemanticEmbedder;
    use std::time::Instant;

    let models = std::env::var("SMARTPHOTO_FP16_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("APPDATA")
                .map(|appdata| std::path::PathBuf::from(appdata).join(r"com.smartphoto.app\models"))
                .expect("APPDATA")
        });
    let db_dir = std::env::var("SMARTPHOTO_FP16_LIBRARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(r"I:\SmartPhoto\主库"));

    for id in [
        "siglip2-visual-fp16",
        "siglip2-text-fp16",
        "siglip2-tokenizer",
    ] {
        assert!(
            models.join(format!("{id}.onnx")).is_file(),
            "{id} 未落位（{}）——先在设置页下载或手动放置",
            models.display()
        );
    }
    assert!(
        db_dir.join("library.db").is_file(),
        "真库不存在: {}",
        db_dir.display()
    );

    let use_gpu = !matches!(std::env::var("SMARTPHOTO_AI_EP").as_deref(), Ok("cpu"));
    let bus = events::EventBus::new();
    let supervisor = tasks::TaskSupervisor::new(bus.clone());
    let manager = ai::ModelManager::new(models, bus, supervisor);
    // SMARTPHOTO_CALIB_TIER=normal 可跑 int8 对照轮（同库同查询）
    let tier = match std::env::var("SMARTPHOTO_CALIB_TIER").as_deref() {
        Ok("normal") => QualityTier::Normal,
        _ => QualityTier::Accurate,
    };
    eprintln!(
        "标定档位：{}（{}）",
        tier.as_str(),
        if matches!(tier, QualityTier::Accurate) {
            "fp16"
        } else {
            "int8"
        }
    );
    manager.set_ai_params(ai::AiIndexParams {
        quality_tier: tier,
        use_gpu,
        ..ai::AiIndexParams::default()
    });
    let embedder: std::sync::Arc<dyn SemanticEmbedder> = std::sync::Arc::new(manager);

    let db = common::open_db(&db_dir);
    let paths: Vec<String> =
        db.0.prepare("SELECT path FROM assets WHERE kind IN ('photo','raw') ORDER BY id")
            .unwrap()
            .query_map([], |r| r.get::<_, String>(0))
            .unwrap()
            .flatten()
            .collect();
    assert!(!paths.is_empty(), "真库无照片资产");
    eprintln!("标定库：{} 张（{}）", paths.len(), db_dir.display());

    // —— 吞吐 + 全量向量：批 16，前两批预热（会话/显存暖机）不计时 ——
    let srcs: Vec<std::path::PathBuf> = paths.iter().map(std::path::PathBuf::from).collect();
    let mut vectors: Vec<Vec<f32>> = Vec::with_capacity(srcs.len());
    let mut timed_n = 0usize;
    let mut clock = None;
    for chunk in srcs.chunks(16) {
        let rows = embedder.embed_images(chunk, &db_dir).expect("fp16 批嵌入");
        vectors.extend(rows);
        if clock.is_none() && vectors.len() >= 32 {
            clock = Some(Instant::now()); // 第 3 批起点开始计时
        } else if clock.is_some() {
            timed_n += chunk.len();
        }
    }
    if let Some(t0) = clock {
        let elapsed = t0.elapsed().as_secs_f64();
        eprintln!(
            "fp16 语义索引吞吐：{:.1} img/s（计时 {} 张 / {:.1}s；int8 基线 ≈24 img/s DML 批16）",
            timed_n as f64 / elapsed,
            timed_n,
            elapsed
        );
    }

    // —— 分数带：查询嵌入 vs 图像向量 cos ——
    let absurd = [
        "手术台",
        "无人机航拍",
        "税务审计报表",
        "深海钻井平台",
        "考古发掘现场",
    ];
    let content = ["猫", "狗", "海边", "日落", "人像"];
    let band = |label: &str, words: &[&str]| {
        for q in words {
            let qv = embedder.embed_text(q).expect("查询嵌入");
            let mut sims: Vec<f32> = vectors
                .iter()
                .map(|v| v.iter().zip(&qv).map(|(a, b)| a * b).sum())
                .collect();
            sims.sort_by(|a, b| b.total_cmp(a));
            eprintln!(
                "[{label}] 「{q}」 top1={:.4} top5avg={:.4} 中位={:.4}",
                sims.first().copied().unwrap_or(0.0),
                sims.iter().take(5).sum::<f32>() / 5.0,
                sims.get(sims.len() / 2).copied().unwrap_or(0.0),
            );
        }
    };
    band("荒谬", &absurd);
    band("内容", &content);
    eprintln!("工作点建议：荒谬 top1 上界与内容 top1 下界的分隔带内取保守值（int8 轮 = 0.09）");
}

#[test]
fn semantic_default_min_score_follows_variant() {
    // int8（fast/normal）= 2026-09-21 标定工作点 0.09
    for tier in [QualityTier::Fast, QualityTier::Normal] {
        assert!((ai::semantic::semantic_default_min_score(tier) - 0.09).abs() < 1e-6);
    }
    // fp16（accurate）= 0.03（2026-09-28 真机标定，497 张真库荒谬/内容词
    // 分隔带；变更需重标定并同步 semantic.rs 注释）
    let fp16 = ai::semantic::semantic_default_min_score(QualityTier::Accurate);
    assert!((fp16 - 0.03).abs() < 1e-6, "fp16 标定工作点 0.03: {fp16}");
}
