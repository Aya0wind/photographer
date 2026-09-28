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
    pub gallery: GallerySettings,
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
/// 布局属性**不再可配置**（用户定案 2026-09-28）：M2 时代的库级
/// `dir_template` 目录模板与 `import_subdir` 导入子目录已退役——物理布局
/// 固定为 `photoRoot/{创建YYYY}/{创建MM}/{dir_name}/`（相册内平铺，公式
/// 唯一来源 [`crate::db::album_home_rel_parts`]）。旧 settings.json 里的
/// `dirTemplate`/`importSubdir` 残留键 serde 反序列化自动忽略（不升 schema）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Library {
    pub id: String,
    pub name: String,
    pub db_dir: String,
    pub photo_root: String,
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
    /// 库级 AI 档位；None 仅用于迁移旧全局配置。
    pub ai_quality_tier: Option<String>,
}

impl Default for Library {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            db_dir: String::new(),
            photo_root: String::new(),
            configured: false,
            streams: default_streams(),
            ai_quality_tier: None,
        }
    }
}

/// 库级并发流数默认值。
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
            gallery: GallerySettings::default(),
            ai: AiSettings::default(),
            system: SystemSettings::default(),
            storage: StorageSettings::default(),
            watch_folders: Vec::new(),
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

/// ai.qualityTier 是当前库的运行时投影；持久化真值属于每个 Library。
pub fn normalize_library_quality_tiers(
    settings: &mut Settings,
    previous: &Settings,
) -> Result<(), String> {
    for lib in &mut settings.libraries {
        if lib.ai_quality_tier.is_none() {
            let existing = previous.libraries.iter().find(|old| old.id == lib.id);
            lib.ai_quality_tier = Some(
                existing
                    .and_then(|old| old.ai_quality_tier.clone())
                    .unwrap_or_else(|| {
                        if existing.is_some() {
                            previous.ai.quality_tier.clone()
                        } else {
                            default_quality_tier()
                        }
                    }),
            );
        }
        if !matches!(
            lib.ai_quality_tier.as_deref(),
            Some("fast" | "normal" | "accurate")
        ) {
            return Err(format!("库 {} 的 AI 档位无效", lib.name));
        }
    }
    let changed_here = settings.active_library_id == previous.active_library_id
        && settings.ai.quality_tier != previous.ai.quality_tier;
    if let Some(lib) = settings
        .libraries
        .iter_mut()
        .find(|lib| Some(&lib.id) == settings.active_library_id.as_ref())
    {
        if changed_here {
            lib.ai_quality_tier = Some(settings.ai.quality_tier.clone());
        }
        settings.ai.quality_tier = lib
            .ai_quality_tier
            .clone()
            .unwrap_or_else(default_quality_tier);
    }
    Ok(())
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

/// 设备与导入配置。目录布局不在其列——布局固定不可配置（2026-09-28
/// 定案，见 [`Library`] 注释）；旧 settings.json 的 `import.dirTemplate` /
/// `import.importSubdir` 残留键 serde 自动忽略。
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
// 库路径规范化（2026-09-28 边界修复）
// ---------------------------------------------------------------------------

/// 库路径规范化（onboarding/建库统一闸门）：前端提交的 photoRoot/dbDir
/// 字符串曾放过盘符相对路径（真机实证 `I:SmartPhotoedge-photos`——用户
/// 手输少打一个反斜杠），被按进程 CWD 解析后 DB 落到 src-tauri/ 下并触发
/// dev watcher 风暴。规则：
/// ① 必须是绝对路径（`Path::is_absolute()`——Windows 上 `I:xxx` 无根
///    分量因此为 false），否则拒绝；
/// ② 组件级逻辑归一（始终）：正斜杠折成反斜杠、折叠 `.`/`..`，保持绝对
///    形态与用户大小写。**不走 fs::canonicalize**（2026-09-28 修正：会把
///    映射盘 Y:\ 翻成 UNC 形态，导致库内盘符路径前缀匹配失效、且与用户
///    配置形态漂移；绝对性校验已由 ① 保证，canonical 不再必要）。
pub fn normalize_library_path(input: &str) -> Result<String, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("路径必须是绝对路径：{input}"));
    }
    let path = Path::new(trimmed);
    if !path.is_absolute() {
        return Err(format!("路径必须是绝对路径：{trimmed}"));
    }
    Ok(logical_normalize(path).to_string_lossy().into_owned())
}

/// 全部库的 db_dir / photo_root 逐一规范化（settings_set 前置；任一非法
/// 拒绝整次写入，前端提示修正后重提）。既有库路径已是规范形态时幂等。
pub fn normalize_library_paths(settings: &mut Settings) -> Result<(), String> {
    for lib in &mut settings.libraries {
        lib.db_dir = normalize_library_path(&lib.db_dir)?;
        lib.photo_root = normalize_library_path(&lib.photo_root)?;
    }
    Ok(())
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
                migrate_legacy_ai_settings(&mut settings);
                // 旧库首次迁移时各自记住旧档位，之后不再跟随全局投影。
                for lib in &mut settings.libraries {
                    if lib.ai_quality_tier.is_none() {
                        lib.ai_quality_tier = Some(settings.ai.quality_tier.clone());
                    }
                    if !matches!(
                        lib.ai_quality_tier.as_deref(),
                        Some("fast" | "normal" | "accurate")
                    ) {
                        lib.ai_quality_tier = Some(default_quality_tier());
                    }
                }
                if let Some(tier) = settings
                    .active_library()
                    .and_then(|lib| lib.ai_quality_tier.clone())
                {
                    settings.ai.quality_tier = tier;
                }
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
