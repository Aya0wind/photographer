//! 应用配置体系：`settings.json` 的加载/保存/容错迁移。
//!
//! - 文件不存在 -> 返回全默认值（schema_version = SCHEMA_VERSION）。
//! - JSON 损坏 -> 把坏文件改名为 `settings.json.corrupt-{unix秒}` 后返回默认值，不视为错误。
//! - 旧版本缺字段 -> 依赖 serde `#[serde(default)]` 容错填充，并视为已迁移。
//! - 保存采用原子写：先写 `settings.json.tmp` 再 rename 覆盖。
//!
//! 2026-10-09 多数据库修正：settings.json 在应用级设置之外承载**数据库注册表**
//!（`databases` + `activeDatabaseId`）——应用可登记多个数据库（为多用户协作
//! 预埋），每个 = 独立 SQLite(library.db) + thumbs/ + 向量，落在各自的
//! db_dir；任一时刻恰有一个「激活数据库」，一切库内操作（照片库登记表/
//! 资产/相册）都作用于它，切换数据库即整体换库。旧单库时代的
//! `databaseDir` 键 serde 反序列化自动忽略（不做迁移，版本未发布红线）。

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub(crate) mod config_directory;

/// 当前配置结构版本。
pub const SCHEMA_VERSION: u32 = 1;

const SETTINGS_FILE: &str = "settings.json";

/// 应用配置根结构（camelCase JSON；应用级设置 + 数据库注册表）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub schema_version: u32,
    pub onboarding_completed: bool,
    /// 数据库注册表（2026-10-09 多数据库修正）：每个条目 = 一个独立数据库
    ///（SQLite/thumbs/向量同居 db_dir）；照片库登记表（photos_libraries）
    /// 在各数据库内部，不同数据库的照片库天然隔离。经 `database_*` 命令族
    /// 维护，settings_set 对该字段原样保留后端真值。
    pub databases: Vec<DatabaseEntry>,
    /// 激活数据库 id（注册表内恰好一个；None=尚未创建任何数据库，一切
    /// 依赖数据库的命令返回「尚未创建数据库」）。
    pub active_database_id: Option<String>,
    pub import: ImportSettings,
    pub gallery: GallerySettings,
    pub appearance: AppearanceSettings,
    pub ai: AiSettings,
    pub system: SystemSettings,
    /// 存储与缓存策略（M8-③：缩略图缓存 LRU 上限等）。
    pub storage: StorageSettings,
    /// 监视文件夹（F4：后台轮询发现新文件自动入册；绝对路径形态）。
    /// v1 默认空；旧 settings.json 缺字段由 serde default 容错填充。
    pub watch_folders: Vec<String>,
}

impl Settings {
    /// 激活数据库目录解析（多数据库修正）：取激活 [`DatabaseEntry`] 的
    /// db_dir——library.db / thumbs / 向量等一切数据件所在处；与照片库
    /// root 的互斥校验（§八-6）以此为基准。
    ///
    /// 无激活数据库返回 Err("尚未创建数据库")；注册表内查无该 id（注册表
    /// 被外部手改坏）同样报错拒绝启动库操作。`app_config_dir` 仅为签名
    /// 对称保留（默认 db_dir 约定 = `<app_config_dir>/databases/<id>` 在
    /// `database_create` 落地，解析侧不需要）。
    pub fn active_database_dir(&self, _app_config_dir: &Path) -> Result<PathBuf, String> {
        let id = self
            .active_database_id
            .as_deref()
            .ok_or_else(|| "尚未创建数据库".to_string())?;
        let entry = self
            .databases
            .iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| format!("激活数据库不在注册表内：{id}"))?;
        Ok(PathBuf::from(&entry.db_dir))
    }

    /// 按注册表序取激活条目之后的下一个数据库 id（database_remove 删激活
    /// 库时切换用）；注册表空返回 None。
    pub fn first_database_id(&self) -> Option<&str> {
        self.databases.first().map(|entry| entry.id.as_str())
    }
}

/// 数据库注册表条目（camelCase；与前端 `DatabaseEntry` 契约对齐）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseEntry {
    /// uuid（database_create 生成）。
    pub id: String,
    pub name: String,
    /// 数据件目录（规范化绝对路径；library.db / thumbs / 向量同居于此）。
    pub db_dir: String,
}

/// 两路径「相同或互相包含」（忽略大小写与 `/`\\` 方向；前缀后必须是分隔
/// 符，防 `I:\db` 误匹配 `I:\db2`——与 db::libraries::paths_overlap 同
/// 语义的本模块内副本，settings 不依赖 db 层）。
fn db_paths_overlap(a: &str, b: &str) -> bool {
    fn norm(c: char) -> char {
        if c == '/' {
            '\\'
        } else {
            c.to_ascii_lowercase()
        }
    }
    fn starts_with(path: &str, prefix: &str) -> bool {
        let prefix = prefix.trim_end_matches(['/', '\\']);
        let mut p = path.chars();
        let mut r = prefix.chars();
        loop {
            match (r.next(), p.next()) {
                (Some(rc), Some(pc)) if norm(rc) == norm(pc) => continue,
                (None, Some(pc)) => return pc == '/' || pc == '\\',
                (None, None) => return true,
                _ => return false,
            }
        }
    }
    starts_with(a, b) || starts_with(b, a)
}

/// 数据库目录注册表内互斥校验（settings 层闸门，database_create 前置）：
/// 新 db_dir 不得与任一已注册数据库的 db_dir 相同或互相包含（数据件
/// 目录混同会让两个注册表条目指向同一份 SQLite，切库形同虚设）。
/// **不含**与照片库 root 的重叠校验——照片库 root 在各数据库的
/// photos_libraries 表里，需要开库查，由 IPC 层（`crate::ipc::databases`）
/// 完成；本函数只做注册表内互斥。
pub fn validate_database_dir_overlap(
    databases: &[DatabaseEntry],
    db_dir: &Path,
) -> Result<(), String> {
    let new_dir = db_dir.to_string_lossy().into_owned();
    for entry in databases {
        if db_paths_overlap(&new_dir, &entry.db_dir) {
            return Err(format!(
                "数据库位置与已有数据库「{}」相同或互相包含（{}）",
                entry.name, entry.db_dir
            ));
        }
    }
    Ok(())
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            onboarding_completed: false,
            databases: Vec::new(),
            active_database_id: None,
            import: ImportSettings::default(),
            gallery: GallerySettings::default(),
            appearance: AppearanceSettings::default(),
            ai: AiSettings::default(),
            system: SystemSettings::default(),
            storage: StorageSettings::default(),
            watch_folders: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AppearanceSettings {
    pub theme: ColorTheme,
    pub animations: bool,
}

impl Default for AppearanceSettings {
    fn default() -> Self {
        Self {
            theme: ColorTheme::Dark,
            animations: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct GallerySettings {
    pub merge_raw_jpg: bool,
}

impl Default for GallerySettings {
    fn default() -> Self {
        Self {
            merge_raw_jpg: true,
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

/// 设备与导入配置。目录布局不在其列——落盘布局固定为纯时间
/// `{照片库}/{拍摄年}/{拍摄月}/{原文件名}`（2026-10-09 定案 §三，不可
/// 配置）；旧 settings.json 的 `import.dirTemplate` 等残留键 serde 自动忽略。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportSettings {
    pub prompt_on_device: bool,
    pub skip_imported: bool,
    pub duplicate_policy: DuplicatePolicy,
}

impl Default for ImportSettings {
    fn default() -> Self {
        Self {
            prompt_on_device: true,
            skip_imported: true,
            duplicate_policy: DuplicatePolicy::Skip,
        }
    }
}

/// AI 能力与资源限制配置（应用级全局；旧库级 aiQualityTier 随库注册表退役）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AiSettings {
    pub enable_clip: bool,
    pub enable_face: bool,
    pub enable_scene_tags: bool,
    pub index_schedule: IndexSchedule,
    pub cpu_limit_percent: u32,
    pub use_gpu: bool,
    /// AI 索引画质档位（2026-09-28 三档画质）："fast"（SCRFD 10G 小检测
    /// 模型）|"normal"（默认，现件）|"accurate"（语义 fp16 + 检测源 2048
    /// 优先）。切档经参数指纹自动重建受影响通道（fast↔normal 只重建
    /// face；normal↔accurate 重建 face+semantic）。非法值加载时兜成
    /// "normal"，settings_set 拒绝写入。
    #[serde(default = "default_quality_tier")]
    pub quality_tier: String,
    /// 语义检索相似度阈值（cos，0..1）：低于该分的结果过滤；0 = 不过滤；
    /// null = **auto**——按当前语义模型变体取默认（int8 = 0.09，
    /// 2026-09-21 标定；fp16 见 ai::semantic_default_min_score）。
    /// SigLIP2 cos 分数区间压缩（实测无关内容 top≈0.087，相关簇 ≈0.099+），
    /// 过高全灭、过低「进哪个相册都是全部照片」。老配置显式 0.09（= 旧
    /// 默认）加载时迁移为 null（auto）；非 0.09 的自定义值保留。
    #[serde(default)]
    pub semantic_min_score: Option<f64>,
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
    /// 疑似失焦阈值（0-100 归一清晰度分，默认 30）：低于判 soft（「疑似
    /// 失焦」建议标签，绝不自动定罪——运动模糊/浅景深天然误报）。改了经
    /// selection 指纹比对重排 blur 任务。
    pub blur_soft_threshold: f32,
    /// 闭眼判定 EAR 阈（MediaPipe 468 点 EAR，越小越闭；默认 0.13）。
    /// 低于判 closed（有人闭眼）。**真库标定（2026-09-28，方法同语义阈值
    /// 轮）**：497 张主库全量跑 eyes 通道（facemesh-ear-v1，335 张检出
    /// 人脸），逐脸双眼 min EAR 分布 p10=0.089 / p25=0.145 / p50=0.241；
    /// 对最低 12 张与 0.10-0.22 边界带逐张肉眼核验——EAR ≤ 0.13 眼睑环
    /// 贴合（真闭眼/眨眼瞬间），0.13-0.20 半睁/眯眼，≥ 0.22 确定睁眼。
    /// 0.13 取「闭眼簇上沿」并略保守（覆盖 19.4% 检出脸资产——家庭库
    /// 连拍/群像占比所致）；0.13-0.16 区间眯眼样本归 maybe 不硬判。
    pub eyes_ear_closed: f32,
    /// 疑似闭眼 EAR 上阈（默认 0.20）：[closed, maybe) 判 maybe。0.20 =
    /// 睁眼主体带下沿（0.19-0.20 仍见眯眼/单眼窄样本，0.219 起确认全睁；
    /// 标定同上）。改任一 EAR 阈值经 selection 指纹重排 eyes 任务
    /// （分析结果随阈值变）。
    pub eyes_ear_maybe: f32,
    /// Include intentional single-eye closure in review suggestions (default off).
    pub eyes_include_single: bool,
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
            quality_tier: default_quality_tier(),
            semantic_min_score: None,
            embed_input_size: 256,
            face_detect_threshold: 0.5,
            face_cluster_threshold: 0.4,
            index_params_version: 1,
            burst_gap_ms: 2000,
            burst_hamming_max: 10,
            burst_min_size: 2,
            blur_soft_threshold: 30.0,
            eyes_ear_closed: 0.13,
            eyes_ear_maybe: 0.20,
            eyes_include_single: false,
        }
    }
}

/// 画质档位默认值（"normal"）。
fn default_quality_tier() -> String {
    "normal".to_string()
}

/// AI 设置合法性校验（settings_set 前置；返回 Err 的值拒绝落盘）。
/// quality_tier 必须是三档之一——档位驱动模型件选择与指纹重建，脏值
/// 会让推理层与 marker 各自兜底成不一致状态。
pub fn validate_ai_settings(ai: &AiSettings) -> Result<(), String> {
    if matches!(ai.quality_tier.as_str(), "fast" | "normal" | "accurate") {
        Ok(())
    } else {
        Err(format!(
            "非法画质档位: {}（可选 fast / normal / accurate）",
            ai.quality_tier
        ))
    }
}

// ---------------------------------------------------------------------------
// 路径规范化（建库统一闸门）
// ---------------------------------------------------------------------------

/// 照片库根目录规范化（photo_library_create 统一闸门）：
/// 前端提交的 root 字符串曾放过盘符相对路径（真机实证 `I:SmartPhotoedge-photos`
/// ——用户手输少打一个反斜杠），被按进程 CWD 解析后误落他处。规则：
/// ① 必须是绝对路径（`Path::is_absolute()`——Windows 上 `I:xxx` 无根
///    分量因此为 false），否则拒绝；
/// ② 组件级逻辑归一（始终）：正斜杠折成反斜杠、折叠 `.`/`..`，保持绝对
///    形态与用户大小写。**不走 fs::canonicalize**（2026-09-28 修正：会把
///    映射盘 Y:\ 翻成 UNC 形态，导致库内盘符路径前缀匹配失效、且与用户
///    配置形态漂移；绝对性校验已由 ① 保证，canonical 不再必要）。
pub fn normalize_library_path(input: &str) -> Result<String, String> {
    normalize_absolute_path(input, "照片库根目录")
}

/// 数据库目录规范化（database_create 统一闸门，多数据库修正沿用单库时代
/// 规则）：拒绝相对路径（同 `I:xxx` 盘符相对陷阱）与盘根（数据库目录承载
/// 一切数据件，不得与整盘混同——db_dir 互斥校验以它为基准，盘根会与任何
/// 同盘照片库互相包含）。
pub fn normalize_database_path(input: &str) -> Result<String, String> {
    let normalized = normalize_absolute_path(input, "数据库位置")?;
    let path = Path::new(&normalized);
    if path
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return Err(format!(
            "数据库位置不能是磁盘或网络共享的根目录：{normalized}"
        ));
    }
    Ok(normalized)
}

/// 绝对路径规范化闸门内核（照片库 root / 数据库目录共用，规则见
/// [`normalize_library_path`]；`label` 进错误文案）。
fn normalize_absolute_path(input: &str, label: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("{label}必须是绝对路径：{input}"));
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(format!("{label}必须是绝对路径：{trimmed}"));
    }
    Ok(logical_normalize(path).to_string_lossy().into_owned())
}

/// 组件级逻辑归一（目标路径尚不存在时的兜底）：`/` 分隔符经 components
/// 重建自然折成 `\`；`.` 直接跳过；`..` 仅在上一段是普通目录名时折叠
///（避免把盘根/UNC 段 pop 掉退化成盘符相对路径）。
fn logical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::{Component, PathBuf};
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 系统行为配置。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SystemSettings {
    /// Optional registered database to validate and open automatically on startup.
    pub auto_open_database_id: Option<String>,
    pub launch_at_login: bool,
    pub close_to_tray: bool,
    pub language: String,
}

impl Default for SystemSettings {
    fn default() -> Self {
        Self {
            auto_open_database_id: None,
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
    /// 旧键（libraries/activeLibraryId/单库 databaseDir）自动忽略
    ///（2026-10-09 多数据库修正，不做迁移）。
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
                migrate_legacy_ai_settings(&mut settings);
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

/// AI 节一次性迁移（2026-09-28 三档画质）：
/// - 旧 settings.json 的 `semanticMinScore: 0.09` = 旧默认值（f32 存储
///   时代写死）→ 迁移为 `None`（auto，随语义模型变体自适应）；非 0.09
///   的自定义值保留用户语义。缺字段本就落 None（auto）。
/// - `qualityTier` 非法（手改坏）→ 兜成 "normal"（settings_set 写入侧
///   有硬校验，这里只救读取侧，避免整份配置走损坏分支被备份重置）。
fn migrate_legacy_ai_settings(settings: &mut Settings) {
    if settings
        .ai
        .semantic_min_score
        .is_some_and(|v| (v - 0.09).abs() < 1e-9)
    {
        settings.ai.semantic_min_score = None;
    }
    if validate_ai_settings(&settings.ai).is_err() {
        settings.ai.quality_tier = default_quality_tier();
    }
}
