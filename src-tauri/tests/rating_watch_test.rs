//! 评分 / 收藏 / 最近添加 / XMP 边车 / 监视文件夹（M5 + F4 v1）：
//! migration 0009 默认值、评分写入与校验、筛选条件（rating/flagged/favorite）、
//! XMP 边车异步同步（LR 互通 + 存量回填）、watch_folders 设置与轮询自动入册。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::{AssetFilters, AssetRow};
use events::AssetKind;
use metadata::xmp;

fn asset(path: &str, created_at: &str) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-20T00:00:00.000Z".into(),
        xxhash: 1,
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: created_at.into(),
        origin: "imported".into(),
        width: None,
        height: None,
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
    }
}

fn wait_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pred()
}

// ---------------------------------------------------------------------------
// 最近添加（任务 A）
// ---------------------------------------------------------------------------

#[test]
fn recent_assets_orders_by_created_at_desc_with_keyset() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    db.insert_asset(&asset("X:/p/a.jpg", "2026-09-01T10:00:00.000Z"))
        .unwrap();
    db.insert_asset(&asset("X:/p/b.jpg", "2026-09-03T10:00:00.000Z"))
        .unwrap();
    db.insert_asset(&asset("X:/p/c.jpg", "2026-09-02T10:00:00.000Z"))
        .unwrap();
    // 同 created_at → id DESC tiebreak（后插入的排前）
    db.insert_asset(&asset("X:/p/d1.jpg", "2026-09-03T10:00:00.000Z"))
        .unwrap();

    // 第一页（limit 3）：b/d1（09-03，id 大的在前）→ c
    let page1 = ipc::rating::fetch_recent_assets(&state, 0, 3).unwrap();
    let names: Vec<&str> = page1.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["d1.jpg", "b.jpg", "c.jpg"]);
    // keyset 第二页：after = c 的 id → a
    let after = page1.last().unwrap().id;
    let page2 = ipc::rating::fetch_recent_assets(&state, after, 3).unwrap();
    assert_eq!(
        page2.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["a.jpg"]
    );
    // 游标行已删（不存在的大 id）→ 按第一页回退
    let fallback = ipc::rating::fetch_recent_assets(&state, 999_999, 2).unwrap();
    assert_eq!(fallback.len(), 2);
    assert_eq!(fallback[0].name, "d1.jpg");
}

// ---------------------------------------------------------------------------
// 评分 / 收藏：写入 + 校验 + 筛选
// ---------------------------------------------------------------------------

#[test]
fn rating_and_flag_set_with_validation_and_filters() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"] {
        db.insert_asset(&asset(&format!("X:/p/{name}"), "2026-09-01T10:00:00.000Z"))
            .unwrap();
    }
    let id = |name: &str| {
        db.asset_id_by_path(&format!("X:/p/{name}"))
            .unwrap()
            .unwrap()
    };

    // 校验：0-5 之外拒绝
    assert!(ipc::rating::fetch_asset_rating_set(&state, id("a.jpg"), 6).is_err());
    assert!(ipc::rating::fetch_asset_rating_set(&state, id("a.jpg"), -1).is_err());
    // 不存在的资产
    assert!(ipc::rating::fetch_asset_rating_set(&state, 999_999, 3).is_err());
    assert!(ipc::rating::fetch_asset_flag_set(&state, 999_999, true).is_err());

    // 写入：a=5、b=2、c=0（未评）、d=旗标
    ipc::rating::fetch_asset_rating_set(&state, id("a.jpg"), 5).unwrap();
    ipc::rating::fetch_asset_rating_set(&state, id("b.jpg"), 2).unwrap();
    ipc::rating::fetch_asset_flag_set(&state, id("d.jpg"), true).unwrap();

    // rating 默认 0 / flagged 默认 0（migration 0009 契约）
    let row = db.asset_by_id(id("c.jpg")).unwrap().unwrap();
    assert_eq!(row.rating, 0);
    assert_eq!(row.flagged, 0);
    let row = db.asset_by_id(id("a.jpg")).unwrap().unwrap();
    assert_eq!(row.rating, 5);
    assert_eq!(db.asset_by_id(id("d.jpg")).unwrap().unwrap().flagged, 1);

    // 清除评分 → 0
    ipc::rating::fetch_asset_rating_set(&state, id("a.jpg"), 0).unwrap();
    assert_eq!(db.asset_by_id(id("a.jpg")).unwrap().unwrap().rating, 0);
    ipc::rating::fetch_asset_rating_set(&state, id("a.jpg"), 5).unwrap();

    // 筛选：ratingMin
    let page = |filters: AssetFilters| {
        db.assets_page(0, 50, &filters)
            .unwrap()
            .iter()
            .map(|r| r.filename.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        page(AssetFilters {
            rating_min: Some(1),
            ..Default::default()
        }),
        vec!["b.jpg".to_string(), "a.jpg".to_string()]
    );
    // rating 区间
    assert_eq!(
        page(AssetFilters {
            rating_min: Some(2),
            rating_max: Some(4),
            ..Default::default()
        }),
        vec!["b.jpg".to_string()]
    );
    // 旗标
    assert_eq!(
        page(AssetFilters {
            flagged: Some(true),
            ..Default::default()
        }),
        vec!["d.jpg".to_string()]
    );
    assert_eq!(
        page(AssetFilters {
            flagged: Some(false),
            ..Default::default()
        })
        .len(),
        3
    );
    // 收藏页组合：rating>0 OR flagged=1
    assert_eq!(
        page(AssetFilters {
            favorite: Some(true),
            ..Default::default()
        }),
        vec![
            "d.jpg".to_string(),
            "b.jpg".to_string(),
            "a.jpg".to_string()
        ]
    );
    // 非收藏
    assert_eq!(
        page(AssetFilters {
            favorite: Some(false),
            ..Default::default()
        }),
        vec!["c.jpg".to_string()]
    );
    // 组合：收藏 AND 旗标关闭
    assert_eq!(
        page(AssetFilters {
            favorite: Some(true),
            flagged: Some(false),
            ..Default::default()
        }),
        vec!["b.jpg".to_string(), "a.jpg".to_string()]
    );

    // 详情载荷带 rating/flagged（camelCase）
    let detail = ipc::assets::fetch_asset_detail(&state, id("d.jpg"))
        .unwrap()
        .unwrap();
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["rating"], 0);
    assert_eq!(json["flagged"], 1);
}

// ---------------------------------------------------------------------------
// XMP 边车：应用内评分 → 边车（异步）
// ---------------------------------------------------------------------------

#[test]
fn rating_set_syncs_xmp_sidecar_async() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    // 资产指向真实文件（imported：边车写到它旁边）
    let photo = photo_root.join("DSC_0001.NEF");
    std::fs::write(&photo, b"fake-nef").unwrap();
    let row = asset(&photo.to_string_lossy(), "2026-09-01T10:00:00.000Z");
    db.insert_asset(&row).unwrap();
    let id = db
        .asset_id_by_path(&photo.to_string_lossy())
        .unwrap()
        .unwrap();

    // 评分 → 边车异步落盘（supervisor 线程；轮询等待）
    ipc::rating::fetch_asset_rating_set(&state, id, 4).unwrap();
    let sidecar = xmp::sidecar_path(&photo);
    assert!(
        wait_until(Duration::from_secs(10), || {
            sidecar.is_file()
                && xmp::sidecar_rating(&std::fs::read_to_string(&sidecar).unwrap()) == Some(4)
        }),
        "边车应异步生成并携带评分 4"
    );

    // LR 风格边车（已有其他字段）→ 改评分保留其余
    let lr = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="1" crs:Exposure2012="+0.35">
   <crs:Look><rdf:Bag><rdf:li>Matte</rdf:li></rdf:Bag></crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;
    std::fs::write(&sidecar, lr).unwrap();
    ipc::rating::fetch_asset_rating_set(&state, id, 5).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            sidecar.is_file()
                && xmp::sidecar_rating(&std::fs::read_to_string(&sidecar).unwrap()) == Some(5)
        }),
        "已有边车应被原位改写"
    );
    let text = std::fs::read_to_string(&sidecar).unwrap();
    assert!(text.contains(r#"crs:Exposure2012="+0.35""#), "LR 字段保留");
    assert!(text.contains("Matte"), "LR 结构保留");
    assert!(text.contains("<?xpacket end=\"w\"?>"));

    // 外部库（origin=external）不写边车：评分只入库
    db.0.execute("UPDATE assets SET origin = 'external' WHERE id = ?1", [id])
        .unwrap();
    std::fs::remove_file(&sidecar).unwrap();
    ipc::rating::fetch_asset_rating_set(&state, id, 2).unwrap();
    assert_eq!(db.asset_by_id(id).unwrap().unwrap().rating, 2, "评分已入库");
    std::thread::sleep(Duration::from_millis(500));
    assert!(!sidecar.is_file(), "外部库不写边车");
}

// ---------------------------------------------------------------------------
// XMP 存量评分回填（exif 通道）
// ---------------------------------------------------------------------------

#[test]
fn exif_task_backfills_rating_from_existing_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let db = open_db(&db_dir);
    // 资产 + LR 存量边车（评分 4）
    let photo = dir.path().join("DSC_0002.NEF");
    std::fs::write(&photo, b"fake-nef").unwrap();
    let sidecar = xmp::sidecar_path(&photo);
    std::fs::write(
        &sidecar,
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    db.insert_asset(&asset(&photo.to_string_lossy(), "2026-09-01T10:00:00.000Z"))
        .unwrap();
    let id = db
        .asset_id_by_path(&photo.to_string_lossy())
        .unwrap()
        .unwrap();
    // 建 exif 任务并跑完（exif 通道顺带回填评分）
    db.0.execute(
        "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('exif', ?1, 'pending', 0, '2026', '2026')",
        [id],
    )
    .unwrap();
    index::run_pending(&db_dir, 2);
    assert_eq!(
        db.asset_by_id(id).unwrap().unwrap().rating,
        4,
        "LR 存量评分回填"
    );
    // 边车未被改动（回填方向只读）
    let text = std::fs::read_to_string(&sidecar).unwrap();
    assert!(text.contains(r#"xmp:Rating="4""#));

    // DB 已有评分（>0）不被边车覆盖：应用内改 5 → 重跑 exif → 仍 5
    db.set_asset_rating(id, 5).unwrap();
    // 0007 唯一索引：复用既有任务行复位 pending
    db.0
        .execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0              WHERE kind = 'exif' AND asset_id = ?1",
            [id],
        )
        .unwrap();
    index::run_pending(&db_dir, 2);
    assert_eq!(
        db.asset_by_id(id).unwrap().unwrap().rating,
        5,
        "应用内评分优先"
    );
}

// ---------------------------------------------------------------------------
// 监视文件夹（F4 v1）
// ---------------------------------------------------------------------------

/// 独立 AppState：photo_root 与 watch 目录分离（自动入册是「从 watch 复制
/// 到库」）。
fn watch_state(
    db_dir: &std::path::Path,
    photo_root: &std::path::Path,
    config_dir: &std::path::Path,
) -> ipc::AppState {
    use std::collections::HashMap;
    use std::sync::Mutex;
    let settings = settings::Settings {
        libraries: vec![settings::Library {
            id: "lib-1".into(),
            name: "主库".into(),
            db_dir: db_dir.to_string_lossy().into_owned(),
            photo_root: photo_root.to_string_lossy().into_owned(),
            ..settings::Library::default()
        }],
        active_library_id: Some("lib-1".into()),
        watch_folders: Vec::new(),
        ..settings::Settings::default()
    };
    let supervisor = tasks::TaskSupervisor::new(events::EventBus::new());
    ipc::AppState {
        settings: Mutex::new(settings),
        config_dir: config_dir.to_path_buf(),
        bus: events::EventBus::new(),
        devices: Mutex::new(HashMap::new()),
        active_import: Mutex::new(None),
        ai: ai::ModelManager::new(
            config_dir.join("models"),
            events::EventBus::new(),
            supervisor.clone(),
        ),
        supervisor,
        thumb_queue: ipc::thumb::ThumbQueue::new(),
        migrations: Mutex::new(Default::default()),
    }
}

#[test]
fn watch_folder_add_remove_and_poll_imports_new_files() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    let config_dir = dir.path().join("config");
    let watch = dir.path().join("incoming");
    for d in [&db_dir, &photo_root, &config_dir, &watch] {
        std::fs::create_dir_all(d).unwrap();
    }
    let state = watch_state(&db_dir, &photo_root, &config_dir);
    let db = open_db(&db_dir);

    // add / remove 契约
    assert!(ipc::watch::fetch_watch_folder_add(&state, "X:/not/exist").is_err());
    ipc::watch::fetch_watch_folder_add(&state, &watch.to_string_lossy()).unwrap();
    // 幂等 + 落盘（settings.json）
    ipc::watch::fetch_watch_folder_add(&state, &watch.to_string_lossy()).unwrap();
    assert_eq!(ipc::watch::fetch_watch_folders(&state).len(), 1);
    assert!(config_dir.join("settings.json").is_file());
    let saved = settings::SettingsManager::load(&config_dir).unwrap();
    assert_eq!(saved.watch_folders.len(), 1);

    // 空目录轮询：无导入、无任务
    assert!(ipc::watch::poll_once(&state).is_empty());

    // 新文件（合法 jpg 魔数）→ 轮询自动入册
    std::fs::write(
        watch.join("IMG_9001.jpg"),
        common::shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096),
    )
    .unwrap();
    let started = ipc::watch::poll_once(&state);
    assert_eq!(started.len(), 1, "应发起一轮自动入册");
    assert_eq!(started[0].1, 1, "1 个新文件");
    let imported_id = || -> Option<i64> {
        db.0.query_row(
            "SELECT id FROM assets WHERE filename = 'IMG_9001.jpg'",
            [],
            |r| r.get(0),
        )
        .ok()
    };
    assert!(
        wait_until(Duration::from_secs(30), || imported_id().is_some()),
        "新文件应按库目录模板复制入册"
    );
    let id = imported_id().unwrap();
    let path =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap();
    assert!(
        path.starts_with(photo_root.to_string_lossy().as_ref()),
        "落在 photoRoot: {path}"
    );

    // 再轮询：无新文件 → 不再建导入任务（幂等）
    assert!(ipc::watch::poll_once(&state).is_empty());

    // remove：即时生效 + 落盘
    ipc::watch::fetch_watch_folder_remove(&state, &watch.to_string_lossy()).unwrap();
    assert!(ipc::watch::fetch_watch_folders(&state).is_empty());
    let saved = settings::SettingsManager::load(&config_dir).unwrap();
    assert!(saved.watch_folders.is_empty());
    // 幂等移除
    ipc::watch::fetch_watch_folder_remove(&state, &watch.to_string_lossy()).unwrap();
}

#[test]
fn watch_folder_event_serializes() {
    let ev = events::AppEvent::WatchFolderImported {
        folder: r"D:\incoming".into(),
        files: 3,
    };
    let json = serde_json::to_value(&ev).unwrap();
    assert_eq!(json["type"], "watchFolderImported");
    assert_eq!(json["folder"], r"D:\incoming");
    assert_eq!(json["files"], 3);
}
