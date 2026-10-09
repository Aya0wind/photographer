//! SettingsManager / Settings 行为测试（2026-10-09 单数据库多照片库新语义）：
//! settings.json 只留应用级设置——默认值、缺文件、roundtrip、损坏 JSON
//! 恢复、原子写、旧键忽略（库注册表残留键 serde 丢弃）、camelCase 序列化、
//! 「数据库位置」归一化闸门。

use std::fs;
use std::path::{Path, PathBuf};

use smart_photo_lib::settings::{
    normalize_database_path, normalize_library_path, normalize_settings_database_dir,
    validate_ai_settings, AiSettings, DuplicatePolicy, ImportSettings, IndexSchedule, Settings,
    SettingsError, SettingsManager, SystemSettings, SCHEMA_VERSION,
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
    assert!(!s.onboarding_completed);
    // 「数据库位置」（§一）：默认 None = 应用数据目录
    assert_eq!(s.database_dir, None);

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
        onboarding_completed: true,
        database_dir: Some(r"I:\SmartPhoto\db".to_string()),
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
    // 旧字段 libraryRoot 被忽略（serde 默认不拒绝未知字段）
    assert!(s.onboarding_completed);
    assert_eq!(s.import, ImportSettings::default());
    assert_eq!(s.ai.index_schedule, IndexSchedule::IdleOnly);
    assert_eq!(s.system.language, "zh");
}

/// 旧库注册表残留键（libraries/activeLibraryId/dbDir 等）整体忽略（§二
/// 退役，不做迁移）：加载不报错、不复活任何库概念字段。
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

    let s = SettingsManager::load(dir.path()).expect("load");
    assert_eq!(s.schema_version, SCHEMA_VERSION, "不升 schema_version");
    assert_eq!(
        serde_json::to_value(&s).unwrap()["libraries"],
        serde_json::json!(null),
        "库注册表字段已不存在于序列化产物"
    );
    assert_eq!(s.import, ImportSettings::default());

    // 回存后旧键消失（落盘产物 = 纯应用级设置）
    SettingsManager::save(&s, dir.path()).unwrap();
    let raw = fs::read_to_string(settings_path(dir.path())).unwrap();
    assert!(!raw.contains("libraries"));
    assert!(!raw.contains("activeLibraryId"));
    assert!(!raw.contains("photoRoot"));
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
    assert_eq!(value["onboardingCompleted"], serde_json::json!(false));
    assert_eq!(value["databaseDir"], serde_json::json!(null));
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
            "onboardingCompleted": true
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
// 「数据库位置」设置项（§一：默认应用数据目录，可改）
// ---------------------------------------------------------------------------

/// 解析契约：None → 应用数据目录；Some → 自定义路径优先。
#[test]
fn database_dir_resolution_defaults_to_app_data_dir() {
    let dir = temp_dir();
    let mut s = Settings::default();
    assert_eq!(
        s.database_dir_path(dir.path()),
        dir.path().to_path_buf(),
        "缺省 = 应用数据目录（settings.json 所在处）"
    );
    s.database_dir = Some(dir.path().join("custom-db").to_string_lossy().into_owned());
    assert_eq!(
        s.database_dir_path(dir.path()),
        dir.path().join("custom-db"),
        "自定义位置优先于应用数据目录"
    );
}

/// roundtrip：databaseDir 落盘/读回；旧 settings.json 缺键 → None（默认）。
#[test]
fn database_dir_roundtrips_and_missing_key_defaults_to_none() {
    let dir = temp_dir();
    let mut s = Settings::default();
    s.database_dir = Some(r"I:\SmartPhoto\db".to_string());
    SettingsManager::save(&s, dir.path()).unwrap();
    let loaded = SettingsManager::load(dir.path()).unwrap();
    assert_eq!(loaded.database_dir.as_deref(), Some(r"I:\SmartPhoto\db"));

    // 旧配置（无 databaseDir 键）→ serde default 落 None，不报错
    fs::write(
        settings_path(dir.path()),
        r#"{"schemaVersion":1,"onboardingCompleted":true}"#,
    )
    .unwrap();
    let legacy = SettingsManager::load(dir.path()).unwrap();
    assert_eq!(legacy.database_dir, None);
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

/// Settings 级闸门：None 通过（默认应用数据目录无路径可归一）。
#[test]
fn normalize_settings_database_dir_passes_none_and_normalizes_some() {
    let mut s = Settings::default();
    normalize_settings_database_dir(&mut s).unwrap();
    assert_eq!(s.database_dir, None);
    #[cfg(windows)]
    {
        let dir = temp_dir();
        let raw = format!("{}/a/./b", dir.path().to_string_lossy().replace('\\', "/"));
        s.database_dir = Some(raw);
        normalize_settings_database_dir(&mut s).unwrap();
        assert_eq!(
            Path::new(s.database_dir.as_deref().unwrap()),
            dir.path().join("a").join("b"),
            "Some 值归一为反斜杠绝对形态（折叠 . 与正斜杠）"
        );
    }
}
