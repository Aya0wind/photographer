//! 数据库命令族测试（2026-10-09 多数据库修正 + 设置独立改造）：database_
//! create/switch/remove 核心（fetch_* 直测，命令壳只做 tauri 包装）。
//!
//! 覆盖：建库一律激活新库、默认约定路径物化、db_dir 注册表内互斥、跨库
//! 照片库 root 重叠校验、切换校验（存在/可用）、两级移除（仅摘登记 /
//! 连数据目录删）与「绝不删照片库文件夹」安全闸、删激活库后的回退切换；
//! 偏好按库独立（建库/切库整套换偏好、两库值互不污染）与切库运行时
//! 重投影（AI 参数不沿用上一个库）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, thumbs,
};

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::open_db;
use events::EventBus;
use ipc::AppState;

/// 空注册表 AppState（config_dir = settings.json 落点 = 默认约定路径锚）。
fn state_at(config_dir: &Path) -> AppState {
    let supervisor = tasks::TaskSupervisor::new(EventBus::new());
    AppState {
        settings: Mutex::new(settings::Settings::default()),
        config_dir: config_dir.to_path_buf(),
        bus: EventBus::new(),
        devices: Mutex::new(HashMap::new()),
        active_import: Mutex::new(None),
        import_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        register_gate: std::sync::Arc::new(std::sync::Mutex::new(())),
        library_scan_kick: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        supervisor: supervisor.clone(),
        thumb_queue: ipc::thumb::ThumbQueue::new(),
        ai: ai::ModelManager::new(config_dir.join("models"), EventBus::new(), supervisor),
    }
}

/// 共享 AppState（fetch_settings_set 需要 &SharedState——指纹比对 spawn
/// 要求 'static 状态句柄，与生产命令壳一致）。
fn shared_state_at(config_dir: &Path) -> ipc::SharedState {
    Arc::new(state_at(config_dir))
}

/// 新建数据库并断言注册表/激活态/物化（复用样板）。
fn create_db(state: &AppState, name: &str, db_dir: Option<&str>) -> settings::DatabaseEntry {
    let initial=ipc::databases::InitialPhotoLibrary{reorganize_existing:false,existing_folder:false,name:name.to_string(),root_path:state.config_dir.join("photos").join(uuid::Uuid::new_v4().to_string()).to_string_lossy().into_owned(),organization_template:crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.into()};
    let entry = ipc::databases::fetch_database_create(state, name, db_dir, Some(&initial))
        .unwrap_or_else(|e| panic!("新建数据库「{name}」失败: {e}"));
    assert!(
        Path::new(&entry.db_dir).join("library.db").is_file(),
        "建库即物化（建目录 + 单版本 schema）"
    );
    entry
}

#[test]
fn database_create_activates_first_and_defaults_to_convention_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    // 首个库自动激活；dbDir 缺省 = <config>/databases/<id>（约定路径）
    let entry = create_db(&state, "主数据库", None);
    assert_eq!(entry.name, "主数据库");
    let expect = config.join("databases").join(&entry.id);
    assert_eq!(std::path::PathBuf::from(&entry.db_dir), expect);

    let list = ipc::databases::fetch_database_list(&state).unwrap();
    assert_eq!(list.databases.len(), 1);
    assert_eq!(list.active_id.as_deref(), Some(entry.id.as_str()));

    // 激活目录解析指向新库（一切库内操作的作用域）
    let resolved = ipc::app_database_dir(&state).unwrap();
    assert_eq!(resolved, expect);

    // 全局文件只落纯全局形态（设置独立改造：无 db 级键回流）
    let raw = std::fs::read_to_string(config.join("settings.json")).unwrap();
    assert!(raw.contains("\"databases\""));
    assert!(!raw.contains("\"ai\""), "全局文件不含库级偏好键：{raw}");

    // 无激活数据库时明确报错（新会话/删光后）
    let bare = settings::Settings::default();
    assert_eq!(
        bare.active_database_dir(&config).unwrap_err(),
        "尚未创建数据库"
    );
}

#[test]
fn database_create_second_activates_and_rejects_overlap() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    let first = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let second = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));
    // 一律激活新库（设置独立改造：引导建第二库后，后续 AI 步的
    // settings_set 要落进新库）
    let list = ipc::databases::fetch_database_list(&state).unwrap();
    assert_eq!(
        list.active_id.as_deref(),
        Some(second.id.as_str()),
        "建库即激活（不再保持旧激活）"
    );

    // 注册表内互斥：相同 / 互相包含（含嵌套子目录）一律拒绝
    for bad in [
        first.db_dir.clone(),
        std::path::PathBuf::from(&first.db_dir)
            .join("nested")
            .to_string_lossy()
            .into_owned(),
    ] {
        let err = ipc::databases::fetch_database_create(&state, "坏库", Some(&bad), None).unwrap_err();
        assert!(err.contains("相同或互相包含"), "{bad} 应被互斥拒绝：{err}");
    }
    // 相对路径 / 空名拒绝
    assert!(ipc::databases::fetch_database_create(&state, "相对路径库", Some("relative/db"), None).is_err());
    assert!(ipc::databases::fetch_database_create(&state, "  ", None, None).is_err());
    assert_eq!(
        ipc::databases::fetch_database_list(&state).unwrap().databases.len(),
        2,
        "失败请求不落注册表"
    );
    assert!(Path::new(&second.db_dir).join("library.db").is_file());
}

#[test]
fn database_switch_changes_active_scope() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    let first = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let second = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));

    // 不存在的 id 拒绝切换
    assert!(ipc::databases::fetch_database_switch(&state, "no-such-db").is_err());

    // 建库即激活（= 库 B），真正的切换是 B → A
    ipc::databases::fetch_database_switch(&state, &first.id).unwrap();
    assert_eq!(
        ipc::databases::fetch_database_list(&state).unwrap().active_id,
        Some(first.id.clone()),
        "切换后激活 = 库 A"
    );
    assert_eq!(
        ipc::app_database_dir(&state).unwrap(),
        std::path::PathBuf::from(&first.db_dir),
        "激活目录解析跟随切换"
    );

    // 切回 B + 幂等：切到当前库 no-op 成功
    ipc::databases::fetch_database_switch(&state, &second.id).unwrap();
    ipc::databases::fetch_database_switch(&state, &second.id).unwrap();
    assert_eq!(
        ipc::databases::fetch_database_list(&state).unwrap().active_id,
        Some(second.id)
    );
}

#[test]
fn database_remove_two_tiers_and_active_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    let first = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let second = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));

    // 两级之一：仅摘登记——数据目录原样留在磁盘
    ipc::databases::fetch_database_remove(&state, &second.id, false).unwrap();
    assert!(Path::new(&second.db_dir).join("library.db").is_file(), "摘登记不删数据");
    assert_eq!(ipc::databases::fetch_database_list(&state).unwrap().databases.len(), 1);

    // 两级之二：连数据目录删；删的是激活库 → 回退剩余第一个（此处无剩余 → None）
    let result = ipc::databases::fetch_database_remove(&state, &first.id, true).unwrap();
    assert_eq!(result.data_dirs_deleted, 1);
    assert!(!Path::new(&first.db_dir).exists(), "数据目录已整棵删除");
    let list = ipc::databases::fetch_database_list(&state).unwrap();
    assert!(list.databases.is_empty());
    assert_eq!(list.active_id, None, "删激活库且无剩余 → 回到未建库态");
    assert_eq!(
        ipc::app_database_dir(&state).unwrap_err(),
        "尚未创建数据库",
        "库内操作在无数据库时明确报错"
    );

    // 不存在的 id 拒绝
    assert!(ipc::databases::fetch_database_remove(&state, &first.id, false).is_err());
}

#[test]
fn database_remove_and_create_guard_photo_library_roots() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    let first = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let second = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));

    // 场景一（create 闸门）：在库 B 里登记一个**远离一切 dbDir** 的照片库
    // root；新 dbDir 落它之内 → 注册表内互斥放行、跨库照片库重叠拒绝。
    ipc::databases::fetch_database_switch(&state, &second.id).unwrap();
    let photo_root = dir.path().join("照片");
    std::fs::create_dir_all(&photo_root).unwrap();
    let db = ipc::app_database_db(&state).unwrap();
    db.photos_library_register(
        "库 B 的照片库",
        &photo_root.to_string_lossy(),
        &std::path::PathBuf::from(&second.db_dir),
    )
    .unwrap();

    let inside_root = photo_root.join("nested");
    let err =
        ipc::databases::fetch_database_create(&state, "坏库", Some(inside_root.to_str().unwrap()), None)
            .unwrap_err();
    assert!(err.contains("照片库"), "新建闸门应点名列出照片库：{err}");

    // 场景二（remove 闸门）：再登记一个**落库 A 数据目录之内**的照片库
    // root（库 A 建库时它尚不存在——跨库重叠只能开库查，正是被测闸门的
    // 目标场景）；deleteData=true 删库 A 与照片库文件夹重叠 → 拒绝。
    let inner_root = std::path::PathBuf::from(&first.db_dir).join("库内照片");
    std::fs::create_dir_all(&inner_root).unwrap();
    db.photos_library_register(
        "落库 A 目录的照片库",
        &inner_root.to_string_lossy(),
        &std::path::PathBuf::from(&second.db_dir),
    )
    .unwrap();
    let err = ipc::databases::fetch_database_remove(&state, &first.id, true).unwrap_err();
    assert!(err.contains("拒绝删除"), "重叠保护应拒绝删除：{err}");
    // 仅摘登记不受影响（不动任何文件），照片库文件夹原样保留
    ipc::databases::fetch_database_remove(&state, &first.id, false).unwrap();
    assert!(photo_root.is_dir() && inner_root.is_dir(), "照片库文件夹原样保留");
}

#[test]
fn database_switch_rejects_unavailable_database() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);

    let first = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    // 手动删掉数据目录后注册表条目仍在：切换要求文件在盘，拒绝保持现库
    std::fs::remove_dir_all(&first.db_dir).unwrap();
    let err = ipc::databases::fetch_database_switch(&state, &first.id).unwrap_err();
    assert!(err.contains("未切换"), "{err}");
}

// ---------------------------------------------------------------------------
// 偏好按库独立 + 切库运行时重投影（2026-10-09 设置独立改造）
// ---------------------------------------------------------------------------

/// 偏好独立性：库 A 改偏好 → 建库 B（一律激活）整套换默认 → B 改自己的
/// → 切回 A 原样恢复；两库落各自 db_dir 的库级文件，值互不污染。
#[test]
fn settings_preferences_are_independent_per_database() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = shared_state_at(&config);

    let a = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    // 库 A 改 AI 开关 + 监视目录（settings_set 核：七组偏好落激活库库级文件）
    let mut edited = ipc::settings::fetch_settings_get(&state);
    edited.ai.enable_clip = true;
    edited.watch_folders = vec![r"I:\incoming".into()];
    ipc::settings::fetch_settings_set(&state, edited).unwrap();
    let snapshot = ipc::settings::fetch_settings_get(&state);
    assert!(snapshot.ai.enable_clip);
    assert_eq!(snapshot.watch_folders.len(), 1);

    // 落点：A 的 db_dir 有库级 settings.json，值落位
    let a_settings = Path::new(&a.db_dir).join("settings.json");
    assert!(a_settings.is_file(), "库级文件落各自 db_dir");
    let raw = std::fs::read_to_string(&a_settings).unwrap();
    assert!(raw.contains("\"enableClip\": true"), "A 的 AI 开关落盘：{raw}");

    // 建库 B（一律激活）→ 偏好整套换默认（新库无库级文件 = 出厂态）
    let b = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));
    let snapshot = ipc::settings::fetch_settings_get(&state);
    assert!(!snapshot.ai.enable_clip, "新库 AI 开关 = 默认");
    assert!(snapshot.watch_folders.is_empty(), "新库监视目录 = 默认");

    // 库 B 改自己的偏好（gallery）→ 不污染 A
    let mut edited = ipc::settings::fetch_settings_get(&state);
    edited.gallery.merge_raw_jpg = false;
    ipc::settings::fetch_settings_set(&state, edited).unwrap();

    // 切回 A → A 的偏好原样恢复（B 的 gallery 改动不在）
    ipc::databases::fetch_database_switch(&state, &a.id).unwrap();
    let snapshot = ipc::settings::fetch_settings_get(&state);
    assert!(snapshot.ai.enable_clip, "A 的 AI 开关原样恢复");
    assert_eq!(snapshot.watch_folders.len(), 1);
    assert!(snapshot.gallery.merge_raw_jpg, "B 的 gallery 改动不污染 A");

    // 切到 B → B 自己的值
    ipc::databases::fetch_database_switch(&state, &b.id).unwrap();
    let snapshot = ipc::settings::fetch_settings_get(&state);
    assert!(!snapshot.ai.enable_clip);
    assert!(!snapshot.gallery.merge_raw_jpg);
}

/// 切库运行时重投影：库 A 关 GPU/改 embed 档位 → 建库 B（默认参数）→
/// state.ai 投影换回默认，绝不沿用上一个库的参数；切回 A 再恢复。
#[test]
fn switching_database_reprojects_runtime_ai_params() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = shared_state_at(&config);

    let a = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let mut edited = ipc::settings::fetch_settings_get(&state);
    edited.ai.use_gpu = false;
    edited.ai.embed_input_size = 384;
    ipc::settings::fetch_settings_set(&state, edited).unwrap();
    let params = state.ai.ai_params();
    assert!(!params.use_gpu);
    assert_eq!(params.embed_input_size, 384);

    // 建库 B（默认 use_gpu=true / 256）→ 投影按新库重算
    create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));
    let params = state.ai.ai_params();
    assert!(params.use_gpu, "建库后投影换回新库默认（不沿用 A）");
    assert_eq!(params.embed_input_size, 256);

    // 切回 A → A 的参数恢复
    ipc::databases::fetch_database_switch(&state, &a.id).unwrap();
    let params = state.ai.ai_params();
    assert!(!params.use_gpu, "切回 A 恢复 A 的投影");
    assert_eq!(params.embed_input_size, 384);
}

/// 无激活库时 settings_set 明确报错（七组偏好无处安放）。
#[test]
fn settings_set_without_active_database_errors() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = shared_state_at(&config);

    let err = ipc::settings::fetch_settings_set(&state, settings::Settings::default()).unwrap_err();
    assert_eq!(err, "尚未创建数据库");
}

// ---------------------------------------------------------------------------
// photo_library_remove 完整级联清理 + 可选删原文件（2026-10-09 用户定案）
// ---------------------------------------------------------------------------

/// 删库场景的库内资产直插（绕过导入管线；返回自增 id）。
fn insert_library_asset(db: &db::Db, library_id: &str, path: &Path, xxhash: i64) -> i64 {
    db.0.execute(
        "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
         created_at, origin, library_id) \
         VALUES (?1, ?2, 1, '2026-01-01T00:00:00Z', ?3, 'photo', 'imported', \
         '2026-01-01T00:00:00Z', 'imported', ?4)",
        rusqlite::params![
            path.to_string_lossy().to_string(),
            path.file_name().unwrap().to_string_lossy().to_string(),
            xxhash,
            library_id
        ],
    )
    .unwrap();
    db.0
        .query_row(
            "SELECT id FROM assets WHERE path = ?1",
            [path.to_string_lossy().to_string()],
            |r| r.get(0),
        )
        .unwrap()
}

/// 删库连记录删的完整级联：FK 级联（album_item/faces）+ 补齐盲区——孤儿
/// 人物簇 / 缩略图缓存（全档位全 mtime 代）/ 语义向量 / 空连拍壳 /
/// library_scan_dirs 剪枝缓存 / pair 双向引用；共享人物与邻库的缓存/
/// 向量/账本不受牵连，物理文件不动（delete_files=false）。
#[test]
fn photo_library_remove_with_records_cleans_children_and_derivations() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);
    let entry = create_db(&state, "主数据库", Some(dir.path().join("main-db").to_str().unwrap()));
    let db_dir = Path::new(&entry.db_dir).to_path_buf();
    let db = ipc::app_database_db(&state).unwrap();

    // 两库：A 待删（管理库），B 保留（验证共享数据不被牵连）
    let root_a = dir.path().join("photos-a");
    let root_b = dir.path().join("photos-b");
    std::fs::create_dir_all(root_a.join("2026/06")).unwrap();
    std::fs::create_dir_all(root_b.join("2026/06")).unwrap();
    let lib_a = db
        .photos_library_register("库A", &root_a.to_string_lossy(), &db_dir)
        .unwrap();
    let lib_b = db
        .photos_library_register("库B", &root_b.to_string_lossy(), &db_dir)
        .unwrap();
    let file_a1 = root_a.join("2026/06/DSC_1.jpg");
    let file_a2 = root_a.join("2026/06/DSC_2.jpg");
    let file_b1 = root_b.join("2026/06/DSC_9.jpg");
    for file in [&file_a1, &file_a2, &file_b1] {
        std::fs::write(file, b"jpeg").unwrap();
    }
    let a1 = insert_library_asset(&db, &lib_a.id, &file_a1, 1);
    let a2 = insert_library_asset(&db, &lib_a.id, &file_a2, 2);
    let b1 = insert_library_asset(&db, &lib_b.id, &file_b1, 3);

    // 相册引用（相册是全局概念：行保留、成员级联清）
    db.0.execute(
        "INSERT INTO album (name, created_at) VALUES ('相册', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    let album_id = db.0.last_insert_rowid();
    db.0.execute(
        "INSERT INTO album_item (album_id, asset_id, added_at) \
         VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
        rusqlite::params![album_id, a1],
    )
    .unwrap();

    // 人物簇：孤儿簇只挂库A 的脸；共享簇库A/B 各有脸
    let orphan_person = db.create_person().unwrap();
    let shared_person = db.create_person().unwrap();
    db.insert_face(a1, 0.0, 0.0, 0.5, 0.5, &[0.1, 0.2], Some(orphan_person))
        .unwrap();
    db.insert_face(a2, 0.0, 0.0, 0.5, 0.5, &[0.3, 0.4], Some(orphan_person))
        .unwrap();
    db.insert_face(a1, 0.0, 0.0, 0.3, 0.3, &[0.5, 0.6], Some(shared_person))
        .unwrap();
    db.insert_face(b1, 0.0, 0.0, 0.5, 0.5, &[0.7, 0.8], Some(shared_person))
        .unwrap();

    // 连拍（库A 独占）+ 跨库 pair（B 引 A）+ 两库各自目录剪枝缓存
    db.0.execute(
        "INSERT INTO bursts (asset_count, started_at, ended_at) VALUES (2, NULL, NULL)",
        [],
    )
    .unwrap();
    let burst_id = db.0.last_insert_rowid();
    db.0.execute(
        "UPDATE assets SET burst_id = ?2 WHERE id = ?1",
        rusqlite::params![a1, burst_id],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET burst_id = ?2 WHERE id = ?1",
        rusqlite::params![a2, burst_id],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
        rusqlite::params![b1, a1],
    )
    .unwrap();
    for lib in [&lib_a, &lib_b] {
        db.0.execute(
            "INSERT INTO library_scan_dirs (library_id, dir_path, mtime) \
             VALUES (?1, '2026', '2026-01-01T00:00:00Z')",
            [&lib.id],
        )
        .unwrap();
    }

    // 缩略图缓存夹具：a1 当前代 + 陈旧 mtime 代（256 档）+ 512 档；b1 一份
    let mtime_a1 = std::fs::metadata(&file_a1).unwrap().modified().unwrap();
    let cache_cur = thumbs::cache_path(&db_dir, &file_a1, 256, mtime_a1);
    let cache_stale = thumbs::cache_path(
        &db_dir,
        &file_a1,
        256,
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1),
    );
    let cache_512 = thumbs::cache_path(&db_dir, &file_a1, 512, mtime_a1);
    let cache_b1 = thumbs::cache_path(
        &db_dir,
        &file_b1,
        256,
        std::fs::metadata(&file_b1).unwrap().modified().unwrap(),
    );
    for cache in [&cache_cur, &cache_stale, &cache_512, &cache_b1] {
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        std::fs::write(cache, b"thumb").unwrap();
    }

    // 语义向量夹具（usearch 真值文件；池内与落盘同把柄）
    let dim = ai::embed::EMBED_DIM;
    let index = ai::semantic::shared_index(&db_dir).unwrap();
    {
        let idx = index.lock().unwrap();
        idx.reserve(64).unwrap();
        idx.add(a1 as u64, &vec![0.5f32; dim]).unwrap();
        idx.add(b1 as u64, &vec![0.4f32; dim]).unwrap();
        idx.save(db_dir.join("vectors.usearch").to_str().unwrap())
            .unwrap();
    }

    // 执行：连记录删（不动文件）
    let result =
        ipc::photo_library::fetch_photo_library_remove(&state, &lib_a.id, false).unwrap();
    assert_eq!(result.records_deleted, 2);
    assert_eq!(result.files_deleted, 0, "delete_files=false 不动物理文件");

    // FK 级联 + 补齐清理逐项断言（同一连接读已提交数据）
    let album_items: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM album_item WHERE asset_id IN (?1, ?2)",
            rusqlite::params![a1, a2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(album_items, 0, "相册成员级联清（相册行保留）");
    let faces_a: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM faces WHERE asset_id IN (?1, ?2)",
            rusqlite::params![a1, a2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(faces_a, 0, "人脸实例级联清");
    let orphan_left: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM people WHERE id = ?1", [orphan_person], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(orphan_left, 0, "变孤儿的人物簇删除");
    let shared_left: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM people WHERE id = ?1", [shared_person], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(shared_left, 1, "别的库仍在用的人物保留");
    let bursts_left: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM bursts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(bursts_left, 0, "空连拍壳清理");
    let pair_of_b1: Option<i64> = db
        .0
        .query_row("SELECT pair_asset_id FROM assets WHERE id = ?1", [b1], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(pair_of_b1.is_none(), "指向被删资产的 pair 引用清空");
    let scan_dirs_a: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM library_scan_dirs WHERE library_id = ?1",
            [&lib_a.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(scan_dirs_a, 0, "库扫描剪枝缓存随库清（表无 FK）");
    let scan_dirs_b: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM library_scan_dirs WHERE library_id = ?1",
            [&lib_b.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(scan_dirs_b, 1, "保留库的剪枝缓存不动");

    // 缩略图缓存：a1 全档位全代清；b1 保留
    assert!(
        !cache_cur.exists() && !cache_stale.exists() && !cache_512.exists(),
        "库A 资产缩略图缓存（含陈旧代/跨档位）清理"
    );
    assert!(cache_b1.is_file(), "库B 资产缓存不受牵连");

    // 向量：a1 删、b1 留（逐出池后从落盘文件重载，断言清理已持久化）
    ai::semantic::invalidate_index(&db_dir);
    let index = ai::semantic::shared_index(&db_dir).unwrap();
    {
        let idx = index.lock().unwrap();
        assert!(!idx.contains(a1 as u64), "库A 资产向量删除");
        assert!(idx.contains(b1 as u64), "库B 资产向量保留");
    }

    // 物理文件不动 + 库行消失 + 库内资产行清空
    assert!(file_a1.is_file() && file_a2.is_file(), "不删文件档物理照片保留");
    assert!(db.photos_library_get(&lib_a.id).unwrap().is_none());
    let assets_left: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE library_id = ?1",
            [&lib_a.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(assets_left, 0, "库内资产记录清空");
}

/// 管理库 delete_files=true：磁盘文件删除 + 缺失容忍（外部已删不计）+
/// 删除失败容忍（路径是目录 → warn 日志不中断）。
#[test]
fn photo_library_remove_managed_library_deletes_files_with_tolerance() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);
    let entry = create_db(&state, "主数据库", Some(dir.path().join("main-db").to_str().unwrap()));
    let db_dir = Path::new(&entry.db_dir).to_path_buf();
    let db = ipc::app_database_db(&state).unwrap();

    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("2026/06")).unwrap();
    let lib = db
        .photos_library_register("管理库", &root.to_string_lossy(), &db_dir)
        .unwrap();
    let file_ok = root.join("2026/06/OK.jpg");
    let file_gone = root.join("2026/06/GONE.jpg");
    let path_dir = root.join("2026/06/DIR.jpg");
    std::fs::write(&file_ok, b"jpeg").unwrap();
    std::fs::write(&file_gone, b"jpeg").unwrap();
    std::fs::create_dir_all(&path_dir).unwrap(); // 目录占位：remove_file 必失败
    let _id_ok = insert_library_asset(&db, &lib.id, &file_ok, 1);
    let _id_gone = insert_library_asset(&db, &lib.id, &file_gone, 2);
    let _id_dir = insert_library_asset(&db, &lib.id, &path_dir, 3);
    std::fs::remove_file(&file_gone).unwrap(); // 登记后外部删除 → 缺失容忍

    let result = ipc::photo_library::fetch_photo_library_remove(&state, &lib.id, true).unwrap();
    assert_eq!(result.records_deleted, 3, "三条记录照常清");
    assert_eq!(result.files_deleted, 1, "只计实际删掉的文件（缺失/失败不计）");
    assert!(!file_ok.exists(), "管理库原片被删");
    assert!(path_dir.is_dir(), "删除失败的条目不中断流程");
}

/// 引用库（从已有文件夹建立）拒绝 delete_files=true：错误明确、库行原样；
/// delete_files=false 一切正常；reference 标志经 photo_library_list 透传
/// （前端据此隐藏删文件选项）。

#[test]
fn photo_library_remove_default_clears_records_and_cache_but_keeps_file() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);
    let entry = create_db(&state, "主数据库", Some(dir.path().join("main-db").to_str().unwrap()));
    let db_dir = Path::new(&entry.db_dir).to_path_buf();
    let db = ipc::app_database_db(&state).unwrap();

    let root = dir.path().join("photos");
    std::fs::create_dir_all(root.join("2026/06")).unwrap();
    let lib = db
        .photos_library_register("库", &root.to_string_lossy(), &db_dir)
        .unwrap();
    let file = root.join("2026/06/DSC_1.jpg");
    std::fs::write(&file, b"jpeg").unwrap();
    insert_library_asset(&db, &lib.id, &file, 1);
    let cache = thumbs::cache_path(
        &db_dir,
        &file,
        256,
        std::fs::metadata(&file).unwrap().modified().unwrap(),
    );
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, b"thumb").unwrap();

    let result =
        ipc::photo_library::fetch_photo_library_remove(&state, &lib.id, false).unwrap();
    assert_eq!(result.records_deleted, 1);
    assert_eq!(result.files_deleted, 0);
    assert!(file.is_file(), "物理文件不动");
    assert!(!cache.exists(), "记录删除则缩略图缓存随清");
    let left: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE library_id = ?1",
            [&lib.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "资产记录已清");
    assert!(db.photos_library_get(&lib.id).unwrap().is_none(), "登记行消失");
}

// ---------------------------------------------------------------------------
// 切库索引恢复（2026-10-09 审计 P0 断链B）：切回旧库时遗留索引任务被
// resume_and_kick 唤醒（running 复位 pending → worker 续跑）+ 参数指纹
// 比对落基线（marker）。
// ---------------------------------------------------------------------------

#[test]
fn database_switch_resumes_pending_index_tasks_of_new_database() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("appdata");
    std::fs::create_dir_all(&config).unwrap();
    let state = state_at(&config);
    let db_a = create_db(&state, "库 A", Some(dir.path().join("a-db").to_str().unwrap()));
    let db_b = create_db(&state, "库 B", Some(dir.path().join("b-db").to_str().unwrap()));
    assert_eq!(
        ipc::databases::fetch_database_list(&state).unwrap().active_id.as_deref(),
        Some(db_b.id.as_str()),
        "建库即激活（当前 = B）"
    );

    // 在库 A 造一条「崩溃遗留」running 任务（资产 kind=video：thumb 通道
    // 永久占位收尾，不依赖图像解码，确定性收敛）
    let a_dir = Path::new(&db_a.db_dir).to_path_buf();
    let a_db = open_db(&a_dir);
    a_db.0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
             created_at, origin, missing) \
             VALUES ('C:/gone/clip.mp4', 'clip.mp4', 1, '2026-01-01T00:00:00Z', 0, 'video', \
             'imported', '2026-01-01T00:00:00Z', 'imported', 0)",
            [],
        )
        .unwrap();
    a_db.0
        .execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             SELECT 'thumb', id, 'running', 0, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z' \
             FROM assets WHERE path = 'C:/gone/clip.mp4'",
            [],
        )
        .unwrap();
    drop(a_db);

    // 切回库 A：resume_and_kick（running 复位 pending → 派 worker 续跑）
    ipc::databases::fetch_database_switch(&state, &db_a.id).unwrap();

    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let check = open_db(&a_dir);
        let state_name: String = check
            .0
            .query_row(
                "SELECT state FROM index_tasks WHERE kind = 'thumb'",
                [],
                |r| r.get(0),
            )
            .unwrap_or_default();
        let thumb_state: i64 = check
            .0
            .query_row(
                "SELECT thumb_state FROM assets WHERE path = 'C:/gone/clip.mp4'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(-1);
        if state_name == "done" && thumb_state == 2 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "切库后遗留任务未被唤醒（state={state_name} thumb_state={thumb_state}）"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // 指纹基线随切库落盘（check_params_and_rebuild 执行过的痕迹）
    assert!(
        a_dir.join("index-params.marker").is_file(),
        "切库后参数指纹基线应写入新库目录"
    );

    // 等 supervisor 派发的 index-resume 线程收尾（tempdir 生命周期保护）
    let started = std::time::Instant::now();
    while state.supervisor.running_count() > 0 {
        assert!(started.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
}
