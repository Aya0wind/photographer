//! SettingsManager / Settings 行为测试：默认值、缺文件、roundtrip、
//! 损坏 JSON 恢复、原子写、旧版本容错迁移、camelCase 序列化。

use std::fs;
use std::path::{Path, PathBuf};

use smart_photo_lib::settings::{
    AiSettings, DuplicatePolicy, ImportSettings, IndexSchedule, Library, Settings, SettingsError,
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
    assert_eq!(s.import.dir_template, "{YYYY}/{MM-DD}/{原文件名}");
    assert_eq!(s.import.duplicate_policy, DuplicatePolicy::Skip);
    assert!(s.import.notify_milestones);

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
        }],
        active_library_id: Some("lib-1".to_string()),
        onboarding_completed: true,
        import: ImportSettings {
            dir_template: "{YYYY}/{原文件名}".to_string(),
            duplicate_policy: DuplicatePolicy::Rename,
            skip_imported: false,
            notify_milestones: false,
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
    assert_eq!(
        value["import"]["dirTemplate"],
        serde_json::json!("{YYYY}/{MM-DD}/{原文件名}")
    );
    assert_eq!(
        value["import"]["duplicatePolicy"],
        serde_json::json!("skip")
    );
    assert_eq!(value["import"]["notifyMilestones"], serde_json::json!(true));
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
    })
    .expect("serialize library");
    assert_eq!(lib["dbDir"], serde_json::json!(r"I:\SmartPhoto\主库"));
    assert_eq!(lib["photoRoot"], serde_json::json!(r"Y:\照片"));
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
    };
    let backup = Library {
        id: "lib-backup".to_string(),
        name: "备份库".to_string(),
        db_dir: r"I:\SmartPhoto\备份库".to_string(),
        photo_root: r"Z:\照片".to_string(),
    };
    s.libraries = vec![main.clone(), backup];
    s.active_library_id = Some("lib-main".to_string());
    assert_eq!(s.active_library(), Some(&main));

    // 指向不存在的 id -> None（注册表脏数据容错）
    s.active_library_id = Some("lib-missing".to_string());
    assert!(s.active_library().is_none());
}
