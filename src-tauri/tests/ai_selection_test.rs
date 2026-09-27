//! AI 辅助选片（0021）：blur 拉普拉斯清晰度分与阈值判定、eyes 三态聚合
//! （stub 分类器；真模型冒烟 #[ignore]）、任务账与指纹重排、ai_analysis
//! 级联清理、筛选维度（eyes/blur）、模型目录「selection 暂无条目」契约。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::{AssetFilters, AssetRow};
use events::AssetKind;

fn setup() -> (tempfile::TempDir, db::Db, ipc::AppState) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photos).unwrap();
    let db = open_db(&db_dir);
    let state = common::state_with_library(&db_dir, &photos, Duration::from_millis(1));
    (dir, db, state)
}

fn ins(db: &db::Db, dir: &std::path::Path, name: &str, img: &image::RgbImage) -> i64 {
    let path = dir.join(name);
    img.save_with_format(&path, image::ImageFormat::Jpeg)
        .unwrap();
    db.insert_asset(&AssetRow {
        path: path.to_string_lossy().into_owned(),
        filename: name.into(),
        size: 4096,
        mtime: "2026-09-01T00:00:00.000Z".into(),
        xxhash: name.len() as u64,
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-09-01T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: Some(img.width()),
        height: Some(img.height()),
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
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
    })
    .unwrap();
    db.asset_id_by_path(&path.to_string_lossy())
        .unwrap()
        .unwrap()
}

/// 高频棋盘（清晰）与强模糊同图（soft）。
fn sharp_image() -> image::RgbImage {
    let mut img = image::RgbImage::new(256, 256);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let v = if (x / 4 + y / 4) % 2 == 0 { 240 } else { 16 };
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    img
}

fn soft_image() -> image::RgbImage {
    // 垂直线性渐变：拉普拉斯响应 ≈ 0（确定性 soft，不依赖模糊半径）
    let mut img = image::RgbImage::new(256, 256);
    for y in 0..256u32 {
        let v = (y * 255 / 255) as u8;
        for x in 0..256u32 {
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    img
}

fn analysis(db: &db::Db, id: i64, kind: &str) -> Option<(Option<String>, Option<f64>)> {
    db.0.query_row(
        "SELECT value, score FROM ai_analysis WHERE asset_id = ?1 AND kind = ?2",
        rusqlite::params![id, kind],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

// ---------------------------------------------------------------------------
// blur 通道：分数、阈值、任务账
// ---------------------------------------------------------------------------

#[test]
fn blur_task_scores_sharp_and_soft_with_threshold() {
    let (dir, db, _state) = setup();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    let sharp = ins(&db, &photos, "sharp.jpg", &sharp_image());
    let soft = ins(&db, &photos, "soft.jpg", &soft_image());

    // 分数单调性（同归一曲线：清晰 > 模糊）
    let sharp_var = ai::selection::laplacian_variance(&image::imageops::blur(
        &image::imageops::grayscale(&sharp_image()),
        0.0,
    ))
    .unwrap();
    let soft_var =
        ai::selection::laplacian_variance(&image::imageops::grayscale(&soft_image())).unwrap();
    assert!(sharp_var > soft_var * 10.0, "棋盘应显著高于强模糊");
    assert!(ai::selection::normalize_blur_score(sharp_var) > 90.0);
    assert!(ai::selection::normalize_blur_score(soft_var) < 10.0);

    // 任务账：导入时自动登记（insert_asset_on），代际重排复位
    assert!(db.pending_index_task_count("blur").unwrap() >= 2);
    assert!(db.pending_index_task_count("eyes").unwrap() >= 2);

    // 处理 blur 任务（阈值 30）
    assert!(ai::selection::process_blur_task(&db, &db_dir, sharp, 30.0));
    assert!(ai::selection::process_blur_task(&db, &db_dir, soft, 30.0));
    let (sharp_value, sharp_score) = analysis(&db, sharp, "blur").unwrap();
    let (soft_value, soft_score) = analysis(&db, soft, "blur").unwrap();
    assert_eq!(sharp_value.as_deref(), Some("sharp"));
    assert_eq!(soft_value.as_deref(), Some("soft"));
    assert!(sharp_score.unwrap() > soft_score.unwrap());
    assert!((0.0..=100.0).contains(&sharp_score.unwrap()));

    // 高阈值下清晰图也可判 soft（阈值参与判定；归一分恒 < 100）
    assert!(ai::selection::process_blur_task(&db, &db_dir, sharp, 100.0));
    assert_eq!(
        analysis(&db, sharp, "blur").unwrap().0.as_deref(),
        Some("soft")
    );

    // upsert：同一 (asset, kind) 只有一行
    let rows: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM ai_analysis WHERE asset_id = ?1",
            [sharp],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn blur_missing_thumb_records_unknown() {
    let (dir, db, _state) = setup();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    let id = ins(&db, &photos, "ghost.jpg", &sharp_image());
    std::fs::remove_file(photos.join("ghost.jpg")).unwrap(); // 源消失

    // 源消失 → 512 档无法生成 → unknown + 按完成收尾（真机语义：不占重试）
    assert!(ai::selection::process_blur_task(&db, &db_dir, id, 30.0));
    assert_eq!(
        analysis(&db, id, "blur").unwrap().0.as_deref(),
        Some("unknown")
    );
}

// ---------------------------------------------------------------------------
// eyes 通道：三态聚合（stub 分类器）+ 无人脸/模型缺失跳过
// ---------------------------------------------------------------------------

struct FixedEyes(f32);
impl ai::selection::EyeStateClassifier for FixedEyes {
    fn classify_closed(&self, _crop: &image::RgbImage) -> Result<f32, String> {
        Ok(self.0)
    }
}

#[test]
fn eyes_aggregation_three_states() {
    use ai::selection::{aggregate_eyes, EYES_CLOSED_THRESHOLD, EYES_MAYBE_THRESHOLD};
    assert_eq!(EYES_CLOSED_THRESHOLD, 0.7);
    assert_eq!(EYES_MAYBE_THRESHOLD, 0.5);

    // 任一 closed → closed（score = 最大 p）
    let (v, s) = aggregate_eyes(&[0.2, 0.9], 0.7, 0.5).unwrap();
    assert_eq!(v, "closed");
    assert!((s - 0.9).abs() < 1e-6);
    // 无 closed 但有 maybe → maybe
    let (v, _) = aggregate_eyes(&[0.2, 0.6], 0.7, 0.5).unwrap();
    assert_eq!(v, "maybe");
    // 全睁 → 无记录（None；≠ unknown）
    assert!(aggregate_eyes(&[0.1, 0.3], 0.7, 0.5).is_none());
    // 空（无人脸/遮挡不可判定）→ unknown 保守三态
    let (v, _) = aggregate_eyes(&[], 0.7, 0.5).unwrap();
    assert_eq!(v, "unknown");
}

#[test]
fn eyes_backfill_without_model_skips_and_counts() {
    let (dir, db, _state) = setup();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    let _a = ins(&db, &photos, "a.jpg", &sharp_image());
    let _b = ins(&db, &photos, "b.jpg", &soft_image());

    let pending_before = db.pending_index_task_count("eyes").unwrap();
    assert!(pending_before >= 2);

    // 模型未收录（清单无 selection 条目）：回填跳过、任务保持 pending
    let manager = ai::ModelManager::new(
        db_dir.join("models"),
        events::EventBus::new(),
        tasks::TaskSupervisor::new(events::EventBus::new()),
    );
    let _ = &manager;
    assert!(!manager.selection_eyes_ready());
    assert_eq!(
        ai::selection::run_eyes_backfill(&db_dir, &manager, &events::EventBus::new()),
        0
    );
    assert_eq!(
        db.pending_index_task_count("eyes").unwrap(),
        pending_before,
        "跳过不消费任务"
    );
}

// ---------------------------------------------------------------------------
// 级联 / 筛选 / 指纹 / 目录契约
// ---------------------------------------------------------------------------

#[test]
fn ai_analysis_cascades_on_asset_delete_and_filters_work() {
    let (dir, db, state) = setup();
    let photos = dir.path().join("photos");
    let closed = ins(&db, &photos, "closed.jpg", &soft_image());
    let blurry = ins(&db, &photos, "blurry.jpg", &soft_image());
    let clean = ins(&db, &photos, "clean.jpg", &sharp_image());

    db.set_ai_analysis(closed, "eyes", Some("closed"), Some(0.93), "stub-v1")
        .unwrap();
    db.set_ai_analysis(
        blurry,
        "blur",
        Some("soft"),
        Some(12.5),
        ai::selection::BLUR_ALGO_VERSION,
    )
    .unwrap();
    db.set_ai_analysis(
        clean,
        "blur",
        Some("sharp"),
        Some(88.0),
        ai::selection::BLUR_ALGO_VERSION,
    )
    .unwrap();

    // 筛选维度（白名单外值容错不过滤）
    let page = |f: AssetFilters| {
        ipc::assets::fetch_assets_page(&state, 0, 50, f)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        page(AssetFilters {
            eyes: Some("closed".into()),
            ..Default::default()
        }),
        vec!["closed.jpg".to_string()]
    );
    assert_eq!(
        page(AssetFilters {
            blur: Some("soft".into()),
            ..Default::default()
        }),
        vec!["blurry.jpg".to_string()]
    );
    assert_eq!(
        page(AssetFilters {
            eyes: Some("bogus".into()),
            ..Default::default()
        })
        .len(),
        3,
        "非白名单值容错不过滤"
    );

    // 详情 aiAnalysis（契约对象形状 {eyes, blur}）
    let detail = ipc::assets::fetch_asset_detail(&state, closed)
        .unwrap()
        .unwrap();
    assert_eq!(
        detail
            .ai_analysis
            .eyes
            .as_ref()
            .and_then(|e| e.value.clone()),
        Some("closed".to_string())
    );
    assert!(detail.ai_analysis.blur.is_none());
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["aiAnalysis"]["eyes"]["value"], "closed");
    assert_eq!(json["aiAnalysis"]["eyes"]["modelVersion"], "stub-v1");

    // 级联：删除资产 → 分析行随 FK 消失
    db.assets_delete_rows(&[closed]).unwrap();
    assert!(analysis(&db, closed, "eyes").is_none());
}

#[test]
fn selection_fingerprint_and_requeue() {
    let (dir, db, _state) = setup();
    let photos = dir.path().join("photos");
    let _ = ins(&db, &photos, "x.jpg", &sharp_image());

    let mut ai = settings::AiSettings::default();
    let f1 = ipc::indexing::selection_params_fingerprint(&ai);
    ai.blur_soft_threshold = 45.0;
    let f2 = ipc::indexing::selection_params_fingerprint(&ai);
    assert_ne!(f1, f2, "阈值变更应改变指纹");

    // 代际重排：done → pending + 无任务行新建
    let id = db
        .asset_id_by_path(&photos.join("x.jpg").to_string_lossy())
        .unwrap()
        .unwrap();
    db.set_ai_analysis(id, "blur", Some("sharp"), Some(90.0), "laplacian-v1")
        .unwrap();
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'blur' AND asset_id = ?1",
        [id],
    )
    .unwrap();
    let pending_after = db.requeue_blur_tasks_for_all().unwrap();
    assert!(pending_after >= 1);
    let state: String =
        db.0.query_row(
            "SELECT state FROM index_tasks WHERE kind = 'blur' AND asset_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(state, "pending");
}

#[test]
fn catalog_has_no_selection_entry_until_model_licensed() {
    // 0021 模型选型停摆契约：许可证干净且 ONNX 的小模型找到之前，
    // 清单不得出现 feature="selection" 条目（不塞来源不明的权重）
    assert!(ai::catalog().iter().all(|e| e.feature != "selection"));
    // 既有三包制不变：semantic + face
    let features: Vec<&str> = {
        let mut v: Vec<&str> = ai::catalog().iter().map(|e| e.feature.as_str()).collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    assert_eq!(features, vec!["face", "semantic"]);
}
