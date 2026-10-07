//! SettingsManager / Settings 行为测试：默认值、缺文件、roundtrip、
//! 损坏 JSON 恢复、原子写、旧版本容错迁移、camelCase 序列化。

use std::fs;
use std::path::{Path, PathBuf};

use smart_photo_lib::settings::{
    validate_library_storage_paths, AiSettings, DuplicatePolicy, ImportSettings, IndexSchedule,
    Library, Settings, SettingsError, SettingsManager, SystemSettings, SCHEMA_VERSION,
};

#[test]
fn new_library_requires_dedicated_directories_and_recognizes_recovery_db() {
    let temp = temp_dir();
    let db_dir = temp.path().join("database");
    let photos = temp.path().join("photos");
    let mut proposed = Settings::default();
    proposed.libraries.push(Library {
        id: "new".into(),
        name: "New library".into(),
        db_dir: db_dir.to_string_lossy().into_owned(),
        photo_root: photos.to_string_lossy().into_owned(),
        ..Library::default()
    });
    let previous = Settings::default();
    assert!(validate_library_storage_paths(&proposed, &previous).is_ok());

    fs::create_dir_all(&photos).unwrap();
    fs::write(photos.join("existing.jpg"), b"user photo").unwrap();
    assert!(validate_library_storage_paths(&proposed, &previous)
        .unwrap_err()
        .contains("照片目录必须是空目录"));
    fs::remove_file(photos.join("existing.jpg")).unwrap();

    fs::create_dir_all(&db_dir).unwrap();
    fs::write(db_dir.join("notes.txt"), b"user file").unwrap();
    assert!(validate_library_storage_paths(&proposed, &previous)
        .unwrap_err()
        .contains("数据库目录必须为空"));
    fs::remove_file(db_dir.join("notes.txt")).unwrap();
    let conn = rusqlite::Connection::open(db_dir.join("library.db")).unwrap();
    conn.execute_batch("CREATE TABLE assets (id INTEGER PRIMARY KEY)")
        .unwrap();
    drop(conn);
    assert!(validate_library_storage_paths(&proposed, &previous).is_ok());

    proposed.libraries.push(Library {
        id: "overlap".into(),
        name: "Overlap".into(),
        db_dir: temp
            .path()
            .join("database/sub")
            .to_string_lossy()
            .into_owned(),
        photo_root: temp.path().join("other").to_string_lossy().into_owned(),
        ..Library::default()
    });
    assert!(validate_library_storage_paths(&proposed, &previous)
        .unwrap_err()
        .contains("重叠"));
}

fn temp_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

fn settings_path(dir: &Path) -> PathBuf {
    dir.join("settings.json")
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

#[test]
fn default_settings_match_spec() {
    let s = Settings::default();

    assert_eq!(s.schema_version, 1);
    assert_eq!(s.schema_version, SCHEMA_VERSION);
    assert!(s.libraries.is_empty());
    assert_eq!(s.active_library_id, None);
    assert!(!s.onboarding_completed);

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

#[test]
fn load_missing_file_returns_defaults() {
    let dir = temp_dir();
    let loaded = SettingsManager::load(dir.path()).expect("load should succeed");
    assert_eq!(loaded, Settings::default());
}

#[test]
fn save_then_load_roundtrip_with_custom_values() {
    let dir = temp_dir();
    let s = Settings {
        libraries: vec![Library {
            id: "lib-1".to_string(),
            name: "主库".to_string(),
            db_dir: r"I:\SmartPhoto\主库".to_string(),
            photo_root: r"Y:\照片".to_string(),
            // 布局属性已退役（2026-09-28 固定公式，不可配置）
            // 配置链已走完（达芬奇式启动流：库选择器跳过配置向导的依据）
            configured: true,
            streams: 4,
            ai_quality_tier: Some("normal".into()),
        }],
        active_library_id: Some("lib-1".to_string()),
        onboarding_completed: true,
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
        },
        ..Settings::default()
    };

    SettingsManager::save(&s, dir.path()).expect("save should succeed");
    let loaded = SettingsManager::load(dir.path()).expect("load should succeed");
    assert_eq!(loaded, s);
    // 库属性 roundtrip 逐字段确认（不依赖整体相等）
    let lib = loaded.libraries.first().expect("library kept");
    assert!(lib.configured, "configured roundtrip 保留");
}

#[test]
fn corrupted_json_is_backed_up_and_defaults_returned() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), "{{{ 这不是 JSON").expect("write corrupt file");

    let loaded = SettingsManager::load(dir.path()).expect("load should recover");
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
    SettingsManager::save(&Settings::default(), dir.path()).expect("save");
    assert!(settings_path(dir.path()).exists());
    assert!(
        !dir.path().join("settings.json.tmp").exists(),
        "no .tmp file should remain after save"
    );

    // 覆盖保存同样不留残留
    let s = Settings {
        onboarding_completed: true,
        ..Settings::default()
    };
    SettingsManager::save(&s, dir.path()).expect("overwrite save");
    assert!(!dir.path().join("settings.json.tmp").exists());
    let reloaded = SettingsManager::load(dir.path()).expect("reload");
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

    let s = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(s.schema_version, SCHEMA_VERSION);
    // 旧字段 libraryRoot 被忽略（serde 默认不拒绝未知字段），库注册表回退默认。
    assert!(s.libraries.is_empty());
    assert_eq!(s.active_library_id, None);
    assert!(s.onboarding_completed);
    assert_eq!(s.import, ImportSettings::default());
    assert_eq!(s.ai.index_schedule, IndexSchedule::IdleOnly);
    assert_eq!(s.system.language, "zh");
}

/// 布局属性退役（2026-09-28 固定公式）：旧 settings.json 残留的
/// dirTemplate/importSubdir 键（库级与 import 级）加载时被忽略，不报错、
/// 不升 schema_version。
#[test]
fn retired_layout_keys_in_old_settings_are_ignored() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"activeLibraryId":"lib-1","libraries":[
            {"id":"lib-1","name":"主库","dbDir":"I:\\SmartPhoto\\主库","photoRoot":"Y:\\照片",
             "dirTemplate":"{YYYY}/{MM-DD}","importSubdir":"卡导入区"}
        ],"import":{"dirTemplate":"{YYYY}","importSubdir":"旧子目录"}}"#,
    )
    .expect("write old-style settings");

    let s = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(s.schema_version, SCHEMA_VERSION, "不升 schema_version");
    let lib = s.libraries.first().expect("library kept");
    assert_eq!(lib.id, "lib-1");
    assert_eq!(s.import, ImportSettings::default());
}

/// 达芬奇式启动流（用户规定）：configured 库级标记 + 存量一次性迁移。
#[test]
fn legacy_onboarded_libraries_migrated_to_configured() {
    let dir = temp_dir();
    // 旧 JSON：Library 无 configured（也无目录属性字段），但引导已完成
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":true,"activeLibraryId":"lib-1","libraries":[
            {"id":"lib-1","name":"主库","dbDir":"I:\\SmartPhoto\\主库","photoRoot":"Y:\\照片"},
            {"id":"lib-2","name":"备份库","dbDir":"I:\\SmartPhoto\\备份库","photoRoot":"Z:\\照片"}
        ]}"#,
    )
    .expect("write legacy settings");

    let s = SettingsManager::load(dir.path()).expect("load");
    assert!(
        s.libraries.iter().all(|lib| lib.configured),
        "已引导的存量库应全部迁移为 configured"
    );

    // 迁移结果落盘后再次加载稳定（不再依赖迁移路径）
    SettingsManager::save(&s, dir.path()).expect("save");
    let reloaded = SettingsManager::load(dir.path()).expect("reload");
    assert!(reloaded.libraries.iter().all(|lib| lib.configured));
}

#[test]
fn unonboarded_libraries_stay_unconfigured() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":false,"activeLibraryId":null,"libraries":[
            {"id":"lib-1","name":"主库","dbDir":"I:\\SmartPhoto\\主库","photoRoot":"Y:\\照片"}
        ]}"#,
    )
    .expect("write unonboarded settings");

    let s = SettingsManager::load(dir.path()).expect("load");
    assert!(
        s.libraries.iter().all(|lib| !lib.configured),
        "未走完引导的库不得被迁移"
    );
}

#[test]
fn partial_configured_libraries_are_not_touched_by_migration() {
    let dir = temp_dir();
    // 已有部分库 configured（新模型写入过）→ 迁移条件不满足，剩余库保持原状
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":true,"activeLibraryId":"lib-1","libraries":[
            {"id":"lib-1","name":"主库","dbDir":"I:\\a","photoRoot":"Y:\\a","configured":true},
            {"id":"lib-2","name":"新库","dbDir":"I:\\b","photoRoot":"Y:\\b"}
        ]}"#,
    )
    .expect("write mixed settings");

    let s = SettingsManager::load(dir.path()).expect("load");
    fn by_id<'a>(s: &'a Settings, id: &str) -> &'a Library {
        s.libraries
            .iter()
            .find(|lib| lib.id == id)
            .unwrap_or_else(|| panic!("missing {id}"))
    }
    assert!(by_id(&s, "lib-1").configured);
    assert!(
        !by_id(&s, "lib-2").configured,
        "存在已配置库时迁移不得波及未配置库（新模型语义优先）"
    );
}

#[test]
fn empty_json_object_yields_defaults() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), "{}").expect("write empty object");
    let s = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(s, Settings::default());
}

#[test]
fn newer_schema_version_is_migration_error() {
    let dir = temp_dir();
    fs::write(settings_path(dir.path()), r#"{"schemaVersion":99}"#).expect("write future settings");
    let result = SettingsManager::load(dir.path());
    assert!(matches!(result, Err(SettingsError::Migration(99))));
}

#[test]
fn serialization_uses_camel_case() {
    let value = serde_json::to_value(Settings::default()).expect("serialize");
    assert_eq!(value["schemaVersion"], serde_json::json!(1));
    assert_eq!(value["libraries"], serde_json::json!([]));
    assert_eq!(value["activeLibraryId"], serde_json::json!(null));
    assert_eq!(value["onboardingCompleted"], serde_json::json!(false));
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

    // Library 结构的 camelCase 字段
    let lib = serde_json::to_value(Library {
        id: "a".to_string(),
        name: "主库".to_string(),
        db_dir: r"I:\SmartPhoto\主库".to_string(),
        photo_root: r"Y:\照片".to_string(),
        configured: true,
        streams: 3,
        ai_quality_tier: Some("accurate".into()),
    })
    .expect("serialize library");
    assert_eq!(lib["dbDir"], serde_json::json!(r"I:\SmartPhoto\主库"));
    assert_eq!(lib["photoRoot"], serde_json::json!(r"Y:\照片"));
    // 布局键（dirTemplate/importSubdir）已退役：序列化产物不再包含
    assert!(lib.get("dirTemplate").is_none());
    assert!(lib.get("importSubdir").is_none());
    assert_eq!(lib["configured"], serde_json::json!(true));
    assert_eq!(lib["streams"], serde_json::json!(3));
    assert_eq!(lib["aiQualityTier"], serde_json::json!("accurate"));
    // 缺省库序列化同样带 camelCase 键与默认值
    let default_lib = serde_json::to_value(Library::default()).expect("serialize default library");
    assert_eq!(default_lib["streams"], serde_json::json!(4));
}

#[test]
fn active_library_lookup_follows_active_id() {
    let mut s = Settings::default();
    assert!(s.active_library().is_none());

    let main = Library {
        id: "lib-main".to_string(),
        name: "主库".to_string(),
        db_dir: r"I:\SmartPhoto\主库".to_string(),
        photo_root: r"Y:\照片".to_string(),
        ..Library::default()
    };
    let backup = Library {
        id: "lib-backup".to_string(),
        name: "备份库".to_string(),
        db_dir: r"I:\SmartPhoto\备份库".to_string(),
        photo_root: r"Z:\照片".to_string(),
        ..Library::default()
    };
    s.libraries = vec![main.clone(), backup];
    s.active_library_id = Some("lib-main".to_string());
    assert_eq!(s.active_library(), Some(&main));

    // 指向不存在的 id -> None（注册表脏数据容错）
    s.active_library_id = Some("lib-missing".to_string());
    assert!(s.active_library().is_none());
}

#[test]
fn library_quality_tiers_are_independent_across_switches_and_edits() {
    use smart_photo_lib::settings::normalize_library_quality_tiers;
    let mut previous = Settings::default();
    previous.libraries = vec![
        Library {
            id: "main".into(),
            ai_quality_tier: Some("normal".into()),
            ..Library::default()
        },
        Library {
            id: "new".into(),
            ai_quality_tier: Some("fast".into()),
            ..Library::default()
        },
    ];
    previous.active_library_id = Some("main".into());
    let mut switched = previous.clone();
    switched.active_library_id = Some("new".into());
    normalize_library_quality_tiers(&mut switched, &previous).unwrap();
    assert_eq!(switched.ai.quality_tier, "fast");
    let mut edited = switched.clone();
    edited.ai.quality_tier = "accurate".into();
    normalize_library_quality_tiers(&mut edited, &switched).unwrap();
    assert_eq!(
        edited.libraries[1].ai_quality_tier.as_deref(),
        Some("accurate")
    );
    assert_eq!(
        edited.libraries[0].ai_quality_tier.as_deref(),
        Some("normal")
    );
    let mut back = edited.clone();
    back.active_library_id = Some("main".into());
    normalize_library_quality_tiers(&mut back, &edited).unwrap();
    assert_eq!(back.ai.quality_tier, "normal");
    let dir = temp_dir();
    SettingsManager::save(&back, dir.path()).unwrap();
    assert_eq!(
        SettingsManager::load(dir.path()).unwrap().libraries,
        back.libraries
    );
}

#[test]
fn legacy_quality_tier_is_migrated_per_library_and_gallery_preference_roundtrips() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "activeLibraryId": "old",
            "libraries": [{"id":"old"}],
            "ai": {"qualityTier":"accurate"},
            "gallery": {"mergeRawJpg":false}
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load(dir.path()).unwrap();
    assert_eq!(
        loaded.libraries[0].ai_quality_tier.as_deref(),
        Some("accurate")
    );
    assert!(!loaded.gallery.merge_raw_jpg);
    SettingsManager::save(&loaded, dir.path()).unwrap();
    assert_eq!(SettingsManager::load(dir.path()).unwrap(), loaded);
}

/// 语义阈值默认值：None = auto（2026-09-28 三档画质起随语义模型变体自适应：
/// int8 → 0.09（2026-09-21 标定），fp16 → 标定值见 ai::semantic_default_min_score）。
#[test]
fn ai_settings_default_semantic_min_score_is_auto() {
    let ai = AiSettings::default();
    assert_eq!(ai.semantic_min_score, None, "默认 auto（null）");
}

/// 旧配置缺 semanticMinScore 字段 → serde default 落 None（auto）；显式
/// 0.09（= 旧默认值）→ 加载迁移为 None（auto）；非 0.09 自定义值 → 保留。
/// 画质档位缺字段 → "normal"；非法值 → 兜成 "normal"。
#[test]
fn legacy_settings_without_semantic_min_score_gets_default() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true,
            "libraries": [],
            "activeLibraryId": null
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load(dir.path()).expect("load legacy settings");
    assert_eq!(loaded.ai.semantic_min_score, None, "缺字段落 auto");
    assert_eq!(loaded.ai.quality_tier, "normal", "缺字段落 normal 档");
}

#[test]
fn legacy_explicit_default_threshold_migrates_to_auto_but_custom_survives() {
    let dir = temp_dir();
    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true,
            "libraries": [],
            "activeLibraryId": null,
            "ai": { "semanticMinScore": 0.09 }
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(loaded.ai.semantic_min_score, None, "旧默认 0.09 → auto");

    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true,
            "libraries": [],
            "activeLibraryId": null,
            "ai": { "semanticMinScore": 0.15, "qualityTier": "accurate" }
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(loaded.ai.semantic_min_score, Some(0.15), "自定义值保留");
    assert_eq!(loaded.ai.quality_tier, "accurate", "合法档位保留");

    fs::write(
        settings_path(dir.path()),
        serde_json::json!({
            "schemaVersion": SCHEMA_VERSION,
            "onboardingCompleted": true,
            "libraries": [],
            "activeLibraryId": null,
            "ai": { "qualityTier": "turbo" }
        })
        .to_string(),
    )
    .unwrap();
    let loaded = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(loaded.ai.quality_tier, "normal", "非法档位兜成 normal");
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
            smart_photo_lib::settings::validate_ai_settings(&ai).is_ok(),
            "{ok} 应合法"
        );
    }
    let bad = AiSettings {
        quality_tier: "ultra".to_string(),
        ..AiSettings::default()
    };
    assert!(smart_photo_lib::settings::validate_ai_settings(&bad).is_err());
}

// ---------------------------------------------------------------------------
// 库路径规范化（2026-09-28 边界修复：建库统一闸门）
// ---------------------------------------------------------------------------

use smart_photo_lib::settings::{normalize_library_path, normalize_library_paths};

/// 盘符相对路径（`I:foo`，无根分量 → is_absolute()==false）与普通相对路径
/// 一律拒绝——真机实证 `I:SmartPhotoedge-photos` 曾按进程 CWD 解析，
/// DB 落到 src-tauri/ 下还触发 dev watcher 风暴。
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
        assert!(err.contains("路径必须是绝对路径"), "{bad} 应被拒绝：{err}");
    }
}

#[test]
#[cfg(windows)]
fn normalize_library_path_folds_forward_slashes_and_dots() {
    let dir = temp_dir();
    // 存在于盘：canonicalize（剥 \\?\ verbatim 前缀），正斜杠输入折成反斜杠
    let raw = dir.path().to_string_lossy().replace('\\', "/");
    let norm = normalize_library_path(&raw).unwrap();
    assert_eq!(
        Path::new(&norm),
        dir.path(),
        "正斜杠绝对路径应规范化为反斜杠 canonical 形态"
    );
    assert!(!norm.contains(r"\\?\"), "不得带 verbatim 前缀: {norm}");

    // 尚未创建（onboarding 新建库，目录还没落）：组件级逻辑归一
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

#[test]
#[cfg(windows)]
fn normalize_library_paths_rewrites_all_libraries() {
    let dir = temp_dir();
    let photo_root = tempfile::tempdir().unwrap();
    let mut s = Settings::default();
    s.libraries.push(Library {
        id: "lib-1".into(),
        name: "主库".into(),
        db_dir: format!(
            "{}/SmartPhoto/db",
            dir.path().to_string_lossy().replace('\\', "/")
        ),
        photo_root: photo_root.path().to_string_lossy().replace('\\', "/"),
        ..Library::default()
    });
    smart_photo_lib::settings::normalize_library_paths(&mut s).unwrap();
    let lib = &s.libraries[0];
    assert_eq!(
        Path::new(&lib.db_dir),
        dir.path().join("SmartPhoto").join("db"),
        "dbDir 规范化为反斜杠绝对形态"
    );
    assert_eq!(
        Path::new(&lib.photo_root),
        photo_root.path(),
        "photoRoot（存在于盘）取 canonical 形态"
    );
}

#[test]
fn normalize_library_paths_rejects_bad_library_and_keeps_input() {
    let mut s = Settings::default();
    s.libraries.push(Library {
        id: "lib-1".into(),
        name: "边界库".into(),
        db_dir: r"I:\SmartPhoto\db".into(),
        photo_root: "I:edge-photos".into(), // 盘符相对：拒绝整次写入
        ..Library::default()
    });
    let err = normalize_library_paths(&mut s).unwrap_err();
    assert!(err.contains("路径必须是绝对路径"), "{err}");
    assert_eq!(
        s.libraries[0].photo_root, "I:edge-photos",
        "非法输入不得被就地改写（拒绝语义，非静默修正）"
    );
}
