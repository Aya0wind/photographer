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
    pub onboarding_completed: bool,
    /// 库注册表（达芬奇式：每个库是独立数据单元，见设计文档 §5.11）。
    pub libraries: Vec<Library>,
    pub active_library_id: Option<String>,
    pub import: ImportSettings,
    pub ai: AiSettings,
    pub system: SystemSettings,
    /// 存储与缓存策略（M8-③：缩略图缓存 LRU 上限等）。
    pub storage: StorageSettings,
    /// 监视文件夹（F4：后台轮询发现新文件自动入册；绝对路径形态）。
    /// v1 默认空；旧 settings.json 缺字段由 serde default 容错填充。
    pub watch_folders: Vec<String>,
}

impl Settings {
    /// 当前激活的库（按 id 在注册表中查找；未设置或找不到返回 None）。
    pub fn active_library(&self) -> Option<&Library> {
        let id = self.active_library_id.as_ref()?;
        self.libraries.iter().find(|lib| &lib.id == id)
    }
}

/// 库 = 独立数据单元：`db_dir` 数据库目录自包含（SQLite/缩略图/向量/日志），
/// `photo_root` 照片存储目录与之分离；两者均可迁移。
///
/// M2 起**导入目录属性是库属性**（建库时填写、导入时只读，用户规定
/// 2026-09-18）：`dir_template` 目录模板与 `import_subdir` 导入子目录随库
/// 保存；旧 settings.json 缺这两字段时按默认值容错填充（不升 schema）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub name: String,
    pub db_dir: String,
    pub photo_root: String,
    /// 导入目录模板（库属性：导入时只读）。
    #[serde(default = "default_dir_template")]
    pub dir_template: String,
    /// 卡/相机导入的专用子目录名（相对 photoRoot 的应用写入区；库属性）。
    #[serde(default = "default_import_subdir")]
    pub import_subdir: String,
    /// 配置链是否走完（达芬奇式启动流，用户规定 2026-09-18：每次启动先进
    /// 库选择器）：新建库为 false，走完库配置链置 true；选择器据此决定
    /// 是否继续进入配置向导。旧 settings.json 缺字段 → false，由 load 的
    /// 一次性迁移平滑处理（见 `SettingsManager::load`）。
    #[serde(default)]
    pub configured: bool,
    /// 导入并发流数（库属性，用户规定 2026-09-19：向导内不可改）。
    /// MTP 源后端仍强制单流，此处值仅作用于卷/文件夹源。
    #[serde(default = "default_streams")]
    pub streams: u32,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            db_dir: String::new(),
            photo_root: String::new(),
            dir_template: default_dir_template(),
            import_subdir: default_import_subdir(),
            configured: false,
            streams: default_streams(),
        }
    }
}

/// 库级目录模板默认值（与建库默认一致，见 ImportSettings 注释）。
fn default_dir_template() -> String {
    "{YYYY}/{MM-DD}/{原文件名}".to_string()
}

/// 库级导入子目录默认值。
fn default_import_subdir() -> String {
    "SmartPhoto".to_string()
}

/// 库级并发流数默认值（卷/文件夹源）。
fn default_streams() -> u32 {
    4
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            onboarding_completed: false,
            libraries: Vec::new(),
            active_library_id: None,
            import: ImportSettings::default(),
            ai: AiSettings::default(),
            system: SystemSettings::default(),
            storage: StorageSettings::default(),
            watch_folders: Vec::new(),
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
    /// **已降级（M2 用户规定 2026-09-18）**：目录模板/导入子目录是库属性
    /// （`Library.dir_template` / `Library.import_subdir`，建库时填写、导入时
    /// 只读）。本字段保留仅作**创建新库时的默认值**，存量配置不断裂。
    pub dir_template: String,
    pub duplicate_policy: DuplicatePolicy,
    pub notify_milestones: bool,
    /// **已降级**：语义同 `dir_template`——新库 `import_subdir` 的默认值
    /// （spec §5.11：photoRoot 归用户管理，应用只写 `photoRoot\import_subdir`）。
    pub import_subdir: String,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            prompt_on_device: true,
            skip_imported: true,
            dir_template: default_dir_template(),
            duplicate_policy: DuplicatePolicy::Skip,
            notify_milestones: true,
            import_subdir: default_import_subdir(),
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
    /// 语义检索相似度阈值（cos，0..1）：低于该分的结果过滤；0 = 不过滤。
    /// 默认 0.09——SigLIP2 cos 分数区间压缩（实测无关内容 top≈0.087，
    /// 相关簇 ≈0.099+），过高全灭、过低「进哪个相册都是全部照片」。
    pub semantic_min_score: f32,
    /// 语义嵌入输入档位（px，squash 到 embed_input_size²）：默认 256。
    /// **改了必须重建语义索引**（嵌入向量随输入尺寸变化）——启动时经
    /// dbDir/index-params.marker 指纹比对自动重建，设置页另有手动按钮。
    pub embed_input_size: u16,
    /// SCRFD 检测框置信门槛（0-1）：默认 0.5。改了需重建人脸索引。
    pub face_detect_threshold: f32,
    /// 在线聚类归簇 cos 阈值（0-1）：默认 0.4。改了需重建人脸索引。
    pub face_cluster_threshold: f32,
    /// AI 索引参数指纹版本（内部）：参数语义变更时递增，强制全通道重建。
    pub index_params_version: u32,
    /// 连拍分组：相邻照片时间链间隔上限（毫秒，默认 2000）。
    /// 改参数只重组不重算 pHash。
    pub burst_gap_ms: u32,
    /// 连拍分组：pHash 汉明距离上限（默认 10，>此判为场景切换）。
    pub burst_hamming_max: u8,
    /// 连拍分组：成组最小成员数（默认 2；单张不成组）。
    pub burst_min_size: u32,
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
            semantic_min_score: 0.09,
            embed_input_size: 256,
            face_detect_threshold: 0.5,
            face_cluster_threshold: 0.4,
            index_params_version: 1,
            burst_gap_ms: 2000,
            burst_hamming_max: 10,
            burst_min_size: 2,
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

/// 存储与缓存策略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StorageSettings {
    /// 缩略图缓存总量上限（GB；默认 20，0 = 不限）。命中 touch mtime 续命，
    /// 超限后台 LRU 淘汰（跳过 60s 内新写入的文件）。
    pub thumb_cache_max_gb: u32,
}

impl Default for StorageSettings {
    fn default() -> Self {
        Self {
            thumb_cache_max_gb: 20,
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
                migrate_legacy_libraries(&mut settings);
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

/// 存量库一次性迁移（达芬奇式启动流，2026-09-18）：旧模型没有
/// `configured` 标记——已完成引导且持有库的用户视为库配置早已走完：
/// 若 `onboarding_completed` 且库列表非空且**所有库都未 configured**，
/// 全部标 true（部分库已 configured 说明已是新模型写入，不再迁移，
/// 避免波及用户新建未配置的库）。迁移结果在下次 save 时落盘。
fn migrate_legacy_libraries(settings: &mut Settings) {
    if settings.onboarding_completed
        && !settings.libraries.is_empty()
        && settings.libraries.iter().all(|lib| !lib.configured)
    {
        for lib in &mut settings.libraries {
            lib.configured = true;
        }
    }
}
