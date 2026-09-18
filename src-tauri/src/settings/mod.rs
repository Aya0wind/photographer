//! 应用配置体系：`settings.json` 的加载/保存/容错迁移。
//!
//! - 文件不存在 -> 返回全默认值（schema_version = SCHEMA_VERSION）。
//! - JSON 损坏 -> 把坏文件改名为 `settings.json.corrupt-{unix秒}` 后返回默认值，不视为错误。
//! - 旧版本缺字段 -> 依赖 serde `#[serde(default)]` 容错填充，并视为已迁移。
//! - 保存采用原子写：先写 `settings.json.tmp` 再 rename 覆盖。

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 当前配置结构版本。
pub const SCHEMA_VERSION: u32 = 1;

const SETTINGS_FILE: &str = "settings.json";

/// 应用配置根结构（camelCase JSON）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub library_root: Option<String>,
    pub onboarding_completed: bool,
    pub import: ImportSettings,
    pub ai: AiSettings,
    pub system: SystemSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            library_root: None,
            onboarding_completed: false,
            import: ImportSettings::default(),
            ai: AiSettings::default(),
            system: SystemSettings::default(),
        }
    }
}

/// 查重策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DuplicatePolicy {
    #[default]
    Skip,
    Rename,
    Ask,
}

/// AI 索引调度策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum IndexSchedule {
    #[default]
    IdleOnly,
    AfterImport,
    Manual,
}

/// 设备与导入配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportSettings {
    pub prompt_on_device: bool,
    pub skip_imported: bool,
    pub dir_template: String,
    pub duplicate_policy: DuplicatePolicy,
    pub notify_milestones: bool,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            prompt_on_device: true,
            skip_imported: true,
            dir_template: "{YYYY}/{MM-DD}/{原文件名}".to_string(),
            duplicate_policy: DuplicatePolicy::Skip,
            notify_milestones: true,
        }
    }
}

/// AI 能力与资源限制配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub enable_clip: bool,
    pub enable_face: bool,
    pub enable_scene_tags: bool,
    pub index_schedule: IndexSchedule,
    pub cpu_limit_percent: u32,
    pub use_gpu: bool,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            enable_clip: false,
            enable_face: false,
            enable_scene_tags: false,
            index_schedule: IndexSchedule::IdleOnly,
            cpu_limit_percent: 50,
            use_gpu: true,
        }
    }
}

/// 系统行为配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SystemSettings {
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub language: String,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            launch_at_login: false,
            close_to_tray: true,
            language: "zh".to_string(),
        }
    }
}

/// 配置读写错误。
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// 读写 settings.json 时的 IO 错误。
    #[error("settings io error: {0}")]
    Io(#[from] std::io::Error),
    /// 序列化/反序列化失败（保存路径）。
    #[error("settings serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    /// 配置来自更新的版本（schema_version 高于当前），无法迁移。
    #[error("settings migration failed: schema version {0} is newer than supported")]
    Migration(u32),
}

/// 配置管理器：load/save 均为无状态关联函数。
pub struct SettingsManager;

impl SettingsManager {
    /// 从 `dir` 加载 `settings.json`。
    ///
    /// 文件不存在返回默认值；JSON 损坏时把坏文件改名为
    /// `settings.json.corrupt-{unix秒}` 后返回默认值；旧版本缺字段由
    /// serde default 容错填充并升到当前 SCHEMA_VERSION（视为已迁移）。
    pub fn load(dir: &Path) -> Result<Settings, SettingsError> {
        let path = dir.join(SETTINGS_FILE);
        if !path.is_file() {
            return Ok(Settings::default());
        }
        let raw = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<Settings>(&raw) {
            Ok(mut settings) => {
                if settings.schema_version > SCHEMA_VERSION {
                    return Err(SettingsError::Migration(settings.schema_version));
                }
                // 缺字段已被 serde(default) 填充，这里统一升版本号完成迁移。
                settings.schema_version = SCHEMA_VERSION;
                Ok(settings)
            }
            Err(_parse_error) => {
                // 损坏文件备份后回归默认值，绝不因坏配置拒绝启动。
                let backup = dir.join(format!("{SETTINGS_FILE}.corrupt-{}", unix_timestamp_secs()));
                std::fs::rename(&path, &backup)?;
                Ok(Settings::default())
            }
        }
    }

    /// 原子保存：先写 `settings.json.tmp`，再 rename 覆盖 `settings.json`。
    pub fn save(settings: &Settings, dir: &Path) -> Result<(), SettingsError> {
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_string_pretty(settings)?;
        let tmp_path = dir.join(format!("{SETTINGS_FILE}.tmp"));
        std::fs::write(&tmp_path, json)?;
        std::fs::rename(&tmp_path, dir.join(SETTINGS_FILE))?;
        Ok(())
    }
}

fn unix_timestamp_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
