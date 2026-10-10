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
    let entry = ipc::databases::fetch_database_create(state, name, db_dir)
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
        let err = ipc::databases::fetch_database_create(&state, "坏库", Some(&bad)).unwrap_err();
        assert!(err.contains("相同或互相包含"), "{bad} 应被互斥拒绝：{err}");
    }
    // 相对路径 / 空名拒绝
    assert!(ipc::databases::fetch_database_create(&state, "相对路径库", Some("relative/db")).is_err());
    assert!(ipc::databases::fetch_database_create(&state, "  ", None).is_err());
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
        ipc::databases::fetch_database_create(&state, "坏库", Some(inside_root.to_str().unwrap()))
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
