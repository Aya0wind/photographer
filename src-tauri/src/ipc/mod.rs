//! IPC 命令层，按命名空间拆分模块。 AppState + 可测的核心编排函数
//! （命令层只做 tauri 参数/返回值的薄包装）。

pub mod ai;
pub mod assets;
pub mod device;
pub mod device_manager;
pub mod import;
pub mod indexing;
pub mod migrate;
pub mod people;
pub mod reconcile;
pub mod settings;
pub mod thumb;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::db::{Db, JobRow, LogRow};
use crate::devices::folder::{LocalFolderSource, FOLDER_ID_PREFIX};
use crate::devices::orchestrator::{self, DeviceSnapshot};
use crate::devices::{DeviceSource, SourceKind};
use crate::events::EventBus;
use crate::import::clean::{CleanCandidateDto, CleanResultDto};
use crate::import::engine::{Engine, EngineControls, ImportPlan};
use crate::settings::Settings;
use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};

/// 接入设备注册表条目：源 + 最近扫描快照。
pub struct DeviceEntry {
    pub source: Arc<dyn DeviceSource>,
    pub snapshot: DeviceSnapshot,
    pub scan: DeviceScan,
}

#[derive(Default)]
pub enum DeviceScan {
    Scanning,
    #[default]
    Ready,
    Failed(String, std::time::Instant),
}

impl DeviceEntry {
    #[allow(dead_code)] // 测试脚手架构建已扫描的源
    pub fn ready(source: Arc<dyn DeviceSource>, snapshot: DeviceSnapshot) -> Self {
        Self {
            source,
            snapshot,
            scan: DeviceScan::Ready,
        }
    }
}

/// 活跃导入（M1 约束：同时只允许一个）。
pub struct ActiveImport {
    pub job_id: i64,
    pub controls: EngineControls,
    /// supervisor 任务句柄（panic 捕获/命名；软控制走 controls，同构接口）。
    /// 持有以防提前丢弃（TaskHandle drop 不取消任务，仅保语义显式）。
    #[allow(dead_code)]
    pub handle: Option<crate::tasks::TaskHandle>,
}

/// 全局应用状态：内存中的设置快照 + 配置目录 + 事件总线 +
/// 设备注册表 + 活跃导入。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub config_dir: PathBuf,
    pub bus: EventBus,
    pub devices: Mutex<HashMap<String, DeviceEntry>>,
    pub active_import: Mutex<Option<ActiveImport>>,
    /// 统一后台任务框架（扫描/导入等长活线程的派发/panic 捕获/命名）。
    pub supervisor: std::sync::Arc<crate::tasks::TaskSupervisor>,
    /// 按需缩略图生成队列（asset_thumb_get(asset_id) 未命中路径）。
    pub thumb_queue: thumb::ThumbQueue,
    /// 目录迁移守卫（库 id 集合）：迁移期间该库拒绝新导入/新迁移。
    pub migrations: Mutex<HashSet<String>>,
    /// AI 模型下载管理器（models 目录固定在 app 配置目录下）。
    pub ai: crate::ai::ModelManager,
}

/// 活跃库迁移守卫检查：迁移中返回 Err（导入/迁移入口共用）。
pub fn ensure_library_not_migrating(state: &AppState) -> Result<(), String> {
    let library_id = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .map(|lib| lib.id.clone());
    let Some(library_id) = library_id else {
        return Ok(());
    };
    if state
        .migrations
        .lock()
        .expect("migrations mutex poisoned")
        .contains(&library_id)
    {
        return Err(format!("库 {library_id} 迁移中，拒绝导入"));
    }
    Ok(())
}

/// 设备文件条目 DTO：唯一定义在 events 层（事件载荷与 IPC 共享），此处重导出
/// 保持 `ipc::FileEntryDto` 路径兼容（含 tests/common 的 #[path] 包含场景）。
pub use crate::events::FileEntryDto;

/// 目录树浏览条目（fs_list_dirs 返回；M2 导入向导源面板懒加载）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntryDto {
    pub name: String,
    /// 规范化的绝对路径。
    pub path: String,
    /// 是否含可浏览子目录（浅探测：找到第一个即 true）。
    pub has_subdirs: bool,
}

/// 目录浏览黑名单（与设备源枚举的系统目录一致）。
const DIR_BLACKLIST: &[&str] = &["$RECYCLE.BIN", "System Volume Information"];

/// 托管共享态：`Arc<AppState>`（'static 可跨线程 clone 进 spawn_blocking
/// 闭包；lib.rs `manage(SharedState)`，async 慢命令壳的载体）。
pub type SharedState = std::sync::Arc<AppState>;

/// 慢命令统一壳（铁律：磁盘 IO / WPD COM / 网络(SMB/NAS) / 大结果集 DB
/// 查询 / 哈希计算绝不上主线程——Tauri 同步命令跑在主线程，会冻结事件循环）。
///
/// 核心同步逻辑保持原签名（集成测试直测）；async 命令壳从托管态 clone
/// `SharedState` 后把工作丢 `spawn_blocking` 后台线程执行。返回 `Result`
/// 的命令用本壳；非 `Result` 命令（如 fs_list_dirs 的"失败→空数组"契约）
/// 就地 spawn。本函数不依赖 tauri 运行时外壳（AppHandle/mock app），
/// 集成测试可直接 await。
pub async fn run_blocking<T: Send + 'static>(
    state: SharedState,
    work: impl FnOnce(&AppState) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = work(&state);
        if let Err(error) = &result {
            crate::devices::diagnostics::record(format!("IPC command failed: {error}"));
        }
        result
    })
    .await
    .map_err(|e| format!("后台任务失败: {e}"))?
}

/// Windows 文件属性位。
#[cfg(windows)]
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
#[cfg(windows)]
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

/// 文件系统目录浏览（M2“从文件夹导入”向导，LR 风格树懒加载）：
/// parent=None → 盘符根（'A'..='Z' 逐个探测 `X:\` 存在）；
/// parent=Some → 该目录下的一层子目录。任何读取失败 → 空数组
/// （前端按空 children 处理，不报错）。
pub fn list_dirs(parent: Option<&str>) -> Vec<DirEntryDto> {
    match parent {
        None => drive_roots(),
        Some(path) => child_dirs(path),
    }
}

/// 盘符根列表（不引第三方依赖：26 个字母逐一探测存在性）。
fn drive_roots() -> Vec<DirEntryDto> {
    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let letter = letter as char;
        let root = format!("{letter}:\\");
        let path = PathBuf::from(&root);
        if path.exists() {
            out.push(DirEntryDto {
                name: format!("{letter}:"),
                path: root,
                has_subdirs: has_any_subdir(&path),
            });
        }
    }
    out
}

/// 列一层子目录（跳过隐藏/系统属性、黑名单与点前缀名；不含文件）。
/// 条目 path = 父路径**原样**拼接，不做 canonicalize：映射盘符（`Y:\`）
/// 会被 canonicalize 解析成 `\\?\UNC\server\share\`，剥前缀后产出残缺的
/// `UNC\...` 路径（2026-09-18 线上 bug），且 NAS 上逐目录 canonicalize
/// 是每目录一次网络往返。父路径相对时仅做一次当前目录拼接。
fn child_dirs(parent: &str) -> Vec<DirEntryDto> {
    let given = Path::new(parent);
    let root = if given.is_absolute() {
        given.to_path_buf()
    } else {
        let Ok(cwd) = std::env::current_dir() else {
            return Vec::new();
        };
        cwd.join(given)
    };
    let Ok(read) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in read.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_browsable_dir_name(&name) || is_hidden_or_system(&entry) {
            continue;
        }
        // entry.path() = root.join(name)：与父路径同形态的绝对路径
        let path = entry.path();
        out.push(DirEntryDto {
            has_subdirs: has_any_subdir(&path),
            name,
            path: path.to_string_lossy().into_owned(),
        });
    }
    out.sort_by_key(|e| e.name.to_lowercase());
    out
}

/// 目录名可浏览：非点前缀且不在黑名单。
fn is_browsable_dir_name(name: &str) -> bool {
    !name.starts_with('.') && !DIR_BLACKLIST.contains(&name)
}

/// 隐藏(0x2)/系统(0x4)属性目录不展示；元数据不可得按可见处理。
#[cfg(windows)]
fn is_hidden_or_system(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    entry
        .metadata()
        .map(|meta| {
            let attrs = meta.file_attributes();
            attrs & FILE_ATTRIBUTE_HIDDEN != 0 || attrs & FILE_ATTRIBUTE_SYSTEM != 0
        })
        .unwrap_or(false)
}

#[cfg(not(windows))]
fn is_hidden_or_system(_entry: &std::fs::DirEntry) -> bool {
    false
}

/// 浅探测是否含可浏览子目录（权限不足/空 → false）。
fn has_any_subdir(path: &Path) -> bool {
    let Ok(read) = std::fs::read_dir(path) else {
        return false;
    };
    for entry in read.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_browsable_dir_name(&name) || is_hidden_or_system(&entry) {
            continue;
        }
        return true;
    }
    false
}

impl From<&crate::devices::FileEntry> for FileEntryDto {
    fn from(entry: &crate::devices::FileEntry) -> Self {
        Self {
            id: entry.id.clone(),
            rel_path: entry.rel_path.clone(),
            size: entry.size,
            mtime: entry.mtime.to_rfc3339_opts(SecondsFormat::Millis, true),
        }
    }
}

/// 列出指定设备的全部媒体文件（透传 DeviceSource::list；MTP 源较慢属预期）。
pub fn files_by_id(state: &AppState, id: &str) -> Result<Vec<FileEntryDto>, String> {
    let key = crate::devices::normalize_device_id(id);
    let source = {
        let devices = state.devices.lock().expect("devices mutex poisoned");
        Arc::clone(
            &devices
                .get(&key)
                .ok_or_else(|| format!("设备 {id} 不在线"))?
                .source,
        )
    };
    let files = source
        .list()
        .map_err(|e| format!("枚举设备文件失败: {e}"))?;
    if !state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .get(&key)
        .is_some_and(|entry| Arc::ptr_eq(&entry.source, &source))
    {
        return Err(format!("设备 {id} 已断开或重新连接"));
    }
    Ok(files.iter().map(FileEntryDto::from).collect())
}

/// 事件链路自检：向总线发布一条 Probe 事件（经转发器 emit 到前端
/// `app://event`），返回发布的时间戳供前端对账。前端能收到即
/// bus→转发器→emit 整链通（诊断设备事件不达的定位手段）。
pub fn event_ping(bus: &EventBus) -> String {
    use crate::events::AppEvent;
    let ts = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    bus.publish(AppEvent::Probe { ts: ts.clone() });
    ts
}

/// 设备是否已注册（到达幂等判定：id 过规范化后查注册表——同一 WPD 设备
/// 大小写两种到达形式命中同一条目；枚举空窗期的重复到达按"已在库"忽略，
/// 不误报"未找到"也不动既有条目）。
#[allow(dead_code)] // 集成测试引用（lib 内调用点已由调和器取代）
pub fn device_registered(state: &AppState, id: &str) -> bool {
    let key = crate::devices::normalize_device_id(id);
    state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .contains_key(&key)
}

/// `Arc<dyn DeviceSource>` → `Box<dyn DeviceSource>` 适配（引擎签名收 Box）。
pub struct ArcSource(pub Arc<dyn DeviceSource>);

impl DeviceSource for ArcSource {
    fn id(&self) -> String {
        self.0.id()
    }
    fn kind(&self) -> SourceKind {
        self.0.kind()
    }
    fn name(&self) -> String {
        self.0.name()
    }
    fn list(&self) -> crate::devices::DeviceResult<Vec<crate::devices::FileEntry>> {
        self.0.list()
    }
    fn open_head(&self, id: &str, max: u64) -> crate::devices::DeviceResult<Vec<u8>> {
        self.0.open_head(id, max)
    }
    fn stream(&self, id: &str) -> crate::devices::DeviceResult<Box<dyn std::io::Read + Send>> {
        self.0.stream(id)
    }
    fn delete(&self, id: &str) -> crate::devices::DeviceResult<()> {
        self.0.delete(id)
    }
}

/// 当前激活库的 Db 连接（打开 + migrate 幂等）。无库 → Err。
pub fn active_library_db(state: &AppState) -> Result<Db, String> {
    let settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    let library = settings.active_library().ok_or("尚未创建库")?;
    let dir = PathBuf::from(&library.db_dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("库目录不可用: {e}"))?;
    open_library_db(&dir)
}

/// 打开指定 dbDir 的库连接（library.db + 迁移幂等）。
pub fn open_library_db(db_dir: &Path) -> Result<Db, String> {
    std::fs::create_dir_all(db_dir).map_err(|e| format!("库目录不可用: {e}"))?;
    let db = Db::open(&db_dir.join("library.db")).map_err(|e| format!("打开库失败: {e}"))?;
    db.migrate().map_err(|e| format!("库迁移失败: {e}"))?;
    Ok(db)
}

/// 扫描指定设备并刷新注册表快照（热插拔编排线程 / device_scan 共用）。
pub fn scan_by_id(state: &AppState, id: &str) -> Result<DeviceSnapshot, String> {
    let skip_imported = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .import
        .skip_imported;
    let db = active_library_db(state)?;
    let key = crate::devices::normalize_device_id(id);
    let source = {
        let mut devices = state.devices.lock().expect("devices mutex poisoned");
        let entry = devices
            .get_mut(&key)
            .ok_or_else(|| format!("设备 {id} 不在线"))?;
        if matches!(entry.scan, DeviceScan::Scanning) {
            return Err("设备正在扫描，请稍后重试".into());
        }
        entry.scan = DeviceScan::Scanning;
        Arc::clone(&entry.source)
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        orchestrator::scan_device(&*source, &db, skip_imported)
            .map_err(|e| format!("扫描设备失败: {e}"))
    }))
    .unwrap_or_else(|_| Err("设备扫描异常，请重试".into()));
    let mut devices = state.devices.lock().expect("devices mutex poisoned");
    let entry = devices
        .get_mut(&key)
        .filter(|entry| Arc::ptr_eq(&entry.source, &source))
        .ok_or_else(|| format!("设备 {id} 已断开或重新连接"))?;
    let snapshot = match result {
        Ok(snapshot) => snapshot,
        Err(message) => {
            entry.scan = DeviceScan::Failed(
                message.clone(),
                std::time::Instant::now() + std::time::Duration::from_secs(10),
            );
            state
                .bus
                .publish(crate::events::AppEvent::DeviceScanFailed {
                    id: key,
                    message: message.clone(),
                });
            return Err(message);
        }
    };
    entry.snapshot = snapshot.clone();
    entry.scan = DeviceScan::Ready;
    if snapshot.kind != SourceKind::Folder {
        state.bus.publish(crate::events::AppEvent::DeviceScanned {
            id: snapshot.id.clone(),
            name: snapshot.name.clone(),
            kind: snapshot.kind,
            snapshot: snapshot.clone(),
        });
    }
    Ok(snapshot)
}

/// 注册并扫描本地文件夹源（M2“从文件夹导入”，spec §5.11）：id 规范化为
/// `FOLDER:<canonical 绝对路径>`，重复扫描同一路径幂等覆盖注册表条目。
/// 前端随后用现有 device_files(id) / import_start(plan) 走同一管线。
pub fn scan_folder(state: &AppState, path: &str) -> Result<DeviceSnapshot, String> {
    let source: Arc<dyn DeviceSource> =
        Arc::new(LocalFolderSource::new(path).map_err(|e| format!("文件夹不可用: {e}"))?);
    let id = source.id();
    state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .insert(
            id.clone(),
            DeviceEntry {
                scan: DeviceScan::Ready,
                source,
                snapshot: DeviceSnapshot {
                    id: id.clone(),
                    name: String::new(),
                    kind: SourceKind::Folder,
                    files_by_kind: Default::default(),
                    bytes_total: 0,
                    new_files: 0,
                },
            },
        );
    match scan_by_id(state, &id) {
        Ok(snapshot) => Ok(snapshot),
        Err(err) => {
            // 扫描失败不留死条目
            state
                .devices
                .lock()
                .expect("devices mutex poisoned")
                .remove(&id);
            Err(err)
        }
    }
}

/// 导入用源：注册表取出；FOLDER 源重建为“枚举排除 target_root 子树”的
/// 实例（嵌套守卫已拒绝合法嵌套方案，此处是防御层——避免任何路径形态
/// 下把自己的产出再枚举进来）。文件夹已消失时回退注册表源（由引擎报错）。
fn import_source(
    state: &AppState,
    device_id: &str,
    target_root: &std::path::Path,
) -> Result<ArcSource, String> {
    let registered = state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .get(device_id)
        .map(|entry| Arc::clone(&entry.source))
        .ok_or_else(|| format!("设备 {device_id} 不在线"))?;
    if let Some(root) = device_id.strip_prefix(FOLDER_ID_PREFIX) {
        if let Ok(folder_source) = LocalFolderSource::with_exclude(root, Some(target_root)) {
            return Ok(ArcSource(Arc::new(folder_source)));
        }
    }
    Ok(ArcSource(registered))
}

/// 回收已结束的活跃导入线程（Busy 判定前置步骤）。
fn reap_finished(active: &mut Option<ActiveImport>) {
    if let Some(current) = active.as_ref() {
        if current.controls.is_done() {
            // TaskHandle detach（线程由 supervisor 管理；panic 已被捕获上报）
            active.take();
        }
    }
}

/// 启动导入（新任务）：Busy 检查 → 设备在线检查 → begin → 后台线程 run。
pub fn start_import(state: &AppState, plan: ImportPlan) -> Result<i64, String> {
    ensure_library_not_migrating(state)?;
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }

    let source = import_source(state, &plan.source_id, &plan.target_root)?;
    let db = active_library_db(state)?;
    let mut engine = Engine::new(db, state.bus.clone(), Box::new(source), plan);
    let job_id = engine.begin().map_err(|e| format!("导入启动失败: {e}"))?;
    let controls = engine.controls();
    // 索引任务钩子：engine.run 收尾后全核跑待办（job 不等它）
    let index_db_dir = PathBuf::from(
        &state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .expect("库已在")
            .db_dir,
    );
    let index_supervisor = std::sync::Arc::clone(&state.supervisor);
    let ai_manager = state.ai.clone();
    let ai_bus = state.bus.clone();
    let enable_clip = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_clip;
    let enable_face = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_face;
    let handle = state
        .supervisor
        .spawn("import", format!("job-{job_id}"), move |_| {
            engine.run();
            crate::index::kick(index_db_dir.clone(), &index_supervisor);
            if enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    index_db_dir.clone(),
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
            if enable_face {
                crate::ai::face::kick_face_if_ready(
                    index_db_dir,
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
        });
    *active = Some(ActiveImport {
        job_id,
        controls,
        handle: Some(handle),
    });
    Ok(job_id)
}

/// 从 journal 恢复导入（设备失联暂停后重连 / 显式恢复无活跃任务的任务）。
pub fn resume_import(state: &AppState, job_id: i64) -> Result<(), String> {
    ensure_library_not_migrating(state)?;
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if let Some(current) = active.as_ref() {
        if current.job_id == job_id {
            current.controls.resume();
            state
                .bus
                .publish(crate::events::AppEvent::ImportResumed { job_id });
            return Ok(());
        }
        return Err("任务 ID 与当前活跃导入不一致".into());
    }
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }

    let db = active_library_db(state)?;
    let (device_id, plan) = load_job_plan(&db, job_id)?;
    let source = import_source(state, &device_id, &plan.target_root)?;
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, job_id)
        .map_err(|e| format!("恢复任务失败: {e}"))?;
    let controls = engine.controls();
    let index_db_dir = PathBuf::from(
        &state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .expect("库已在")
            .db_dir,
    );
    let index_supervisor = std::sync::Arc::clone(&state.supervisor);
    let ai_manager = state.ai.clone();
    let ai_bus = state.bus.clone();
    let enable_clip = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_clip;
    let enable_face = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_face;
    let handle = state
        .supervisor
        .spawn("import", format!("job-{job_id}"), move |_| {
            engine.run();
            crate::index::kick(index_db_dir.clone(), &index_supervisor);
            if enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    index_db_dir.clone(),
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
            if enable_face {
                crate::ai::face::kick_face_if_ready(
                    index_db_dir,
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
        });
    *active = Some(ActiveImport {
        job_id,
        controls,
        handle: Some(handle),
    });
    state
        .bus
        .publish(crate::events::AppEvent::ImportResumed { job_id });
    Ok(())
}

/// 读取任务的 (device_id, plan)。
fn load_job_plan(db: &Db, job_id: i64) -> Result<(String, ImportPlan), String> {
    let (device_id, _) = db
        .job_device(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("任务不存在")?;
    let plan_json = db
        .job_plan_json(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("任务缺少导入计划")?;
    let plan = serde_json::from_str(&plan_json).map_err(|e| format!("计划损坏: {e}"))?;
    Ok((device_id, plan))
}

/// 暂停/取消活跃导入（软信号 + 领域事件）。
pub fn set_import_paused(state: &AppState, job_id: i64, paused: bool) -> Result<(), String> {
    let active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    let current = active.as_ref().ok_or("没有进行中的导入任务")?;
    if current.job_id != job_id {
        return Err("任务 ID 与当前活跃导入不一致".into());
    }
    if paused {
        current.controls.pause();
        state
            .bus
            .publish(crate::events::AppEvent::ImportPaused { job_id });
    } else {
        current.controls.resume();
        state
            .bus
            .publish(crate::events::AppEvent::ImportResumed { job_id });
    }
    Ok(())
}

pub fn cancel_import(state: &AppState, job_id: i64) -> Result<(), String> {
    let active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    let current = active.as_ref().ok_or("没有进行中的导入任务")?;
    if current.job_id != job_id {
        return Err("任务 ID 与当前活跃导入不一致".into());
    }
    current.controls.cancel();
    state
        .bus
        .publish(crate::events::AppEvent::ImportCancelled { job_id });
    Ok(())
}

/// 任务列表（keyset 分页）。
pub fn jobs_page(state: &AppState, after: i64, limit: u32) -> Result<Vec<JobRow>, String> {
    let db = active_library_db(state)?;
    db.jobs_page(after, limit.clamp(1, 200))
        .map_err(|e| e.to_string())
}

/// 任务日志（游标分页）。
pub fn logs_page(
    state: &AppState,
    job_id: i64,
    after_id: i64,
    limit: u32,
) -> Result<Vec<LogRow>, String> {
    let db = active_library_db(state)?;
    db.logs_page(job_id, after_id, limit.clamp(1, 500))
        .map_err(|e| e.to_string())
}

/// 失败重试：失败行复制为 pending 新任务并立即执行。
pub fn retry_failed(state: &AppState, job_id: i64) -> Result<i64, String> {
    ensure_library_not_migrating(state)?;
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }
    let db = active_library_db(state)?;
    let new_id = db
        .retry_failed_into_new_job(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("该任务没有可重试的失败文件")?;
    drop(db);

    let db = active_library_db(state)?;
    let (device_id, plan) = load_job_plan(&db, new_id)?;
    let source = import_source(state, &device_id, &plan.target_root)?;
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, new_id)
        .map_err(|e| format!("重试任务失败: {e}"))?;
    let controls = engine.controls();
    let index_db_dir = PathBuf::from(
        &state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .expect("库已在")
            .db_dir,
    );
    let index_supervisor = std::sync::Arc::clone(&state.supervisor);
    let ai_manager = state.ai.clone();
    let ai_bus = state.bus.clone();
    let enable_clip = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_clip;
    let enable_face = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .ai
        .enable_face;
    let handle = state
        .supervisor
        .spawn("import", format!("job-{new_id}"), move |_| {
            engine.run();
            crate::index::kick(index_db_dir.clone(), &index_supervisor);
            if enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    index_db_dir.clone(),
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
            if enable_face {
                crate::ai::face::kick_face_if_ready(
                    index_db_dir,
                    &ai_manager,
                    &ai_bus,
                    &index_supervisor,
                );
            }
        });
    *active = Some(ActiveImport {
        job_id: new_id,
        controls,
        handle: Some(handle),
    });
    Ok(new_id)
}

// ---------------------------------------------------------------------------
// M2 F1：安全清卡（候选列表 / 复验删除）
// ---------------------------------------------------------------------------

/// 清卡任务的设备源（注册表直取；离线报错）。
fn clean_source(state: &AppState, job_id: i64) -> Result<(Db, Arc<dyn DeviceSource>), String> {
    let db = active_library_db(state)?;
    let (device_id, _) = db
        .job_device(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("任务不存在")?;
    let source = state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .get(&device_id)
        .map(|entry| Arc::clone(&entry.source))
        .ok_or_else(|| format!("设备 {device_id} 不在线"))?;
    Ok((db, source))
}

/// 清卡候选列表（已校验入册 + 源仍在设备）。
pub fn list_clean_candidates(
    state: &AppState,
    job_id: i64,
) -> Result<Vec<CleanCandidateDto>, String> {
    let (db, source) = clean_source(state, job_id)?;
    crate::import::clean::clean_candidates(&db, &*source, job_id)
}

/// 执行清卡：逐文件复验（size+xxh64 对比 journal 指纹）一致才删。
pub fn apply_clean(state: &AppState, job_id: i64) -> Result<CleanResultDto, String> {
    let (db, source) = clean_source(state, job_id)?;
    crate::import::clean::clean_apply(&db, &state.bus, &*source, job_id)
}
