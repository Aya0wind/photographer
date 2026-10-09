//! AI 辅助选片（0021）：blur 拉普拉斯清晰度分与阈值判定、eyes 通道
//! （facemesh EAR 三态——纯函数/眼部索引/ROI 旋转几何/聚合分带/落库）、
//! 任务账与指纹重排、ai_analysis 级联清理、筛选维度（eyes/blur）、模型
//! 清单 selection=facemesh pin 契约。

mod common;

#[test]
#[ignore = "read-only stage timing; set SMARTPHOTO_BENCH_DB_DIR, SMARTPHOTO_BENCH_ASSET and SMARTPHOTO_SELECTION_MODELS"]
fn selection_stage_timing() {
    use ai::selection::FaceEyeStateClassifier;
    let directory = std::path::PathBuf::from(std::env::var("SMARTPHOTO_BENCH_DB_DIR").unwrap());
    let asset_id = std::env::var("SMARTPHOTO_BENCH_ASSET")
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let db = db::Db(
        rusqlite::Connection::open_with_flags(
            directory.join("library.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .unwrap(),
    );
    let models = std::path::PathBuf::from(std::env::var("SMARTPHOTO_SELECTION_MODELS").unwrap());
    let bus = events::EventBus::new();
    let manager = ai::ModelManager::new(models, bus.clone(), tasks::TaskSupervisor::new(bus));
    manager.set_ai_params(ai::AiIndexParams {
        quality_tier: ai::QualityTier::Accurate,
        ..Default::default()
    });
    let classifier = ai::selection::FacemeshEarClassifier::new(&manager);
    for round in 0..2 {
        let start = std::time::Instant::now();
        let (img, _) = ai::selection_regions::thumbnail(&db, &directory, asset_id).unwrap();
        let decode = start.elapsed();
        let start = std::time::Instant::now();
        let faces = ai::face::detect_faces(&manager, &img).unwrap();
        let detection = start.elapsed();
        let start = std::time::Instant::now();
        for (i, face) in faces.iter().enumerate() {
            classifier.classify(&img, face, i + 1).unwrap();
        }
        eprintln!("[selection-bench] round={round} input={}x{} faces={} decode_ms={} detection_ms={} eyes_ms={}",
            img.width(),img.height(),faces.len(),decode.as_millis(),detection.as_millis(),start.elapsed().as_millis());
    }
}

#[test]
fn eye_candidates_are_separate_from_calibrated_verdicts() {
    use ai::selection_regions::{eye_state, photo_eyes, Region};
    assert_eq!(eye_state([0.97, 0.96], Some(0.1), 0.13, 0.2).0, "closed");
    assert_eq!(
        eye_state([0.97, 0.05], Some(0.1), 0.13, 0.2),
        ("unknown", Some("crop_disagreement"))
    );
    assert_eq!(eye_state([0.01, 0.03], Some(0.3), 0.13, 0.2).0, "open");
    assert_eq!(
        eye_state([0.99, 0.98], Some(0.3), 0.13, 0.2).0,
        "open",
        "clear geometric opening is not vetoed by the IR candidate"
    );
    assert_eq!(
        eye_state([f64::NAN, 0.98], Some(0.1), 0.13, 0.2).0,
        "unknown"
    );
    assert_eq!(
        eye_state([0.99, 0.98], Some(0.21), 0.13, 0.2),
        ("maybe", Some("model_geometry_disagreement")),
        "borderline opening still needs review"
    );
    assert_eq!(eye_state([0.97, 0.05], Some(0.3), 0.13, 0.2).0, "open");
    let eye = |state: &str| Region {
        kind: "eye".into(),
        person: 1,
        side: None,
        bounds: [0.0, 0.0, 0.1, 0.1],
        state: state.into(),
        reason: None,
        raw_score: None,
        auxiliary_ear: None,
    };
    assert_eq!(photo_eyes(&[eye("open"), eye("open")], false), "open");
    assert_eq!(
        photo_eyes(&[eye("not_detected"), eye("open")], false),
        "open"
    );
    assert_eq!(
        photo_eyes(&[eye("closed"), eye("open")], false),
        "single_closed"
    );
    assert_eq!(photo_eyes(&[eye("closed"), eye("open")], true), "closed");
    assert_eq!(
        photo_eyes(&[eye("closed"), eye("closed")], false),
        "closed",
        "two detected closed eyes yield a binary result"
    );
    assert_eq!(photo_eyes(&[], false), "no_face");
    let mut small = eye("unknown");
    small.person = 2;
    assert_eq!(
        photo_eyes(&[eye("open"), eye("open"), small], false),
        "open"
    );
}

#[test]
fn eye_crop_rejects_pixels_created_by_padding_or_upscaling() {
    let img = image::RgbImage::new(128, 128);
    let points = |x: f32, width: f32| {
        [
            [x, 50.0],
            [x + width * 0.25, 48.0],
            [x + width * 0.75, 48.0],
            [x + width, 50.0],
            [x + width * 0.75, 52.0],
            [x + width * 0.25, 52.0],
        ]
    };
    assert!(ai::selection_regions::eye_crop(&img, &points(50.0, 10.0), 1.4).is_none());
    assert!(ai::selection_regions::eye_crop(&img, &points(0.0, 20.0), 1.4).is_none());
    let (crop, bounds) = ai::selection_regions::eye_crop(&img, &points(50.0, 20.0), 1.4).unwrap();
    assert_eq!(crop.dimensions(), (32, 32));
    assert!((bounds[2] * 128.0 - 28.0).abs() < 0.001);
    assert!(bounds.iter().all(|n| (0.0..=1.0).contains(n)));
    assert!(!ai::selection_regions::eye_has_detail(&crop));
}

#[test]
fn region_evidence_preserves_unknown_and_null_scores_through_ipc() {
    let (dir, db, state) = setup();
    let id = ins(
        &db,
        &dir.path().join("photos"),
        "unknown.jpg",
        &sharp_image(),
    );
    let evidence = serde_json::json!({"source":"original", "width":2048,"height":1365,
        "calibrated":false,"reason":"small_face","regions":[]});
    db.set_ai_analysis_details(id, "eyes", "unknown", None, "region-test", &evidence)
        .unwrap();
    let detail = ipc::assets::fetch_asset_detail(&state, id)
        .unwrap()
        .unwrap();
    let eyes = detail.ai_analysis.eyes.unwrap();
    assert_eq!(eyes.value.as_deref(), Some("unknown"));
    assert_eq!(eyes.score, None);
    assert_eq!(eyes.details.unwrap(), evidence);
    db.set_ai_analysis_details(
        id,
        "eyes",
        "open",
        None,
        "region-test-2",
        &serde_json::json!({"regions":[]}),
    )
    .unwrap();
    assert_eq!(db.ai_analysis_for(id).unwrap().len(), 1);
    assert!(!db
        .ai_analysis_details(id, "eyes")
        .unwrap()
        .unwrap()
        .contains("small_face"));
    db.set_ai_analysis(id, "eyes", Some("open"), None, "legacy-test")
        .unwrap();
    assert!(
        db.ai_analysis_details(id, "eyes").unwrap().is_none(),
        "legacy write must clear stale candidate evidence"
    );
}

#[test]
fn selection_source_is_high_resolution_and_not_the_browsing_cache() {
    let _permit = ai::selection_regions::analysis_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.png");
    image::RgbImage::new(2200, 1100).save(&path).unwrap();
    let (img, origin) = thumbs::selection_source(&path, 2048).unwrap();
    assert_eq!((img.width(), img.height()), (2048, 1024));
    assert_eq!(origin, "original");
    let (a, _) = ai::selection_regions::source(&path, 2048).unwrap();
    let (b, _) = ai::selection_regions::source(&path, 2048).unwrap();
    assert!(std::sync::Arc::ptr_eq(&a, &b));
    std::fs::remove_file(path).unwrap();
    assert!(
        ai::selection_regions::source(&dir.path().join("source.png"), 2048).is_none(),
        "cached image cannot hide an absent source"
    );
}

#[test]
fn production_selection_reads_small_cache_even_when_original_is_offline() {
    let (dir, db, _state) = setup();
    let img = image::RgbImage::new(2400, 1200);
    let photos = dir.path().join("photos");
    let id = ins(&db, &photos, "large.jpg", &img);
    let source = photos.join("large.jpg");
    let cache = thumbs::thumb_file(&dir.path().join("db"), &source, 1024).unwrap();
    let mtime: chrono::DateTime<chrono::Utc> = std::fs::metadata(&source)
        .unwrap()
        .modified()
        .unwrap()
        .into();
    db.0.execute(
        "UPDATE assets SET mtime=?1 WHERE id=?2",
        rusqlite::params![mtime.to_rfc3339(), id],
    )
    .unwrap();
    std::fs::remove_file(&source).unwrap();
    assert_eq!(
        thumbs::cached_for_asset(&dir.path().join("db"), &source, 1024, &mtime.to_rfc3339()),
        Some(cache)
    );
    let (pixels, origin) =
        ai::selection_regions::thumbnail(&db, &dir.path().join("db"), id).unwrap();
    assert_eq!(pixels.dimensions(), (1024, 512));
    assert_eq!(origin, "thumbnail");
    assert!(ai::selection::process_blur_task(
        &db,
        &dir.path().join("db"),
        id,
        30.0
    ));
    let details: serde_json::Value =
        serde_json::from_str(&db.ai_analysis_details(id, "blur").unwrap().unwrap()).unwrap();
    assert_eq!(details["source"], "thumbnail");
    assert_eq!(details["width"], 1024);
}

#[test]
#[ignore = "photography evaluation export; requires photos and downloaded models"]
fn export_selection_candidate_samples() {
    use ai::selection::FaceEyeStateClassifier;
    use std::io::Write;
    let input = std::path::PathBuf::from(
        std::env::var("SMARTPHOTO_SELECTION_PHOTOS").expect("photo directory"),
    );
    let models = std::path::PathBuf::from(
        std::env::var("SMARTPHOTO_SELECTION_MODELS").expect("global models directory"),
    );
    let output = std::path::PathBuf::from(
        std::env::var("SMARTPHOTO_SELECTION_OUTPUT").expect("new JSONL output file"),
    );
    let bus = events::EventBus::new();
    let manager = ai::ModelManager::new(models, bus.clone(), tasks::TaskSupervisor::new(bus));
    assert!(
        manager.selection_eyes_ready(),
        "download current selection package and SCRFD first"
    );
    let classifier = ai::selection::FacemeshEarClassifier::new(&manager);
    let mut out = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    let mut paths: Vec<_> = std::fs::read_dir(input)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file() && thumbs::is_decodable(p))
        .collect();
    paths.sort();
    for path in paths {
        let _permit = ai::selection_regions::analysis_lock().lock().unwrap();
        let (img, source) = ai::selection_regions::source(&path, 2048).expect("source decode");
        let faces = ai::face::detect_selection_faces(&manager, &img).unwrap();
        let mut evidence = ai::selection_regions::evidence(&img, source);
        for (i, face) in faces.iter().enumerate() {
            evidence
                .regions
                .extend(classifier.classify(&img, face, i + 1).unwrap());
        }
        let summary = ai::selection_regions::photo_eyes(&evidence.regions, false);
        let (legacy_img, _) = thumbs::selection_source(&path, 512).unwrap();
        let legacy_faces = ai::face::detect_faces(&manager, &legacy_img).unwrap();
        let ears: Vec<_> = legacy_faces
            .iter()
            .map(|f| classifier.face_min_ear(&legacy_img, f).unwrap())
            .collect();
        let legacy_eyes = ai::selection::aggregate_eyes_ear(&ears, 0.13, 0.20)
            .map(|(value, _)| value)
            .unwrap_or_else(|| {
                if legacy_faces.is_empty() {
                    "unknown"
                } else {
                    "open"
                }
                .into()
            });
        let legacy_blur =
            ai::selection::laplacian_variance(&image::imageops::grayscale(&legacy_img))
                .map(|v| {
                    if ai::selection::normalize_blur_score(v) < 30.0 {
                        "soft"
                    } else {
                        "sharp"
                    }
                })
                .unwrap_or("unknown");
        let boxes: Vec<_> = faces
            .iter()
            .map(|f| ai::face::normalized_box(f, img.width(), img.height()))
            .collect();
        let defocus = if manager.model_path("defocus-candidate").is_file() {
            Some(
                ai::selection_defocus::evaluate(&manager, &img, source, &boxes)
                    .expect("defocus evaluation"),
            )
        } else {
            None
        };
        writeln!(out,"{}",serde_json::json!({"path":path.to_string_lossy(),
            "legacy":{"eyes":legacy_eyes,"blur":legacy_blur},
            "candidate":{"eyes":summary,"blur":"unknown"}, "eyesEvidence":evidence,"defocusEvidence":defocus})).unwrap();
    }
}

#[test]
#[ignore = "requires downloaded pinned ONNX; set SMARTPHOTO_EYE_ONNX"]
fn eye_onnx_contract_smoke() {
    let path = std::env::var("SMARTPHOTO_EYE_ONNX").expect("set SMARTPHOTO_EYE_ONNX");
    let dir = tempfile::tempdir().unwrap();
    let bus = events::EventBus::new();
    let manager = ai::ModelManager::new(
        dir.path().into(),
        bus.clone(),
        tasks::TaskSupervisor::new(bus),
    );
    manager.set_ai_params(ai::AiIndexParams {
        use_gpu: false,
        ..Default::default()
    });
    std::fs::copy(path, manager.model_path(ai::selection_regions::EYE_MODEL)).unwrap();
    ai::selection_regions::release();
    let score =
        ai::selection_regions::closed_score(&manager, &image::RgbImage::new(32, 32)).unwrap();
    assert!((0.0..=1.0).contains(&score));
}

#[test]
#[ignore = "adapter contract test; set SMARTPHOTO_DEFOCUS_TEST_MODELS to generated test model directory"]
fn defocus_onnx_contract_smoke() {
    let root = std::path::PathBuf::from(std::env::var("SMARTPHOTO_DEFOCUS_TEST_MODELS").unwrap());
    let bus = events::EventBus::new();
    let manager = ai::ModelManager::new(root, bus.clone(), tasks::TaskSupervisor::new(bus));
    manager.set_ai_params(ai::AiIndexParams {
        use_gpu: false,
        ..Default::default()
    });
    let evidence = ai::selection_defocus::evaluate(
        &manager,
        &image::RgbImage::new(640, 480),
        "original",
        &[[0.25, 0.25, 0.5, 0.5]],
    )
    .unwrap();
    assert_eq!(evidence.regions.len(), 2);
    assert_eq!(evidence.regions[0].kind, "subject");
    assert_eq!(evidence.regions[1].kind, "background");
    assert_eq!(evidence.regions[0].raw_score, Some(0.5));
    assert!(!evidence.calibrated);
    assert!(evidence.regions.iter().all(|r| r.state == "unknown"));
}

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan,
    settings, tasks, thumbs,
};

use db::{AssetFilters, AssetRow};
use events::AssetKind;

fn setup() -> (tempfile::TempDir, db::Db, ipc::AppState) {
    let (dir, state, db) = common::library_fixture();
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
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
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
fn blur_task_reports_detail_clarity_and_abstains_on_low_texture() {
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
    assert_eq!(soft_value.as_deref(), Some("unknown"));
    assert!(
        soft_score.is_none(),
        "a smooth gradient has no focus evidence"
    );
    assert!((0.0..=100.0).contains(&sharp_score.unwrap()));

    // A higher threshold cannot turn strong edges into a definite soft verdict
    assert!(ai::selection::process_blur_task(&db, &db_dir, sharp, 100.0));
    assert_eq!(
        analysis(&db, sharp, "blur").unwrap().0.as_deref(),
        Some("maybe")
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

    // 源消失 → 1024 档无法生成 → unknown + 按完成收尾（真机语义：不占重试）
    assert!(ai::selection::process_blur_task(&db, &db_dir, id, 30.0));
    assert_eq!(
        analysis(&db, id, "blur").unwrap().0.as_deref(),
        Some("unknown")
    );
}

// ---------------------------------------------------------------------------
// faces 坐标空间 v2（归一化 0..1）：blur 通道人脸局部裁剪映射
// ---------------------------------------------------------------------------

/// 归一化框 → 缩略图像素裁剪窗真值表（外扩 20%、h=w 方窗、边界钳制不 panic）。
#[test]
fn blur_face_crop_window_maps_normalized_box() {
    use ai::selection::face_crop_window_px as win;
    // 常规：512×256 缩略图上 (0.25, 0.5, 0.1) → (128, 128, 61, 61)
    assert_eq!(win(0.25, 0.5, 0.1, 512, 256), (128, 128, 61, 61));
    // 贴右缘：宽夹到剩余边界（512-486=26）
    assert_eq!(win(0.95, 0.5, 0.1, 512, 256), (486, 128, 26, 26));
    // 极小框：下限 8px
    assert_eq!(win(0.0, 0.0, 0.001, 512, 256), (0, 0, 8, 8));
    // 贴底缘：h 夹到剩余边界（256-253=3），不 panic
    assert_eq!(win(0.5, 0.99, 0.1, 512, 256), (256, 253, 61, 3));
    // 溢出坐标防御：负值起点夹 0，超 1 的比例夹边界
    assert_eq!(win(0.0, 0.0, 1.0, 100, 100), (0, 0, 100, 100));
}

/// A low-texture face region does not condemn sharp details elsewhere.
#[test]
fn blur_task_ignores_low_texture_face_and_retains_detail_evidence() {
    let (dir, db, _state) = setup();
    let db_dir = dir.path().join("db");
    let photos = dir.path().join("photos");
    // 左半垂直渐变（soft）、右半棋盘（sharp）：全图分被右半拉高
    let mut img = image::RgbImage::new(256, 256);
    for y in 0..256u32 {
        for x in 0..256u32 {
            let v = if x < 128 {
                y as u8
            } else if (x / 4 + y / 4) % 2 == 0 {
                240
            } else {
                16
            };
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    let id = ins(&db, &photos, "half.jpg", &img);

    // 无人脸框：全图分（棋盘主导）→ sharp
    assert!(ai::selection::process_blur_task(&db, &db_dir, id, 30.0));
    assert_eq!(
        analysis(&db, id, "blur").unwrap().0.as_deref(),
        Some("sharp")
    );

    // Low-texture normalized face region: use detail evidence instead of a false soft verdict
    db.insert_face(id, 0.0, 0.0, 0.4, 0.4, &[0.5f32; 512], None)
        .unwrap();
    assert!(ai::selection::process_blur_task(&db, &db_dir, id, 30.0));
    assert_eq!(
        analysis(&db, id, "blur").unwrap().0.as_deref(),
        Some("sharp"),
        "low texture is not evidence of defocus"
    );
}

// ---------------------------------------------------------------------------
// eyes 通道：EAR 纯函数 / 眼部索引 / ROI 旋转几何 / 三态聚合（真模型全量
// 冒烟见真机验收；stub 分类器注入位 = FaceEyeStateClassifier trait）
// ---------------------------------------------------------------------------

/// EAR 六点合成真值：睁眼（宽 10 上下睑距 2/1）与闭眼（上下睑距 0.2/0.1）。
#[test]
fn eyes_ear_pure_function_open_and_closed() {
    use ai::selection::eye_aspect_ratio;
    let lm_open = [
        [0.0, 0.0],  // p1 外角
        [5.0, 2.0],  // p2 上睑
        [7.0, 2.0],  // p3 上睑内
        [10.0, 0.0], // p4 内角
        [7.0, 1.0],  // p5 下睑内
        [5.0, 1.0],  // p6 下睑
    ];
    let ear = eye_aspect_ratio(&lm_open, &[0, 1, 2, 3, 4, 5]).unwrap();
    assert!((ear - 0.10).abs() < 1e-5, "(1+1)/(2×10) = 0.10，实测 {ear}");
    // 闭眼：上下睑贴合（高度 0.1/0.1）
    let lm_closed = [
        [0.0, 0.0],
        [5.0, 0.2],
        [7.0, 0.2],
        [10.0, 0.0],
        [7.0, 0.1],
        [5.0, 0.1],
    ];
    let ear = eye_aspect_ratio(&lm_closed, &[0, 1, 2, 3, 4, 5]).unwrap();
    assert!((ear - 0.01).abs() < 1e-5);
    // 退化：眼宽 0（p1=p4）→ None；索引越界（点表不足 468 格式）→ None
    let lm_degenerate = [
        [0.0, 0.0],
        [1.0, 0.1],
        [2.0, 0.1],
        [0.0, 0.0],
        [2.0, 0.0],
        [1.0, 0.0],
    ];
    assert!(eye_aspect_ratio(&lm_degenerate, &[0, 1, 2, 3, 4, 5]).is_none());
    assert!(eye_aspect_ratio(&lm_open, &[33, 160, 158, 133, 153, 144]).is_none());
}

/// 眼部索引契约：EAR 六点均 < 468（虹膜区 468-477 不可用不参与）、
/// 左右两组互不重叠、总点数 478（468 网格 + 10 虹膜）。
#[test]
fn eyes_landmark_index_contract() {
    use ai::selection::{EYE_LEFT_EAR, EYE_RIGHT_EAR, FACEMESH_POINTS};
    assert_eq!(FACEMESH_POINTS, 478);
    for idx in EYE_LEFT_EAR.iter().chain(EYE_RIGHT_EAR.iter()) {
        assert!(*idx < 468, "EAR 索引必须落在 468 眼睑网格内：{idx}");
    }
    let overlap = EYE_LEFT_EAR
        .iter()
        .filter(|i| EYE_RIGHT_EAR.contains(i))
        .count();
    assert_eq!(overlap, 0);
}

/// ROI 旋转几何（合成关键点往返）：倾斜双眼线 → 相似矩阵把眼线旋平
/// （两眼映到同一 y）、框中心映到裁剪中心、眼距按 scale 缩放；逆映射
/// 逐点回到源坐标。
#[test]
fn eyes_roi_matrix_rotates_eye_line_horizontal_and_roundtrips() {
    use ai::selection::{face_roi_matrix, map_similarity_inv};
    let face = super_face(200.0, 200.0, 100.0, [160.0, 220.0], [240.0, 180.0]);
    let m = face_roi_matrix(&face, 256).unwrap();
    let apply = |p: [f32; 2]| -> [f32; 2] {
        [
            m[0][0] * p[0] + m[0][1] * p[1] + m[0][2],
            m[1][0] * p[0] + m[1][1] * p[1] + m[1][2],
        ]
    };
    let (l, r) = (apply([160.0, 220.0]), apply([240.0, 180.0]));
    assert!(
        (l[1] - r[1]).abs() < 1e-3,
        "眼线映后应水平：left {l:?} right {r:?}"
    );
    let center = apply([200.0, 200.0]);
    assert!((center[0] - 128.0).abs() < 1e-3 && (center[1] - 128.0).abs() < 1e-3);
    // 眼距缩放：源 80²+40² → sqrt(8000)；scale = 256 / (1.5×100)
    let src_dist = ((240.0f32 - 160.0f32).powi(2) + (180.0f32 - 220.0f32).powi(2)).sqrt();
    let dst_dist = ((r[0] - l[0]).powi(2) + (r[1] - l[1]).powi(2)).sqrt();
    assert!((dst_dist - src_dist * 256.0 / 150.0).abs() < 1e-2);
    // 往返：逆映射回源坐标
    for p in [
        [160.0, 220.0],
        [240.0, 180.0],
        [200.0, 200.0],
        [187.3, 211.9],
    ] {
        let mapped = apply(p);
        let back = map_similarity_inv(&m, mapped[0], mapped[1]).unwrap();
        assert!((back.0 - p[0]).abs() < 1e-2 && (back.1 - p[1]).abs() < 1e-2);
    }
    // 退化防御：零框 / 双眼点重合 → None
    let degenerate = super_face(100.0, 100.0, 0.0, [80.0, 90.0], [120.0, 90.0]);
    assert!(face_roi_matrix(&degenerate, 256).is_none());
    let merged = super_face(100.0, 100.0, 50.0, [80.0, 90.0], [80.0, 90.0]);
    assert!(face_roi_matrix(&merged, 256).is_none());
}

/// 三态聚合分带（纯函数）：任一 closed → closed（score = 闭眼置信，越闭
/// 越高）；[closed, maybe) → maybe；全睁 / 无人脸 → 无记录；有人脸但全
/// 不可判定 → unknown。
#[test]
fn eyes_aggregation_ear_bands() {
    use ai::selection::aggregate_eyes_ear;
    let (closed, maybe) = (0.16, 0.21);
    let (v, s) = aggregate_eyes_ear(&[Some(0.30), Some(0.10)], closed, maybe).unwrap();
    assert_eq!(v, "closed");
    assert!(
        (s - (0.21 - 0.10) / 0.21).abs() < 1e-6,
        "score = (maybe−ear)/maybe"
    );
    let (v, _) = aggregate_eyes_ear(&[Some(0.30), Some(0.18)], closed, maybe).unwrap();
    assert_eq!(v, "maybe");
    assert!(aggregate_eyes_ear(&[Some(0.30), Some(0.25)], closed, maybe).is_none());
    // 质量差的脸混入：取有效脸判，不因 None 拉爆
    let (v, _) = aggregate_eyes_ear(&[None, Some(0.12)], closed, maybe).unwrap();
    assert_eq!(v, "closed");
    let (v, s) = aggregate_eyes_ear(&[None, None], closed, maybe).unwrap();
    assert_eq!(v, "unknown");
    assert_eq!(
        s, 0.0,
        "unknown 行 score 记 0（前端契约：null 会整通道剔除）"
    );
    assert!(
        aggregate_eyes_ear(&[], closed, maybe).is_none(),
        "无人脸 → 无记录"
    );
}

/// 阈值快照：乱序写入自动保序（closed ≤ maybe）。
#[test]
fn eyes_threshold_snapshot_orders_swapped_input() {
    use ai::selection::{eyes_ear_closed, eyes_ear_maybe, set_eyes_ear_thresholds};
    set_eyes_ear_thresholds(0.30, 0.10);
    assert!(eyes_ear_closed() <= eyes_ear_maybe());
    assert!((eyes_ear_closed() - 0.10).abs() < 1e-6);
    set_eyes_ear_thresholds(0.13, 0.20); // 还原默认（真库标定值），防串扰他测
}

/// 默认阈值 pin（2026-09-28 真库标定工作点；变更需重标定并同步注释）。
#[test]
fn eyes_ear_defaults_are_calibrated() {
    let ai = settings::AiSettings::default();
    assert!((ai.eyes_ear_closed - 0.13).abs() < 1e-6);
    assert!((ai.eyes_ear_maybe - 0.20).abs() < 1e-6);
}

/// 合成 DetectedFace（box 中心 + 双眼点；测试几何用）。
fn super_face(
    cx: f32,
    cy: f32,
    side: f32,
    left_eye: [f32; 2],
    right_eye: [f32; 2],
) -> ai::face::DetectedFace {
    ai::face::DetectedFace {
        box_x: cx - side / 2.0,
        box_y: cy - side / 2.0,
        box_w: side,
        box_h: side,
        score: 0.9,
        kps: [
            left_eye,
            right_eye,
            [cx, cy + 20.0],
            [cx - 15.0, cy + 40.0],
            [cx + 15.0, cy + 40.0],
        ],
    }
}

/// 三态落库 + 筛选/详情读取（聚合结果 → ai_analysis 全链路）。
#[test]
fn eyes_tri_state_roundtrip_through_db() {
    use ai::selection::aggregate_eyes_ear;
    let (dir, db, state) = setup();
    let photos = dir.path().join("photos");
    let a = ins(&db, &photos, "a.jpg", &sharp_image());
    let b = ins(&db, &photos, "b.jpg", &sharp_image());
    let c = ins(&db, &photos, "c.jpg", &sharp_image());
    let d = ins(&db, &photos, "d.jpg", &sharp_image());
    let (closed_th, maybe_th) = (0.16, 0.21);
    for (id, ears) in [
        (a, vec![Some(0.08)]),       // closed
        (b, vec![Some(0.19)]),       // maybe
        (c, vec![Some(0.35)]),       // 全睁 → 无记录
        (d, vec![None, Some(0.05)]), // 混合 → closed
    ] {
        if let Some((value, score)) = aggregate_eyes_ear(&ears, closed_th, maybe_th) {
            db.set_ai_analysis(
                id,
                "eyes",
                Some(&value),
                Some(score),
                ai::selection::EYES_ALGO_VERSION,
            )
            .unwrap();
        }
    }
    let row = |id| {
        analysis(&db, id, "eyes")
            .map(|(v, s)| (v.unwrap(), s.unwrap()))
            .unwrap()
    };
    assert_eq!(row(a).0, "closed");
    assert!(row(a).1 > 0.0 && row(a).1 <= 1.0);
    assert_eq!(row(b).0, "maybe");
    assert!(analysis(&db, c, "eyes").is_none(), "全睁不落记录");
    assert_eq!(row(d).0, "closed");
    // 筛选维度：closed / maybe 各自命中
    let page = |eyes: &str| {
        ipc::assets::fetch_assets_page(
            &state,
            0,
            50,
            AssetFilters {
                eyes: Some(eyes.into()),
                ..Default::default()
            },
        )
        .unwrap()
        .into_iter()
        .map(|dto| dto.name)
        .collect::<Vec<_>>()
    };
    let mut closed_hits = page("closed");
    closed_hits.sort();
    assert_eq!(closed_hits, vec!["a.jpg".to_string(), "d.jpg".to_string()]);
    assert_eq!(page("maybe"), vec!["b.jpg".to_string()]);
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

    // 模型未下载（清单 facemesh 条目无落盘文件）：回填跳过、任务保持 pending
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
    // eyes EAR 阈值参与指纹（变更 → eyes 任务重排，分析结果随阈值变）
    ai.eyes_ear_closed = 0.12;
    let f3 = ipc::indexing::selection_params_fingerprint(&ai);
    assert_ne!(f2, f3);
    ai.eyes_ear_maybe = 0.25;
    let f4 = ipc::indexing::selection_params_fingerprint(&ai);
    assert_ne!(f3, f4);

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
fn catalog_selection_entry_is_pinned_facemesh() {
    // 0021 eyes 通道实装契约（2026-09-28）：清单必须收录 facemesh
    // （MediaPipe Face Landmarker 478 点 ONNX，Apache-2.0——license 证据
    // 见 ai::mod CATALOG 注释），sha/bytes pin 死防漂移。
    let entry = ai::catalog()
        .iter()
        .find(|e| e.id == "facemesh")
        .expect("清单缺 facemesh（闭眼通道依赖）");
    assert_eq!(entry.feature, "selection");
    assert_eq!(
        entry.sha256,
        "111795f8703cdeb6d0c68a9f3cc966a0f23f8786bb00f4577a11f461fc4276ac"
    );
    assert_eq!(entry.bytes_total, 4_864_717);
    assert!(entry.url.starts_with("https://github.com/yakhyo/"));
    // Production localization; the dedicated candidate is evaluation-only.
    assert_eq!(
        ai::catalog()
            .iter()
            .filter(|e| e.feature == "selection")
            .count(),
        1
    );
    // 全量 feature 集合：semantic + face + selection
    let mut features: Vec<&str> = ai::catalog().iter().map(|e| e.feature.as_str()).collect();
    features.sort_unstable();
    features.dedup();
    assert_eq!(
        features,
        vec!["evaluation", "face", "selection", "semantic"]
    );
}

// ---------------------------------------------------------------------------
// 真机标定 + 验收（方法同语义阈值轮 fp16_semantic_calibration_and_throughput）
// ---------------------------------------------------------------------------
//
// - 模型根 = 应用真实 models 目录（%APPDATA%\photohub\models，
//   SMARTPHOTO_EYES_MODELS 可覆盖）；库 = 真实主库 `I:\SmartPhoto\主库`
//   （SMARTPHOTO_EYES_LIBRARY 可覆盖）。
// - 标定（默认，只读不写库）：全库逐张 SCRFD 检测 → facemesh EAR →
//   双眼 min EAR 分布直方图 + 最低/最高各 8 张路径（人工肉眼核验睁/闭
//   真值）+ 吞吐。工作点取「睁眼主体带下沿（maybe）/闭眼长尾上沿
//   （closed）」分隔带，写回 settings 默认值 + selection.rs 快照初值。
// - 全量落库（SMARTPHOTO_EYES_APPLY=1）：走生产入口 run_eyes_backfill
//   消费真库 eyes 任务账，closed/maybe/unknown 分布落 ai_analysis，
//   供人物页/UI 抽查。
//
// 跑法：
//   cargo test --test ai_selection_test eyes_ear_calibration -- --ignored --nocapture
//   SMARTPHOTO_EYES_APPLY=1 cargo test --test ai_selection_test eyes_ear_calibration -- --ignored --nocapture
#[test]
#[ignore = "真机标定：需 facemesh + scrfd 已下载到应用 models 目录 + 真实主库"]
fn eyes_ear_calibration_and_throughput() {
    use std::time::Instant;

    use ai::selection::FaceEyeStateClassifier;

    let models = std::env::var("SMARTPHOTO_EYES_MODELS")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("APPDATA")
                .map(|appdata| std::path::PathBuf::from(appdata).join(r"photohub\models"))
                .expect("APPDATA")
        });
    let db_dir = std::env::var("SMARTPHOTO_EYES_LIBRARY")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from(r"I:\SmartPhoto\主库"));
    for id in ["facemesh", "scrfd"] {
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
    manager.set_ai_params(ai::AiIndexParams {
        use_gpu,
        ..ai::AiIndexParams::default()
    });
    let db = ipc::open_library_db(&db_dir).expect("打开真实主库");
    let rows: Vec<(i64, String)> =
        db.0.prepare("SELECT id, path FROM assets WHERE kind IN ('photo','raw') ORDER BY id")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
    assert!(!rows.is_empty(), "真库无照片资产");
    eprintln!("标定库：{} 张（{}）", rows.len(), db_dir.display());

    let classifier = ai::selection::FacemeshEarClassifier::new(&manager);
    let t0 = Instant::now();
    let mut results: Vec<(i64, String, Option<f32>)> = Vec::with_capacity(rows.len());
    let mut no_face = 0usize;
    let mut detect_fail = 0usize;
    for (id, path) in &rows {
        let src = std::path::Path::new(path);
        let Some(thumb) = ai::face::detection_source(&db_dir, src) else {
            detect_fail += 1;
            continue;
        };
        let Ok(img) = image::ImageReader::open(&thumb)
            .map_err(|e| e.to_string())
            .and_then(|r| r.decode().map_err(|e| e.to_string()))
            .map(|d| d.to_rgb8())
        else {
            detect_fail += 1;
            continue;
        };
        match ai::face::detect_faces(&manager, &img) {
            Ok(faces) if faces.is_empty() => {
                no_face += 1;
                results.push((*id, path.clone(), None));
            }
            Ok(faces) => {
                let mut worst = f32::INFINITY;
                let mut any_valid = false;
                for face in faces.iter().take(ai::face::MAX_FACES_PER_IMAGE) {
                    match classifier.face_min_ear(&img, face) {
                        Ok(Some(ear)) => {
                            worst = worst.min(ear);
                            any_valid = true;
                        }
                        Ok(None) => {}
                        Err(e) => eprintln!("  asset {id} 推理失败: {e}"),
                    }
                }
                results.push((*id, path.clone(), any_valid.then_some(worst)));
            }
            Err(_) => detect_fail += 1,
        }
    }
    let elapsed = t0.elapsed().as_secs_f64();
    eprintln!(
        "eyes 标定吞吐：{:.1} img/s（{} 张 / {:.1}s；无人脸 {no_face}，取图/检测失败 {detect_fail}）",
        results.len() as f64 / elapsed,
        results.len(),
        elapsed
    );

    // —— EAR 分布直方图（0.02 步进；None 单列 = unknown 维度）——
    let mut hist = std::collections::BTreeMap::new();
    let mut unknown = 0usize;
    for (_, _, ear) in &results {
        match ear {
            None => unknown += 1,
            Some(e) => {
                let bucket = ((e / 0.02).floor() as i32).max(0).min(30);
                *hist.entry(bucket).or_insert(0usize) += 1;
            }
        }
    }
    eprintln!("EAR 分布（None/unknown = {unknown}）：");
    for (bucket, n) in &hist {
        let lo = *bucket as f32 * 0.02;
        eprintln!("  [{lo:0.2}, {}) {}", lo + 0.02, "#".repeat(*n));
    }
    // 分位数
    let mut sorted: Vec<f32> = results.iter().filter_map(|(_, _, e)| *e).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let q =
        |p: f64| -> f32 { sorted[((sorted.len() as f64 - 1.0) * p).round() as usize].to_owned() };
    eprintln!(
        "EAR 分位：min={:.3} p1={:.3} p5={:.3} p10={:.3} p25={:.3} p50={:.3} p75={:.3} max={:.3}（n={}）",
        sorted.first().copied().unwrap_or(0.0),
        q(0.01),
        q(0.05),
        q(0.10),
        q(0.25),
        q(0.50),
        q(0.75),
        sorted.last().copied().unwrap_or(0.0),
        sorted.len()
    );
    // 最低 8 / 最高 4 张（人工肉眼核验睁闭真值）
    let mut by_ear = results
        .iter()
        .filter_map(|(id, p, e)| e.map(|v| (*id, p.clone(), v)))
        .collect::<Vec<_>>();
    by_ear.sort_by(|a, b| a.2.total_cmp(&b.2));
    eprintln!("最低 EAR（疑似闭眼长尾，应逐张肉眼确认为真闭眼）：");
    for (id, p, v) in by_ear.iter().take(8) {
        eprintln!("  {v:.3}  asset={id}  {}", p);
    }
    eprintln!("最高 EAR（确定睁眼，正脸睁眼不应误报）：");
    for (id, p, v) in by_ear.iter().rev().take(4) {
        eprintln!("  {v:.3}  asset={id}  {}", p);
    }
    eprintln!("工作点建议：闭眼长尾上沿 = closed 阈；睁眼主体带下沿 = maybe 阈");
    // 逐资产 EAR dump（分隔带取样核验用；SMARTPHOTO_EYES_DUMP=1 开启）
    if std::env::var("SMARTPHOTO_EYES_DUMP").as_deref() == Ok("1") {
        let tsv = std::env::temp_dir().join("smartphoto_ears.tsv");
        let mut body = String::new();
        for (id, path, ear) in &results {
            let ear = ear
                .map(|v| format!("{v:.4}"))
                .unwrap_or_else(|| "NA".into());
            body.push_str(&format!("{id}\t{ear}\t{path}\n"));
        }
        std::fs::write(&tsv, body).expect("写 ears.tsv");
        eprintln!("EAR dump：{}", tsv.display());
    }

    // —— 全量落库（生产入口，写 ai_analysis + 消费 eyes 任务账）——
    if std::env::var("SMARTPHOTO_EYES_APPLY").as_deref() == Ok("1") {
        let bus = events::EventBus::new();
        let supervisor = tasks::TaskSupervisor::new(bus.clone());
        let manager = ai::ModelManager::new(
            std::env::var("SMARTPHOTO_EYES_MODELS")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    std::env::var("APPDATA")
                        .map(|a| std::path::PathBuf::from(a).join(r"photohub\models"))
                        .expect("APPDATA")
                }),
            bus.clone(),
            supervisor,
        );
        manager.set_ai_params(ai::AiIndexParams {
            use_gpu,
            ..ai::AiIndexParams::default()
        });
        let t1 = Instant::now();
        let done = ai::selection::run_eyes_backfill(&db_dir, &manager, &bus);
        eprintln!(
            "eyes 全量回填：{done} 张成功 / {:.1}s（{:.1} img/s）",
            t1.elapsed().as_secs_f64(),
            done as f64 / t1.elapsed().as_secs_f64()
        );
        let dist: Vec<(String, i64)> = db
            .0
            .prepare("SELECT value, COUNT(*) FROM ai_analysis WHERE kind='eyes' GROUP BY value ORDER BY value")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
        eprintln!("ai_analysis('eyes') 值分布：{dist:?}");
        let tasks: Vec<(String, i64)> = db
            .0
            .prepare("SELECT state, COUNT(*) FROM index_tasks WHERE kind='eyes' GROUP BY state")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .flatten()
            .collect();
        eprintln!("eyes 任务账：{tasks:?}");
        // 抽查样本：closed/maybe 各 5 张（人物页肉眼核验入口）
        for value in ["closed", "maybe"] {
            let sample: Vec<(i64, String, f64)> =
                db.0.prepare(
                    "SELECT a.id, a.path, n.score FROM ai_analysis n \
                     JOIN assets a ON a.id = n.asset_id \
                     WHERE n.kind='eyes' AND n.value = ?1 ORDER BY n.score LIMIT 5",
                )
                .unwrap()
                .query_map([value], |r| {
                    Ok((
                        r.get(0)?,
                        r.get(1)?,
                        r.get::<_, Option<f64>>(2)?.unwrap_or(0.0),
                    ))
                })
                .unwrap()
                .flatten()
                .collect();
            eprintln!("{value} 抽查（score 升序 5 张）：");
            for (id, p, s) in sample {
                eprintln!("  {s:.3}  asset={id}  {p}");
            }
        }
    }
}

#[test]
fn regional_focus_distinguishes_edges_from_defocus_and_plain_gradients() {
    let stripes = image::GrayImage::from_fn(256, 256, |x, _| {
        image::Luma([if (x / 40) % 2 == 0 { 16 } else { 240 }])
    });
    let blurred = image::imageops::blur(&stripes, 5.0);
    let sharp = ai::focus_quality::measure(&stripes, 30.0);
    let soft = ai::focus_quality::measure(&blurred, 30.0);
    assert_eq!(sharp.state, "sharp");
    assert_eq!(soft.state, "soft");
    assert!(sharp.score.unwrap() > soft.score.unwrap());
    let gradient = image::GrayImage::from_fn(256, 256, |_, y| image::Luma([y as u8]));
    let unknown = ai::focus_quality::measure(&gradient, 30.0);
    assert_eq!(unknown.state, "unknown");
    assert_eq!(unknown.reason, Some("low_texture"));
    assert!(unknown.score.is_none());
}

#[test]
fn binary_eyes_have_no_quality_or_ambiguous_band_after_detection() {
    use ai::selection_regions::{binary_eye_state, detected_eye_bounds, photo_eyes, Region};
    let image = image::RgbImage::new(64, 64); // No texture: no separate quality gate.
    let points = |width: f32| {
        [
            [10.0, 10.0],
            [11.0, 10.2],
            [12.0, 10.2],
            [10.0 + width, 10.0],
            [12.0, 10.1],
            [11.0, 10.1],
        ]
    };
    assert!(detected_eye_bounds(&image, &points(4.0)).is_some());
    assert!(detected_eye_bounds(&image, &points(3.9)).is_none());
    for ear in [0.0, 0.1, 0.129, 0.13, 0.15, 0.2, 0.3, 0.9] {
        assert_eq!(
            binary_eye_state(ear, 0.13),
            if ear < 0.13 { "closed" } else { "open" }
        );
    }
    let missing = Region {
        kind: "eye".into(),
        person: 1,
        side: None,
        bounds: [0.0; 4],
        state: "not_detected".into(),
        reason: Some("eye_not_detected".into()),
        raw_score: None,
        auxiliary_ear: None,
    };
    assert_eq!(photo_eyes(&[missing], false), "no_eye");
}
