//! SettingsManager / Settings 行为测试（2026-10-09 设置独立改造语义）：
//! 默认值、缺文件、roundtrip（全局 + 库级两层）、损坏 JSON 恢复、原子写、
//! split/compose 往返、库级缺文件默认、一次性迁移（旧全量全局 → 激活库
//! 落位 + 全局瘦身；库级已存在不覆盖；无激活库丢弃）、旧键忽略（单库
//! databaseDir 残留键 serde 丢弃）、camelCase 序列化、数据库注册表解析、
//! db_dir 互斥闸门、「数据库位置」归一化闸门。

use std::fs;
use std::path::{Path, PathBuf};

use photographer_lib::settings::{
    normalize_database_path, normalize_library_path, validate_ai_settings,
    validate_database_dir_overlap, AiSettings, DatabaseEntry, DatabaseSettings,
    DuplicatePolicy, GlobalSettings, ImportSettings, IndexSchedule, Settings, SettingsError,
    SettingsManager, SystemSettings, SCHEMA_VERSION,
};

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

fn corrupt_backups(dir: &Path) -> Vec<String> {
    let mut names = Vec::new();
    for entry in fs::read_dir(dir).expect("failed to read dir") {
        let name = entry
            .expect("failed to read dir entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if name.starts_with("settings.json.corrupt-") {
            names.push(name);
        }
    }
    names
}

/// 把合成 Settings 拆两层落盘（save_global + save_database 的样板）：
/// 全局键 → config_dir，七组偏好 → 激活库 db_dir。
fn save_split(settings: &Settings, config_dir: &Path) {
    let (global, database) = settings.split();
    SettingsManager::save_global(&global, config_dir).expect("save global");
    let db_dir = settings
        .active_database_dir(config_dir)
        .expect("需要激活库才能落库级文件");
    SettingsManager::save_database(&database, &db_dir).expect("save database");
}

/// 临时激活库（db-1 → 真实临时目录）注册表条目。
fn registry_with_active_db(dir: &Path) -> Vec<DatabaseEntry> {
    vec![DatabaseEntry {
        id: "db-1".into(),
        name: "主数据库".into(),
        db_dir: dir.join("db-1").to_string_lossy().into_owned(),
    }]
}

#[test]
fn default_settings_match_spec() {
    let s = Settings::default();

    assert_eq!(s.schema_version, 1);
    assert_eq!(s.schema_version, SCHEMA_VERSION);
    assert!(!s.onboarding_completed);
    // 数据库注册表（多数据库修正）：默认空表 + 无激活数据库
    assert!(s.databases.is_empty());
    assert_eq!(s.active_database_id, None);

    assert!(s.import.prompt_on_device);
    assert!(s.import.skip_imported);
    assert_eq!(s.import.duplicate_policy, DuplicatePolicy::Skip);

    assert!(!s.ai.enable_clip);
    assert!(!s.ai.enable_face);
    assert!(!s.ai.enable_scene_tags);
    assert_eq!(s.ai.index_schedule, IndexSchedule::IdleOnly);
    assert_eq!(s.ai.cpu_limit_percent, 50);
    assert!(s.ai.use_gpu);

    assert!(!s.system.launch_at_login);
    assert!(s.system.close_to_tray);
    assert_eq!(s.system.language, "zh");
}

/// 无任何文件（首次启动）→ 全默认合成（不报错，引导流程照常）。
#[test]
fn load_missing_file_returns_default_composition() {
    let dir = temp_dir();
    let loaded = SettingsManager::load_composed(dir.path()).expect("load should succeed");
    assert_eq!(loaded, Settings::default());
}

#[test]
fn save_then_load_roundtrip_with_custom_values() {
    let dir = temp_dir();
    let s = Settings {
        onboarding_completed: true,
        databases: registry_with_active_db(dir.path()),
        active_database_id: Some("db-1".into()),
        import: ImportSettings {
            duplicate_policy: DuplicatePolicy::Rename,
            skip_imported: false,
            ..ImportSettings::default()
        },
        ai: AiSettings {
            enable_clip: true,
            enable_face: true,
            index_schedule: IndexSchedule::AfterImport,
            cpu_limit_percent: 25,
            use_gpu: false,
            ..AiSettings::default()
        },
        system: SystemSettings {
            launch_at_login: true,
            close_to_tray: false,
            language: "en".to_string(),
            auto_open_database_id: Some("preferred-db".into()),
        },
        ..Settings::default()
    };

    save_split(&s, dir.path());
    let loaded = SettingsManager::load_composed(dir.path()).expect("load should succeed");
    assert_eq!(loaded, s);
}

/// split/compose 往返（纯内存）：合成 → 拆两层 → 重组无损；schema_version
/// 取全局侧（库级无版本键）。
#[test]
fn split_compose_roundtrip() {
    let s = Settings {
        onboarding_completed: true,
        databases: registry_with_active_db(Path::new(r"I:\SmartPhoto")),
        active_database_id: Some("db-1".into()),
        ai: AiSettings {
            enable_clip: true,
            ..AiSettings::default()
        },
        ..Settings::default()
    };
    let (global, database) = s.split();
    assert_eq!(global, GlobalSettings {
        schema_version: SCHEMA_VERSION,
        onboarding_completed: true,
        databases: s.databases.clone(),
        active_database_id: Some("db-1".into()),
    });
    assert_eq!(database, DatabaseSettings {
        import: s.import.clone(),
        gallery: s.gallery.clone(),
        appearance: s.appearance.clone(),
        ai: s.ai.clone(),
        system: s.system.clone(),
        storage: s.storage.clone(),
        watch_folders: Vec::new(),
    });
    assert_eq!(Settings::compose(&global, &database), s);
    // 默认合成：全局默认 + 库级默认 = Settings 默认
    assert_eq!(
        Settings::compose(&GlobalSettings::default(), &DatabaseSettings::default()),
        Settings::default()
    );
}

/// 库级文件缺省（新库/从未改过偏好）→ 全默认（每库一份的「出厂态」）。
#[test]
fn load_database_missing_file_returns_defaults() {
    let dir = temp_dir();
    let loaded = SettingsManager::load_database(&dir.path().join("db-1")).unwrap();
    assert_eq!(loaded, DatabaseSettings::default());
    assert!(!dir.path().join("db-1").join("settings.json").exists());
}

/// 库级 roundtrip：save_database → load_database 无损（含 watchFolders）。
#[test]
fn save_database_then_load_roundtrip() {
    let dir = temp_dir();
    let db_dir = dir.path().join("db-1");
    let database = DatabaseSettings {
        watch_folders: vec![r"I:\incoming".into()],
        ai: AiSettings {
            enable_clip: true,
            ..AiSettings::default()
        },
        ..DatabaseSettings::default()
    };
    SettingsManager::save_database(&database, &db_dir).unwrap();
    assert_eq!(
        SettingsManager::load_database(&db_dir).unwrap(),
        database,
        "save_database 自动建目录（新库未物化时迁移路径先落偏好）"
    );
}

/// 库级文件损坏 → .corrupt 改名隔离后回默认（单库偏好脏了不拖累启动）。
#[test]
fn corrupted_database_file_is_quarantined_and_defaults_returned() {
    let dir = temp_dir();
    let db_dir = dir.path().join("db-1");
    fs::create_dir_all(&db_dir).unwrap();
    fs::write(settings_path(&db_dir), "{{{ 这不是 JSON").unwrap();

    let loaded = SettingsManager::load_database(&db_dir).expect("recover");
    assert_eq!(loaded, DatabaseSettings::default());
    assert_eq!(corrupt_backups(&db_dir).len(), 1, "db 级 .corrupt 备件一份");
}

/// 全局文件损坏 → load_composed 兜全默认合成 + .corrupt 隔离。
#[test]
fn corrupted_json_is_backed_up_and_defaults_returned() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), "{{{ 这不是 JSON").expect("write corrupt file");

    let loaded = SettingsManager::load_composed(dir.path()).expect("load should recover");
    assert_eq!(loaded, Settings::default());
    assert!(
        !settings_path(dir.path()).exists(),
        "corrupt settings.json should be renamed away"
    );
    let backups = corrupt_backups(dir.path());
    assert_eq!(
        backups.len(),
        1,
        "exactly one .corrupt backup expected, got {backups:?}"
    );
    assert!(backups[0].starts_with("settings.json.corrupt-"));
}

#[test]
fn save_is_atomic_no_tmp_leftover() {
    let dir = temp_dir();
    let mut s = Settings::default();
    s.databases = registry_with_active_db(dir.path());
    s.active_database_id = Some("db-1".into());
    save_split(&s, dir.path());
    for target in [dir.path(), &dir.path().join("db-1")] {
        assert!(settings_path(target).exists());
        assert!(
            !target.join("settings.json.tmp").exists(),
            "no .tmp file should remain after save"
        );
    }

    // 覆盖保存同样不留残留
    s.onboarding_completed = true;
    save_split(&s, dir.path());
    assert!(!dir.path().join("settings.json.tmp").exists());
    let reloaded = SettingsManager::load_composed(dir.path()).expect("reload");
    assert!(reloaded.onboarding_completed);
}

#[test]
fn old_json_with_missing_fields_is_filled_with_defaults() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":true,"libraryRoot":"D:/Photos"}"#,
    )
    .expect("write partial settings");

    let s = SettingsManager::load_composed(dir.path()).expect("load");
    assert_eq!(s.schema_version, SCHEMA_VERSION);
    // 旧字段 libraryRoot 被忽略（serde 默认不拒绝未知字段）
    assert!(s.onboarding_completed);
    assert_eq!(s.import, ImportSettings::default());
    assert_eq!(s.ai.index_schedule, IndexSchedule::IdleOnly);
    assert_eq!(s.system.language, "zh");
}

/// 旧库注册表残留键（libraries/activeLibraryId/dbDir 等）整体忽略（§二
/// 退役，不做迁移）：加载不报错、不复活任何库概念字段；本例的旧全量文件
/// 含 db 级键（import）→ 触发拆层迁移，无激活库丢弃后全局文件瘦身。
#[test]
fn retired_library_registry_keys_are_ignored_on_load() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"activeLibraryId":"lib-1","libraries":[
            {"id":"lib-1","name":"主库","dbDir":"I:\\SmartPhoto\\主库","photoRoot":"Y:\\照片",
             "dirTemplate":"{YYYY}/{MM-DD}","importSubdir":"卡导入区","streams":6}
        ],"import":{"dirTemplate":"{YYYY}","importSubdir":"旧子目录"}}"#,
    )
    .expect("write old-style settings");

    let s = SettingsManager::load_composed(dir.path()).expect("load");
    assert_eq!(s.schema_version, SCHEMA_VERSION, "不升 schema_version");
    assert_eq!(
        serde_json::to_value(&s).unwrap()["libraries"],
        serde_json::json!(null),
        "库注册表字段已不存在于序列化产物"
    );
    assert_eq!(s.import, ImportSettings::default(), "无激活库：db 级值丢弃");

    // 迁移后全局文件瘦身为纯全局形态（旧键 + db 级键一并消失）
    let raw = fs::read_to_string(settings_path(dir.path())).unwrap();
    assert!(!raw.contains("libraries"));
    assert!(!raw.contains("activeLibraryId"));
    assert!(!raw.contains("photoRoot"));
    assert!(!raw.contains("import"));
}

#[test]
fn empty_json_object_yields_defaults() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), "{}").expect("write empty object");
    let s = SettingsManager::load_composed(dir.path()).expect("load");
    assert_eq!(s, Settings::default());
}

#[test]
fn newer_schema_version_is_migration_error() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), r#"{"schemaVersion":99}"#).expect("write future settings");
    let result = SettingsManager::load_composed(dir.path());
    assert!(matches!(result, Err(SettingsError::Migration(99))));
}

#[test]
fn serialization_uses_camel_case() {
    let value = serde_json::to_value(Settings::default()).expect("serialize");
    assert_eq!(value["schemaVersion"], serde_json::json!(1));
    assert_eq!(value["onboardingCompleted"], serde_json::json!(false));
    // 数据库注册表（多数据库修正）：默认空表 + null 激活 id
    assert_eq!(value["databases"], serde_json::json!([]));
    assert_eq!(value["activeDatabaseId"], serde_json::json!(null));
    assert_eq!(value["import"]["promptOnDevice"], serde_json::json!(true));
    // 布局键已退役：import 序列化产物不再含 dirTemplate
    assert!(value["import"].get("dirTemplate").is_none());
    assert_eq!(
        value["import"]["duplicatePolicy"],
        serde_json::json!("skip")
    );
    assert!(value["import"].get("notifyMilestones").is_none());
    assert_eq!(value["ai"]["enableClip"], serde_json::json!(false));
    assert_eq!(value["ai"]["indexSchedule"], serde_json::json!("idleOnly"));
    assert_eq!(value["ai"]["cpuLimitPercent"], serde_json::json!(50));
    assert_eq!(value["ai"]["useGpu"], serde_json::json!(true));
    assert_eq!(value["system"]["launchAtLogin"], serde_json::json!(false));
    assert_eq!(value["system"]["closeToTray"], serde_json::json!(true));
    assert_eq!(value["system"]["language"], serde_json::json!("zh"));
    // 库注册表键不复存在
    assert!(value.get("libraries").is_none());
    assert!(value.get("activeLibraryId").is_none());

    // 两层落盘形态同 camelCase：全局层无偏好键，库级层无注册表键
    let global = serde_json::to_value(GlobalSettings::default()).unwrap();
    assert_eq!(global["onboardingCompleted"], serde_json::json!(false));
    assert!(global.get("ai").is_none());
    assert!(global.get("import").is_none());
    let database = serde_json::to_value(DatabaseSettings::default()).unwrap();
    assert_eq!(
        database["watchFolders"],
        serde_json::json!([]),
        "watchFolders（camelCase）在库级层"
    );
    assert!(database.get("databases").is_none());
    assert!(database.get("schemaVersion").is_none(), "库级无版本键");
}

#[test]
fn retired_import_notifications_are_ignored() {
    let settings: Settings = serde_json::from_value(serde_json::json!({
        "import": { "notifyMilestones": true, "duplicatePolicy": "rename" }
    }))
    .unwrap();
    assert_eq!(settings.import.duplicate_policy, DuplicatePolicy::Rename);
    let serialized = serde_json::to_value(settings).unwrap();
    assert!(serialized["import"].get("notifyMilestones").is_none());
}

/// 语义阈值默认值：None = auto（2026-09-28 三档画质起随语义模型变体自适应：
/// int8 → 0.09（2026-09-21 标定），fp16 → 标定值见 ai::semantic_default_min_score）。
#[test]
fn ai_settings_default_semantic_min_score_is_auto() {
    let ai = AiSettings::default();
    assert_eq!(ai.semantic_min_score, None, "默认 auto（null）");
}

/// 旧配置缺 semanticMinScore 字段 → serde default 落 None（auto）；缺
/// qualityTier → "normal"（ai 键落库级文件后，这组迁移在 load_database 生效）。
#[test]
fn legacy_settings_without_semantic_min_score_gets_default() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load_composed(dir.path()).expect("load legacy settings");
    assert_eq!(loaded.ai.semantic_min_score, None, "缺字段落 auto");
    assert_eq!(loaded.ai.quality_tier, "normal", "缺字段落 normal 档");

    // 库级文件缺 ai 键同理
    let db_dir = dir.path().join("db-1");
    fs::create_dir_all(&db_dir).unwrap();
    fs::write(settings_path(&db_dir), r#"{"import":{"skipImported":false}}"#).unwrap();
    let database = SettingsManager::load_database(&db_dir).unwrap();
    assert_eq!(database.ai.semantic_min_score, None);
    assert_eq!(database.ai.quality_tier, "normal");
}

/// 显式 0.09（= 旧默认值）→ 加载迁移为 None（auto）；非 0.09 自定义值 →
/// 保留；非法档位 → 兜成 "normal"（读取侧兜底，写入侧另有硬校验）。
#[test]
fn legacy_explicit_default_threshold_migrates_to_auto_but_custom_survives() {
    let dir = temp_dir();
    let db_dir = dir.path().join("db-1");
    fs::create_dir_all(&db_dir).unwrap();

    fs::write(
        settings_path(&db_dir),
        serde_json::json!({ "ai": { "semanticMinScore": 0.09 } }).to_string(),
    )
    .unwrap();
    let database = SettingsManager::load_database(&db_dir).expect("load");
    assert_eq!(database.ai.semantic_min_score, None, "旧默认 0.09 → auto");

    fs::write(
        settings_path(&db_dir),
        serde_json::json!({ "ai": { "semanticMinScore": 0.15, "qualityTier": "accurate" } })
            .to_string(),
    )
    .unwrap();
    let database = SettingsManager::load_database(&db_dir).expect("load");
    assert_eq!(database.ai.semantic_min_score, Some(0.15), "自定义值保留");
    assert_eq!(database.ai.quality_tier, "accurate", "合法档位保留");

    fs::write(
        settings_path(&db_dir),
        serde_json::json!({ "ai": { "qualityTier": "turbo" } }).to_string(),
    )
    .unwrap();
    let database = SettingsManager::load_database(&db_dir).expect("load");
    assert_eq!(database.ai.quality_tier, "normal", "非法档位兜成 normal");
}

/// settings_set 的档位硬校验：三档通过，脏值拒绝。
#[test]
fn validate_ai_settings_rejects_bad_tier() {
    for ok in ["fast", "normal", "accurate"] {
        let ai = AiSettings {
            quality_tier: ok.to_string(),
            ..AiSettings::default()
        };
        assert!(
            validate_ai_settings(&ai).is_ok(),
            "{ok} 应合法"
        );
    }
    let bad = AiSettings {
        quality_tier: "ultra".to_string(),
        ..AiSettings::default()
    };
    assert!(validate_ai_settings(&bad).is_err());
}

// ---------------------------------------------------------------------------
// 一次性迁移（设置独立改造：旧全量单文件 → 全局/库级两层）
// ---------------------------------------------------------------------------

/// 旧全量全局文件（含 db 级键）+ 激活库 → db 级值写入激活库库级文件，
/// 全局文件瘦身为纯全局形态，合成快照还原旧值。
#[test]
fn migration_splits_legacy_global_into_active_db_and_slims_global() {
    let dir = temp_dir();
    let db_dir = dir.path().join("db-1");
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": 1,
            "onboardingCompleted": true,
            "databases": [{ "id": "db-1", "name": "主数据库",
                            "dbDir": db_dir.to_string_lossy() }],
            "activeDatabaseId": "db-1",
            "import": { "skipImported": false },
            "ai": { "enableClip": true, "semanticMinScore": 0.09 },
            "watchFolders": [r"I:\incoming"]
        })
        .to_string(),
    )
    .unwrap();

    let composed = SettingsManager::load_composed(dir.path()).unwrap();
    assert!(!composed.import.skip_imported, "旧 import 值落位激活库");
    assert!(composed.ai.enable_clip);
    assert_eq!(composed.ai.semantic_min_score, None, "迁移中顺带 0.09→auto");
    assert_eq!(composed.watch_folders, vec![r"I:\incoming".to_string()]);

    // 库级文件物化在 db_dir；全局文件瘦身（不再含 db 级键）
    let database = SettingsManager::load_database(&db_dir).unwrap();
    assert!(!database.import.skip_imported);
    assert!(database.ai.enable_clip);
    let raw = fs::read_to_string(settings_path(dir.path())).unwrap();
    assert!(!raw.contains("\"ai\""), "全局文件已瘦身：{raw}");
    assert!(!raw.contains("\"import\""));
    assert!(raw.contains("\"databases\""), "注册表留全局");
    assert!(raw.contains("\"onboardingCompleted\""));

    // 幂等：再跑一遍不变化、不覆盖
    let again = SettingsManager::load_composed(dir.path()).unwrap();
    assert_eq!(again, composed);
}

/// 库级文件已存在（用户拆层后改过偏好）→ 迁移不覆盖，盘上真值优先。
#[test]
fn migration_does_not_overwrite_existing_db_file() {
    let dir = temp_dir();
    let db_dir = dir.path().join("db-1");
    fs::create_dir_all(&db_dir).unwrap();
    fs::write(
        settings_path(&db_dir),
        serde_json::json!({ "ai": { "enableClip": false, "useGpu": false } }).to_string(),
    )
    .unwrap();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": 1,
            "databases": [{ "id": "db-1", "name": "主数据库",
                            "dbDir": db_dir.to_string_lossy() }],
            "activeDatabaseId": "db-1",
            "ai": { "enableClip": true, "useGpu": true }
        })
        .to_string(),
    )
    .unwrap();

    let composed = SettingsManager::load_composed(dir.path()).unwrap();
    assert!(!composed.ai.enable_clip, "库级盘上真值优先于迁移值");
    assert!(!composed.ai.use_gpu);
    // 全局文件仍完成瘦身（迁移的另一半照做）
    let raw = fs::read_to_string(settings_path(dir.path())).unwrap();
    assert!(!raw.contains("\"ai\""));
}

/// 无激活库的旧全量文件 → db 级值丢弃（无处安放，回默认比错挂他库好），
/// 全局文件照样瘦身；不物化任何库级文件。
#[test]
fn migration_without_active_db_drops_db_level_values() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": 1,
            "onboardingCompleted": true,
            "databases": [{ "id": "db-1", "name": "主数据库",
                            "dbDir": dir.path().join("db-1").to_string_lossy() }],
            "activeDatabaseId": null,
            "ai": { "enableClip": true }
        })
        .to_string(),
    )
    .unwrap();

    let composed = SettingsManager::load_composed(dir.path()).unwrap();
    assert!(!composed.ai.enable_clip, "无激活库：db 级值丢弃回默认");
    assert!(composed.onboarding_completed, "全局键保留");
    assert_eq!(composed.databases.len(), 1, "注册表保留");
    assert!(
        !dir.path().join("db-1").join("settings.json").exists(),
        "不物化库级文件"
    );
    let raw = fs::read_to_string(settings_path(dir.path())).unwrap();
    assert!(!raw.contains("\"ai\""), "全局文件瘦身照做");
}

// ---------------------------------------------------------------------------
// 路径归一化闸门（照片库 root / 数据库位置共用内核）
// ---------------------------------------------------------------------------

/// 盘符相对路径（`I:foo`，无根分量 → is_absolute()==false）与普通相对路径
/// 一律拒绝——真机实证 `I:SmartPhotoedge-photos` 曾按进程 CWD 解析。
#[test]
fn normalize_library_path_rejects_non_absolute() {
    for bad in [
        "I:SmartPhotoedge-photos",
        "I:foo\\bar",
        "relative\\path",
        "relative/path",
        "  ",
    ] {
        let err = normalize_library_path(bad).unwrap_err();
        assert!(err.contains("必须是绝对路径"), "{bad} 应被拒绝：{err}");
    }
}

#[test]
#[cfg(windows)]
fn normalize_library_path_folds_forward_slashes_and_dots() {
    let dir = temp_dir();
    // 正斜杠输入折成反斜杠（组件级逻辑归一，不做 canonicalize）
    let raw = dir.path().to_string_lossy().replace('\\', "/");
    let norm = normalize_library_path(&raw).unwrap();
    assert_eq!(
        Path::new(&norm),
        dir.path(),
        "正斜杠绝对路径应规范化为反斜杠形态"
    );

    // 尚未创建（新建照片库，目录还没落）：组件级逻辑归一
    let missing = format!("{}/a/./b/../created", raw);
    let norm2 = normalize_library_path(&missing).unwrap();
    assert_eq!(
        Path::new(&norm2),
        dir.path().join("a").join("created"),
        "逻辑归一：折叠 . 与 ..、正斜杠折反斜杠（{missing} → {norm2}）"
    );

    // 已规范化的值再过一遍幂等
    assert_eq!(normalize_library_path(&norm).unwrap(), norm);
}

// ---------------------------------------------------------------------------
// 数据库注册表（2026-10-09 多数据库修正：databases + activeDatabaseId）
// ---------------------------------------------------------------------------

/// 解析契约：无激活数据库 → Err("尚未创建数据库")；激活条目 db_dir 优先。
#[test]
fn active_database_dir_requires_active_entry() {
    let dir = temp_dir();
    let mut s = Settings::default();
    let err = s.active_database_dir(dir.path()).unwrap_err();
    assert_eq!(err, "尚未创建数据库");

    let custom = dir.path().join("custom-db");
    s.databases = vec![DatabaseEntry {
        id: "db-1".into(),
        name: "主数据库".into(),
        db_dir: custom.to_string_lossy().into_owned(),
    }];
    // 指向不存在的激活 id（注册表被手改坏）→ Err 拒绝
    s.active_database_id = Some("db-x".into());
    assert!(s.active_database_dir(dir.path()).is_err());
    // 正常：激活条目 db_dir 即数据库目录
    s.active_database_id = Some("db-1".into());
    assert_eq!(
        s.active_database_dir(dir.path()).unwrap(),
        custom,
        "激活数据库目录 = 注册表条目 db_dir"
    );
    // 注册表序首个 id（database_remove 删激活库后的切换目标）
    assert_eq!(s.first_database_id(), Some("db-1"));
    assert_eq!(Settings::default().first_database_id(), None);
    // GlobalSettings 同语义（注册表正主在全局层）
    let (global, _) = s.split();
    assert_eq!(global.first_database_id(), Some("db-1"));
    assert_eq!(global.active_database_dir(), Some(custom));
    assert_eq!(GlobalSettings::default().first_database_id(), None);
}

/// roundtrip：databases/activeDatabaseId 落盘读回；旧配置缺键 → 空表/None。
#[test]
fn database_registry_roundtrips_and_missing_keys_default_empty() {
    let dir = temp_dir();
    let mut s = Settings::default();
    s.databases = vec![DatabaseEntry {
        id: "db-1".into(),
        name: "主数据库".into(),
        db_dir: r"I:\SmartPhoto\db".into(),
    }];
    s.active_database_id = Some("db-1".into());
    save_split(&s, dir.path());
    let loaded = SettingsManager::load_composed(dir.path()).unwrap();
    assert_eq!(loaded.databases, s.databases);
    assert_eq!(loaded.active_database_id.as_deref(), Some("db-1"));

    // 旧配置（无 databases/activeDatabaseId 键）→ serde default 空表/None；
    // 旧单库 databaseDir 残留键同样被忽略（不做迁移）
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":true,"databaseDir":"I:\\old"}"#,
    )
    .unwrap();
    let legacy = SettingsManager::load_composed(dir.path()).unwrap();
    assert!(legacy.databases.is_empty());
    assert_eq!(legacy.active_database_id, None);
}

/// 注册表内互斥：db_dir 之间不得相同或互相包含（大小写/分隔符不敏感）。
#[test]
fn database_dir_overlap_registry_gate() {
    let dir = temp_dir();
    let base = dir.path().join("databases").join("db-1");
    let registry = vec![DatabaseEntry {
        id: "db-1".into(),
        name: "主数据库".into(),
        db_dir: base.to_string_lossy().into_owned(),
    }];
    // 完全相同
    assert!(validate_database_dir_overlap(&registry, &base).is_err());
    #[cfg(windows)]
    {
        // 互相包含（新库套旧库 / 旧库套新库）+ 大小写与正斜杠变形
        let inside = base.join("nested");
        assert!(validate_database_dir_overlap(&registry, &inside).is_err());
        let variant = base.to_string_lossy().replace('\\', "/");
        assert!(validate_database_dir_overlap(&registry, std::path::Path::new(&variant)).is_err());
        // 前缀相近但非包含（db-1 vs db-10）必须放行
        let sibling = dir.path().join("databases").join("db-10");
        assert!(validate_database_dir_overlap(&registry, &sibling).is_ok());
    }
    // 空注册表放行
    assert!(validate_database_dir_overlap(&[], &base).is_ok());
}

/// 归一化闸门：相对/盘符相对路径拒绝、盘根拒绝、正斜杠与 `.`/`..` 折叠。
#[test]
fn normalize_database_path_gates_relative_and_drive_root() {
    for bad in ["I:db", "relative\\db", "relative/db", "  "] {
        let err = normalize_database_path(bad).unwrap_err();
        assert!(err.contains("绝对路径"), "{bad} 应被拒绝：{err}");
    }
    // 盘根拒绝（临时目录所在卷的根）
    let volume_root = dir_path_ancestors_root(&temp_dir());
    let err = normalize_database_path(&volume_root).unwrap_err();
    assert!(err.contains("根目录"), "{volume_root} 应被拒绝");

    // 合法路径：正斜杠折叠 + 幂等
    #[cfg(windows)]
    {
        let dir = temp_dir();
        let raw = format!("{}/a/./b", dir.path().to_string_lossy().replace('\\', "/"));
        let norm = normalize_database_path(&raw).unwrap();
        assert_eq!(Path::new(&norm), dir.path().join("a").join("b"));
        assert_eq!(normalize_database_path(&norm).unwrap(), norm, "幂等");
    }
}

fn dir_path_ancestors_root(dir: &tempfile::TempDir) -> String {
    dir.path()
        .ancestors()
        .last()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}
