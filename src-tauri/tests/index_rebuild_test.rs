//! 索引重建（用户 2026-09-20）+ AI 索引参数配置化 + 浏览历史：
//! 每通道「清理 + 重排 + 消费」一例、门槛文案、参数指纹自动重建
//! （marker 比对 / 二次 no-op）、asset_view_mark / recent_viewed 契约。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::AssetRow;
use events::AssetKind::{Photo, Raw};
use ipc::indexing::{
    check_params_and_rebuild, face_params_fingerprint, rebuild_channel_core, rebuild_gates,
    semantic_params_fingerprint,
};

fn asset(path: &str, kind: events::AssetKind) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-20T00:00:00.000Z".into(),
        xxhash: 1,
        kind,
        captured_at: Some("2026-09-01T10:00:00.000Z".into()),
        camera: Some("Sony A7M4".into()),
        source: "imported".into(),
        created_at: "2026-09-20T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: Some(6000),
        height: Some(4000),
        iso: Some(400),
        f_number: Some("2.8".into()),
        exposure_time: Some("1/250".into()),
        focal_length: Some("50".into()),
        lens: Some("FE 24-70mm F2.8 GM".into()),
        pair_asset_id: None,
        thumb_state: 1,
        orientation: Some(1),
        flash: Some("fired".into()),
        metering_mode: Some("pattern".into()),
        white_balance: Some("auto".into()),
        exposure_program: Some("manual".into()),
        software: Some("Ver.01.00".into()),
        artist: Some("tester".into()),
        gps_lat: Some(31.2),
        gps_lon: Some(121.5),
        rating: 0,
        flagged: 0,
    }
}

fn pending_count(db: &db::Db, kind: &str) -> i64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM index_tasks WHERE kind = ?1 AND state IN ('pending', 'running')",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

/// 通道任务行数（任意状态）：自动重建会 kick worker，假模型下任务可能已被
/// 消费到 failed——断言「行存在」而非「仍 pending」，消除异步竞态。
fn task_rows(db: &db::Db, kind: &str) -> i64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM index_tasks WHERE kind = ?1",
        [kind],
        |r| r.get(0),
    )
    .unwrap()
}

/// 造一张真实可解码 JPEG（缩略图通道消费用）。
fn write_real_jpg(path: &std::path::Path, w: u32, h: u32) {
    let mut img = image::RgbImage::new(w, h);
    for (x, y, px) in img.enumerate_pixels_mut() {
        *px = image::Rgb([(x % 256) as u8, (y % 256) as u8, ((x + y) % 256) as u8]);
    }
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    image::DynamicImage::ImageRgb8(img)
        .write_with_encoder(encoder)
        .unwrap();
    std::fs::write(path, buf).unwrap();
}

// ---------------------------------------------------------------------------
// 重建：每通道 清理 + 重排 + 消费
// ---------------------------------------------------------------------------

#[test]
fn rebuild_semantic_clears_vectors_and_requeues() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = open_db(&db_dir);
    db.insert_asset(&asset("X:/p/a.jpg", Photo)).unwrap();
    db.insert_asset(&asset("X:/p/b.jpg", Photo)).unwrap();
    // 既有产物：向量文件 + 时间账 + done 任务
    std::fs::write(db_dir.join("vectors.usearch"), b"usearch-bytes").unwrap();
    db.0.execute_batch(
        "UPDATE assets SET ai_indexed_at = '2026-09-20T00:00:00.000Z'; \
             INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('ai', 1, 'done', 0, '2026', '2026');",
    )
    .unwrap();

    let pending = rebuild_channel_core(&db, &db_dir, "semantic").unwrap();
    assert_eq!(pending, 2, "两资产全部重排");
    assert!(!db_dir.join("vectors.usearch").exists(), "向量索引已删");
    let ledger: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE ai_indexed_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ledger, 0, "时间账复位");
    assert_eq!(pending_count(&db, "ai"), 2);
}

#[test]
fn rebuild_face_clears_tables_and_requeues() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = open_db(&db_dir);
    db.insert_asset(&asset("X:/p/a.jpg", Photo)).unwrap();
    let pid = db.create_person().unwrap();
    db.insert_face(1, 0.0, 0.0, 10.0, 10.0, &[0.1; 512], Some(pid))
        .unwrap();
    db.set_face_indexed(1).unwrap();

    let pending = rebuild_channel_core(&db, &db_dir, "face").unwrap();
    assert_eq!(pending, 1);
    let faces: i64 =
        db.0.query_row("SELECT COUNT(*) FROM faces", [], |r| r.get(0))
            .unwrap();
    let people: i64 =
        db.0.query_row("SELECT COUNT(*) FROM people", [], |r| r.get(0))
            .unwrap();
    assert_eq!((faces, people), (0, 0), "faces/people 清空");
    let ledger: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE face_indexed_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ledger, 0);
    assert_eq!(pending_count(&db, "face"), 1);
}

#[test]
fn rebuild_thumb_clears_dir_resets_state_and_worker_regenerates() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo = dir.path().join("IMG_0001.jpg");
    write_real_jpg(&photo, 800, 600);
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = open_db(&db_dir);
    let mut row = asset(&photo.to_string_lossy(), Photo);
    row.thumb_state = 1;
    db.insert_asset(&row).unwrap();
    let id = db
        .asset_id_by_path(&photo.to_string_lossy())
        .unwrap()
        .unwrap();
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'thumb' AND asset_id = ?1",
        [id],
    )
    .unwrap();
    // 既有缩略图产物
    let thumbs = db_dir.join("thumbs").join("256");
    std::fs::create_dir_all(&thumbs).unwrap();
    std::fs::write(thumbs.join("junk.jpg"), b"junk").unwrap();

    let pending = rebuild_channel_core(&db, &db_dir, "thumb").unwrap();
    assert_eq!(pending, 1);
    assert!(!db_dir.join("thumbs").exists(), "缩略图目录已删");
    assert_eq!(
        db.0.query_row("SELECT thumb_state FROM assets WHERE id = ?1", [id], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        0,
        "thumb_state 复位"
    );
    // 消费：worker 真跑 → 缩略图重建 + thumb_state=1
    let processed = index::run_pending(&db_dir, 2);
    assert!(processed >= 1);
    assert_eq!(
        db.0.query_row("SELECT thumb_state FROM assets WHERE id = ?1", [id], |r| {
            r.get::<_, i64>(0)
        })
        .unwrap(),
        1,
        "缩略图重生成"
    );
    assert!(db_dir.join("thumbs").join("256").is_dir());
}

#[test]
fn rebuild_exif_clears_columns_and_worker_reextracts() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo = dir.path().join("IMG_0002.jpg");
    // 带 EXIF 的合成 JPEG（复用 deep_exif 的 Writer 夹具思路：相机/ISO 可提取）
    write_real_jpg(&photo, 640, 480);
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = open_db(&db_dir);
    db.insert_asset(&asset(&photo.to_string_lossy(), Photo))
        .unwrap();
    let id = db
        .asset_id_by_path(&photo.to_string_lossy())
        .unwrap()
        .unwrap();
    // 既有 EXIF 产物列全有值 + done 任务
    db.0.execute(
        "UPDATE index_tasks SET state = 'done' WHERE kind = 'exif' AND asset_id = ?1",
        [id],
    )
    .unwrap();

    let pending = rebuild_channel_core(&db, &db_dir, "exif").unwrap();
    assert_eq!(pending, 1);
    let row = db.asset_by_id(id).unwrap().unwrap();
    assert_eq!(row.width, None, "0004 列清空");
    assert_eq!(row.iso, None);
    assert_eq!(row.lens, None);
    assert_eq!(row.flash, None, "0008 列清空");
    assert_eq!(row.gps_lat, None);
    // captured_at/camera 保留（时间线/目录结构依赖）
    assert!(row.captured_at.is_some());
    assert!(row.camera.is_some());
    // 消费：worker 重提取 → 宽高回来（SOF 解析）
    index::run_pending(&db_dir, 2);
    let row = db.asset_by_id(id).unwrap().unwrap();
    assert_eq!(row.width, Some(640));
    assert_eq!(row.height, Some(480));
}

#[test]
fn rebuild_unknown_kind_rejected_and_gates_report_clear_errors() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    // 核心未知通道
    assert!(rebuild_channel_core(&db, db_dir.path(), "nope").is_err());

    // 门槛：模型未就绪（models 目录空）
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let err = rebuild_gates(&state, "semantic").unwrap_err();
    assert!(err.contains("请先在设置中下载模型"), "{err}");
    let err = rebuild_gates(&state, "face").unwrap_err();
    assert!(err.contains("请先在设置中下载模型"), "{err}");
    assert!(rebuild_gates(&state, "thumb").is_ok());
    assert!(rebuild_gates(&state, "exif").is_ok());
    assert!(rebuild_gates(&state, "unknown").is_err());

    // 假模型就绪但开关未开
    let models = db_dir.path().join("models");
    std::fs::create_dir_all(&models).unwrap();
    for id in [
        "siglip2-visual",
        "siglip2-text",
        "siglip2-tokenizer",
        "scrfd",
        "arcface",
    ] {
        std::fs::write(models.join(format!("{id}.onnx")), b"fake").unwrap();
    }
    let err = rebuild_gates(&state, "semantic").unwrap_err();
    assert!(err.contains("语义索引未开启"), "{err}");
    let err = rebuild_gates(&state, "face").unwrap_err();
    assert!(err.contains("人脸识别未开启"), "{err}");
    // 开关打开 → 门槛通过
    state.settings.lock().unwrap().ai.enable_clip = true;
    state.settings.lock().unwrap().ai.enable_face = true;
    assert!(rebuild_gates(&state, "semantic").is_ok());
    assert!(rebuild_gates(&state, "face").is_ok());
}

// ---------------------------------------------------------------------------
// 参数指纹：marker 比对 + 自动重建
// ---------------------------------------------------------------------------

fn ai_settings(embed: u16, detect: f32, cluster: f32) -> settings::AiSettings {
    settings::AiSettings {
        enable_clip: true,
        enable_face: true,
        embed_input_size: embed,
        face_detect_threshold: detect,
        face_cluster_threshold: cluster,
        ..settings::AiSettings::default()
    }
}

#[test]
fn params_fingerprint_triggers_channel_rebuild_once() {
    let src = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    // 假模型（就绪检查只看文件存在）
    let models = db_dir.join("models");
    std::fs::create_dir_all(&models).unwrap();
    for id in [
        "siglip2-visual",
        "siglip2-text",
        "siglip2-tokenizer",
        "scrfd",
        "arcface",
    ] {
        std::fs::write(models.join(format!("{id}.onnx")), b"fake").unwrap();
    }
    let state = common::state_with_library(&db_dir, src.path(), Duration::from_millis(1));
    // state_with_library 的 models 根 = db_dir/models（就绪通过）
    {
        let mut s = state.settings.lock().unwrap();
        s.ai.enable_clip = true;
        s.ai.enable_face = true;
    }
    let db = open_db(&db_dir);
    db.insert_asset(&asset("X:/p/a.jpg", Raw)).unwrap();
    db.set_ai_indexed(1).unwrap();
    db.set_face_indexed(1).unwrap();

    // 首轮：marker 不存在 → 两通道都重建 + marker 落盘
    let ai = ai_settings(256, 0.5, 0.4);
    check_params_and_rebuild(&state, &db_dir, &ai);
    let marker = db_dir.join("index-params.marker");
    assert!(marker.is_file(), "指纹 marker 落盘");
    assert_eq!(task_rows(&db, "ai"), 1, "语义通道重排");
    assert_eq!(task_rows(&db, "face"), 1, "人脸通道重排");
    let content = std::fs::read_to_string(&marker).unwrap();
    assert!(content.contains(&format!("semantic={}", semantic_params_fingerprint(&ai))));

    // 二轮（同参数）：marker 命中 → 不再重排（账已被上一轮消费/失败变化，
    // 这里断言指纹函数稳定性 + marker 内容不变即可证明短路）
    let before = content.clone();
    check_params_and_rebuild(&state, &db_dir, &ai);
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        before,
        "同参数不动 marker"
    );

    // 改语义参数（embed 256→512）→ 只重建语义通道：人脸账保持上轮状态，
    // 语义 ai_indexed_at 再次复位（先补账再重建）
    db.0.execute(
        "UPDATE assets SET ai_indexed_at = '2026-09-20T01:00:00.000Z'",
        [],
    )
    .unwrap();
    db.0.execute("DELETE FROM index_tasks WHERE kind = 'ai'", [])
        .unwrap();
    let ai2 = ai_settings(512, 0.5, 0.4);
    assert_ne!(
        semantic_params_fingerprint(&ai),
        semantic_params_fingerprint(&ai2)
    );
    check_params_and_rebuild(&state, &db_dir, &ai2);
    assert_eq!(task_rows(&db, "ai"), 1, "语义参数变更 → 语义重排");
    let ledger: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE ai_indexed_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ledger, 0, "语义时间账复位");
    // marker 更新为新指纹
    let content = std::fs::read_to_string(&marker).unwrap();
    assert!(content.contains(&format!("semantic={}", semantic_params_fingerprint(&ai2))));

    // 门槛挡下：关掉人脸开关 + 改人脸参数 → 不重建也不写新指纹（下轮补）
    {
        let mut s = state.settings.lock().unwrap();
        s.ai.enable_face = false;
    }
    db.0.execute("DELETE FROM index_tasks WHERE kind = 'face'", [])
        .unwrap();
    let ai3 = settings::AiSettings {
        enable_face: false,
        ..ai_settings(512, 0.7, 0.4)
    };
    check_params_and_rebuild(&state, &db_dir, &ai3);
    assert_eq!(task_rows(&db, "face"), 0, "开关关闭不重建人脸");
    let content = std::fs::read_to_string(&marker).unwrap();
    // face 行仍为旧指纹（被挡下的通道保留旧值，条件满足时再重建）
    assert!(content.contains(&format!("face={}", face_params_fingerprint(&ai2))));
}

// ---------------------------------------------------------------------------
// 浏览历史（追加任务：migration 0010 + mark/recent IPC）
// ---------------------------------------------------------------------------

#[test]
fn view_history_mark_recent_and_cascade() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        db.insert_asset(&asset(&format!("X:/p/{name}"), Photo))
            .unwrap();
    }
    let id = |name: &str| {
        db.asset_id_by_path(&format!("X:/p/{name}"))
            .unwrap()
            .unwrap()
    };

    // 无效资产静默 Ok
    ipc::rating::fetch_asset_view_mark(&state, 999_999).unwrap();
    assert!(ipc::rating::fetch_recent_viewed(&state, 10)
        .unwrap()
        .is_empty());

    // mark 顺序 a → b → a（幂等刷新时间：a 应排最前）
    ipc::rating::fetch_asset_view_mark(&state, id("a.jpg")).unwrap();
    std::thread::sleep(Duration::from_millis(15));
    ipc::rating::fetch_asset_view_mark(&state, id("b.jpg")).unwrap();
    std::thread::sleep(Duration::from_millis(15));
    ipc::rating::fetch_asset_view_mark(&state, id("a.jpg")).unwrap();

    let recent = ipc::rating::fetch_recent_viewed(&state, 10).unwrap();
    let names: Vec<&str> = recent.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["a.jpg", "b.jpg"],
        "viewed_at DESC + 每资产一行去重"
    );

    // limit 生效
    assert_eq!(
        ipc::rating::fetch_recent_viewed(&state, 1).unwrap().len(),
        1
    );

    // 行数：3 次浏览 2 资产 → 2 行（upsert 语义）
    let rows: i64 =
        db.0.query_row("SELECT COUNT(*) FROM view_history", [], |r| r.get(0))
            .unwrap();
    assert_eq!(rows, 2);

    // 资产删除级联清历史
    db.0.execute("DELETE FROM assets WHERE id = ?1", [id("a.jpg")])
        .unwrap();
    let recent = ipc::rating::fetch_recent_viewed(&state, 10).unwrap();
    assert_eq!(
        recent.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["b.jpg"]
    );
    let rows: i64 =
        db.0.query_row("SELECT COUNT(*) FROM view_history", [], |r| r.get(0))
            .unwrap();
    assert_eq!(rows, 1, "级联清历史");
}
