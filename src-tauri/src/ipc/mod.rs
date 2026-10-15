//! IPC 命令层，按命名空间拆分模块。 AppState + 可测的核心编排函数
//! （命令层只做 tauri 参数/返回值的薄包装）。

pub mod ai;
pub mod album;
pub mod album_export;
pub mod assets;
pub mod claim;
pub mod culling;
pub mod databases;
pub mod device;
pub mod device_manager;
pub mod duplicates;
pub mod import;
pub mod indexing;
pub mod insights;
pub mod map;
pub mod people;
pub mod photo_library;
pub mod rating;
pub mod reconcile;
pub mod selection;
pub mod settings;
mod sidecar;
pub mod system;
pub mod tethering;
pub mod thumb;
pub mod versions;
pub mod watch;

use std::collections::HashMap;
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
    pub files: Option<Vec<crate::devices::FileEntry>>,
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
            files: None,
        }
    }
}

/// 活跃导入（M1 约束：同时只允许一个）。
pub struct ActiveImport {
    pub job_id: i64,
    /// Pinned at launch: changing the selected library must not change source identity.
    pub source_id: String,
    pub source_roots: Vec<PathBuf>,
    pub controls: EngineControls,
    /// supervisor 任务句柄（panic 捕获/命名；软控制走 controls，同构接口）。
    /// 持有以防提前丢弃（TaskHandle drop 不取消任务，仅保语义显式）。
    #[allow(dead_code)]
    pub handle: Option<crate::tasks::TaskHandle>,
}

/// 全局应用状态：内存中的设置快照（应用级 + 数据库注册表）+ 配置目录 +
/// 事件总线 + 设备注册表 + 活跃导入。
///
/// 2026-10-09 多数据库修正：settings.databases 承载数据库注册表，一切
/// 库内操作作用于 `activeDatabaseId` 指向的激活数据库（切换见
/// `ipc::databases`，切换后前后端各自全量刷新数据）。
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
    /// 导入让路闸（用户定案 2026-09-29）：导入任务在场为 true——一切索引
    /// 触发（手动/自动/跟随）拒绝，开场暂停 kind="index" 在跑任务。
    pub import_running: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// 登记单管道闸（§八-5，2026-10-09）：增量扫描发现与导入落盘产出的
    /// 登记段（查重判定 + 写库原子段）互斥串行——防双写竞争与重复算哈希/
    /// 缩略图。导入引擎（launch_import 注入）与库扫描（photo_library
    /// worker）持同一把锁。
    pub register_gate: std::sync::Arc<std::sync::Mutex<()>>,
    /// 库扫描 worker 唤醒旗（M4b）：「从文件夹建立」落批量登记任务后置位，
    /// 轮询线程秒级打断 60s 等待立即拾取（消费者 swap 复位）。
    pub library_scan_kick: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// AI 模型下载管理器（models 目录固定在 app 配置目录下）。
    pub ai: crate::ai::ModelManager,
}

/// 导入让路检查：导入任务在场时一切索引触发（手动/自动）拒绝。
pub fn ensure_no_import_running(state: &AppState) -> Result<(), String> {
    if state
        .import_running
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        Err("导入进行中：索引已让路暂停，导入完成后自动恢复/触发".into())
    } else {
        Ok(())
    }
}

/// Stop an active import when reconciliation proves its source disappeared.
/// The journal remains resumable, unlike an explicit user cancellation.
pub fn mark_import_device_unavailable(state: &AppState, device_id: &str) {
    let Ok(active) = state.active_import.lock() else {
        return;
    };
    let Some(current) = active.as_ref() else {
        return;
    };
    if current.controls.is_done() || current.controls.is_cancelled() {
        return;
    }
    let source_id = crate::devices::normalize_device_id(&current.source_id);
    let device_id = crate::devices::normalize_device_id(device_id);
    let mut roots = current.source_roots.clone();
    if let Some(folder) = source_id.strip_prefix(FOLDER_ID_PREFIX) { roots.push(folder.into()); }
    let matches_source = source_id == device_id || roots.iter()
        .any(|root| source_path_on_volume(root, &device_id));
    if !matches_source {
        return;
    }
    if current.controls.mark_device_lost() {
        state
            .bus
            .publish(crate::events::AppEvent::DeviceUnavailable {
                id: current.source_id.clone(),
            });
        state.bus.publish(crate::events::AppEvent::ImportPaused {
            job_id: current.job_id,
        });
    }
}

/// Compare normalized path components, including Windows `E:` mount IDs.
fn source_path_on_volume(path: &Path, volume: &str) -> bool {
    let normalize = |value: &str| value.replace('\\', "/").trim_start_matches("//?/")
        .trim_end_matches('/').to_string();
    let mut path = normalize(&path.to_string_lossy());
    let mut volume = normalize(volume);
    if volume.as_bytes().get(1) == Some(&b':') {
        path.make_ascii_lowercase();
        volume.make_ascii_lowercase();
    } else if !volume.starts_with('/') { return false; }
    !volume.is_empty() && (path == volume || path.starts_with(&format!("{volume}/")))
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
    /// 是否允许继续展开。目录内容按需读取；这里不预读每个子目录，避免
    /// 一个包含大量目录的父级产生 N+1 次磁盘或网络访问。
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
/// 的命令用本壳；其他命令
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

/// 文件系统目录浏览（M2“从文件夹导入”向导，LR 风格树懒加载）：
/// parent=None → 平台适配提供的文件系统根；
/// parent=Some → 该目录下的一层子目录。任何读取失败 → 空数组
/// （前端按空 children 处理，不报错）。
pub fn list_dirs(parent: Option<&str>) -> Vec<DirEntryDto> {
    match parent {
        None => list_root_dirs().unwrap_or_default(),
        Some(path) => child_dirs(path),
    }
}

/// 根枚举保留平台错误；旧的纯浏览入口仍兼容失败返回空集合。
pub(crate) fn list_root_dirs() -> Result<Vec<DirEntryDto>, String> {
    crate::platform::drive_roots()
        .map_err(|e| e.to_string())
        .map(|roots| {
            roots
                .into_iter()
                .map(|root| DirEntryDto {
                    name: root.name,
                    path: root.path,
                    has_subdirs: true,
                })
                .collect()
        })
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
        if !is_browsable_dir_name(&name) || crate::platform::is_hidden_or_system(&entry) {
            continue;
        }
        // entry.path() = root.join(name)：与父路径同形态的绝对路径
        let path = entry.path();
        out.push(DirEntryDto {
            // 只枚举当前层，子目录是否为空由下一次展开确认。
            has_subdirs: true,
            name,
            path: path.to_string_lossy().into_owned(),
        });
    }
    out.sort_by_cached_key(|e| e.name.to_lowercase());
    out
}

/// 目录名可浏览：非点前缀且不在黑名单。
fn is_browsable_dir_name(name: &str) -> bool {
    !name.starts_with('.') && !DIR_BLACKLIST.contains(&name)
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
///（生产侧已被 files_by_id_with_progress 取代；device_lifecycle_test 仍用
/// ——待测试迁移后删除，2026-10-12 标注。）
#[allow(dead_code)]
pub fn files_by_id(state: &AppState, id: &str) -> Result<Vec<FileEntryDto>, String> {
    let key = crate::devices::normalize_device_id(id);
    let source = {
        let devices = state.devices.lock().expect("devices mutex poisoned");
        let entry = devices.get(&key).ok_or_else(|| format!("设备 {id} 不在线"))?;
        if let Some(files) = &entry.files {
            return Ok(files.iter().map(FileEntryDto::from).collect());
        }
        Arc::clone(&entry.source)
    };
    let files = source
        .list_names()
        .map_err(|e| format!("枚举设备文件失败: {e}"))?;
    let mut registry = state.devices.lock().expect("devices mutex poisoned");
    let entry = registry.get_mut(&key).filter(|entry| Arc::ptr_eq(&entry.source, &source))
        .ok_or_else(|| format!("设备 {id} 已断开或重新连接"))?;
    entry.files = Some(files.clone());
    Ok(files.iter().map(FileEntryDto::from).collect())
}

/// 事件链路自检：向总线发布一条 Probe 事件（经转发器 emit 到前端
pub fn files_by_id_with_progress(state: &SharedState, id: &str) -> Result<Vec<FileEntryDto>, String> {
    let key = crate::devices::normalize_device_id(id);
    let source = {
        let mut registry = state.devices.lock().unwrap();
        let entry = registry.get_mut(&key).ok_or("设备已断开")?;
        if let Some(files) = &entry.files { return Ok(files.iter().map(FileEntryDto::from).collect()); }
        entry.files = Some(Vec::new());
        entry.scan = DeviceScan::Scanning;
        let source = Arc::clone(&entry.source);
        state.bus.publish(crate::events::AppEvent::DeviceArrived { id: key.clone(), name: source.name(), kind: source.kind() });
        source
    };
    let weak = Arc::downgrade(state);
    let expected = Arc::clone(&source);
    let progress_key = key.clone();
    let callback: crate::devices::FileBatchCallback = Arc::new(move |batch| {
        let Some(state) = weak.upgrade() else { return; };
        let mut registry = state.devices.lock().unwrap();
        if let Some(entry) = registry.get_mut(&progress_key).filter(|entry| Arc::ptr_eq(&entry.source, &expected)) {
            entry.files.get_or_insert_with(Vec::new).extend(batch.iter().cloned());
            state.bus.publish(crate::events::AppEvent::DeviceFilesProgress { id: progress_key.clone(), files: batch.iter().map(FileEntryDto::from).collect() });
        }
    });
    let started = std::time::Instant::now();
    let result = source.list_names_with_progress(callback).map_err(|error| error.to_string());
    let mut registry = state.devices.lock().unwrap();
    let entry = registry.get_mut(&key).filter(|entry| Arc::ptr_eq(&entry.source, &source)).ok_or("设备已重新连接")?;
    match result {
        Ok(files) => {
            crate::devices::diagnostics::record(format!("device names complete: {key}; {} files; elapsed={:?}", files.len(), started.elapsed()));
            entry.snapshot.files_by_kind.clear();
            for file in &files {
                *entry.snapshot.files_by_kind.entry(crate::devices::classify(&file.rel_path, &[])).or_insert(0) += 1;
            }
            entry.files = Some(files.clone());
            entry.scan = DeviceScan::Ready;
            state.bus.publish(crate::events::AppEvent::DeviceScanned { id: key, name: entry.snapshot.name.clone(), kind: entry.snapshot.kind, snapshot: entry.snapshot.clone() });
            Ok(files.iter().map(FileEntryDto::from).collect())
        }
        Err(message) => {
            entry.files = None;
            entry.scan = DeviceScan::Failed(message.clone(), std::time::Instant::now() + std::time::Duration::from_secs(10));
            state.bus.publish(crate::events::AppEvent::DeviceScanFailed { id: key, message: message.clone() });
            Err(message)
        }
    }
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
    fn copy_only(&self) -> bool {
        self.0.copy_only()
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
    fn local_path(&self, id: &str) -> Option<std::path::PathBuf> {
        self.0.local_path(id)
    }
    fn local_roots(&self) -> Vec<std::path::PathBuf> {
        self.0.local_roots()
    }
    fn delete(&self, id: &str) -> crate::devices::DeviceResult<()> {
        self.0.delete(id)
    }
}

/// 激活数据库目录（多数据库修正，2026-10-09）：settings 注册表按
/// `activeDatabaseId` 解析——一切库内操作（照片库登记/资产/相册）都作用
/// 于该目录下的独立 SQLite；尚未创建数据库时 Err（调用方转成明确 UI 态）。
pub fn app_database_dir(state: &AppState) -> Result<PathBuf, String> {
    let settings = state.settings.lock().expect("settings mutex poisoned");
    settings.active_database_dir(&state.config_dir)
}

/// 激活数据库连接（多数据库修正，2026-10-09）：library.db + thumbs/ +
/// 向量等一切数据件所在目录即 [`app_database_dir`]；照片库登记
/// （photos_libraries）、资产、相册等一切表同居于此，切换数据库即整体
/// 换库（不同数据库的照片库天然隔离）。打开即建 schema（单版本幂等）。
pub fn app_database_db(state: &AppState) -> Result<Db, String> {
    open_library_db(&app_database_dir(state)?)
}

/// 打开指定目录的数据库连接（library.db；打开即幂等执行单版本建表脚本，
/// 新库并发首开由 IF NOT EXISTS + busy_timeout 收敛，无迁移锁）。
pub fn open_library_db(db_dir: &Path) -> Result<Db, String> {
    let db = Db::open(&db_dir.join("library.db")).map_err(|e| format!("打开数据库失败: {e}"))?;
    Ok(db)
}

/// 资产行删除后的派生数据清理（删照片库 / 回收站清除 / 重复删除 / 清理
/// 源缺失共用）：缩略图缓存文件（按源路径跨档位清全部 mtime 代）+ 语义
/// 向量（usearch 按 asset id 删）+ 人脸簇心缓存失效（2026-10-09 审计 P1：
/// 四路共用本入口，簇缓存按 dbDir 整库失效——被删脸的嵌入若残留在簇心
/// 池，后续新脸会继续归入已无人照片的簇；重启成本可接受，不做精确剔除）。
/// 行已删，清理全程缺失容忍——失败只留盘上孤儿缓存/向量，不构成错误。
/// 返回 (清掉的缓存文件数, 清掉的向量数)。
pub fn purge_asset_derivations(
    db_dir: &Path,
    asset_ids: &[i64],
    paths: &[&str],
) -> (u64, u64) {
    let mut thumbs = 0u64;
    for path in paths {
        thumbs += crate::thumbs::remove_src_cache(db_dir, std::path::Path::new(path));
    }
    let vectors = crate::ai::semantic::remove_vectors(db_dir, asset_ids);
    crate::ai::face::invalidate_cluster_cache(db_dir);
    (thumbs, vectors)
}

/// 扫描指定设备并刷新注册表快照（热插拔编排线程 / device_scan 共用）。
pub fn scan_by_id(state: &AppState, id: &str) -> Result<DeviceSnapshot, String> {
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
        entry.files = None;
        Arc::clone(&entry.source)
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        orchestrator::scan_device_with_progress(&*source, None, false, None)
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
    entry.files = None;
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
    scan_local_source(state, source)
}

pub fn scan_local_paths(state: &AppState, paths: &[String]) -> Result<DeviceSnapshot, String> {
    if let [path] = paths {
        if Path::new(path).is_dir() { return scan_folder(state, path); }
    }
    let source = crate::devices::selection::LocalPathsSource::new(paths).map_err(|e| e.to_string())?;
    scan_local_source(state, Arc::new(source))
}

fn scan_local_source(state: &AppState, source: Arc<dyn DeviceSource>) -> Result<DeviceSnapshot, String> {
    let id = source.id();
    state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .insert(
            id.clone(),
            DeviceEntry {
                scan: DeviceScan::Ready,
                files: None,
                source: Arc::clone(&source),
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
            let mut devices = state.devices.lock().expect("devices mutex poisoned");
            if devices.get(&id).is_some_and(|entry| Arc::ptr_eq(&entry.source, &source)) {
                devices.remove(&id);
            }
            Err(err)
        }
    }
}

/// 导入用源：注册表取出；FOLDER 源重建为“枚举排除目标子树”的实例
/// （嵌套守卫已拒绝合法嵌套方案，此处是防御层——避免任何路径形态
/// 下把自己的产出再枚举进来）。文件夹已消失时回退注册表源（由引擎报错）。
fn import_source(
    state: &AppState,
    device_id: &str,
    library_root: &std::path::Path,
) -> Result<ArcSource, String> {
    if let Some(root) = device_id.strip_prefix(FOLDER_ID_PREFIX) {
        let folder_source = LocalFolderSource::with_exclude(root, Some(library_root))
            .map_err(|error| error.to_string())?;
        return Ok(ArcSource(Arc::new(folder_source)));
    }
    let registered = state.devices.lock().expect("devices mutex poisoned")
        .get(device_id).map(|entry| Arc::clone(&entry.source))
        .ok_or_else(|| format!("设备 {device_id} 不在线"))?;
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

/// 启动、恢复和重试共用调度；索引在导入结束后按需一次性唤醒。
fn launch_import(state: &AppState, mut engine: Engine, job_id: i64) -> Result<ActiveImport, String> {
    // 登记单管道闸（§八-5）：引擎登记段与库扫描互斥（run 前 spawn 注入点）
    engine.set_registration_gate(std::sync::Arc::clone(&state.register_gate));
    let controls = engine.controls();
    let source_id = engine.source_id();
    let source_roots = engine.source_roots();
    let (index_db_dir, geo_config_dir, ai_settings) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        // app_database_dir 内部会再锁 settings——此处持锁，直接同式解析。
        // 调用方（start/resume/retry）此前都已开过激活数据库；这里 Err 只在
        // 「校验后瞬间被切换数据库」的竞态窗口出现，如实上抛。
        (
            settings.active_database_dir(&state.config_dir)?,
            state.config_dir.clone(),
            settings.ai.clone(),
        )
    };
    // §八-1 导入侧重绑的缩略图缓存键迁移基准（应用数据库目录）
    engine.set_db_dir(index_db_dir.clone());
    let index_supervisor = Arc::clone(&state.supervisor);
    let ai_manager = state.ai.clone();
    let ai_bus = state.bus.clone();
    let enable_clip = ai_settings.enable_clip;
    let enable_face = ai_settings.enable_face;
    let enable_selection = ai_settings.enable_selection;
    let burst_params = crate::bursts::BurstParams::from_settings(&ai_settings);
    let gate = std::sync::Arc::clone(&state.import_running);
    // 闸在派发前同步置位：调用方拿到 job_id 的瞬间，手动索引触发即被拒
    // （闭包内才置位有窗口）。闭包 Drop 兜底放行（含 panic 路径）。进程级
    // 镜像旗（index::kick 让路守卫读点）与上面同点翻转——kick 无 state
    // 访问，经它判导入在场（联拍 kick 穿透让路闸的问题由此收口）。
    gate.store(true, std::sync::atomic::Ordering::SeqCst);
    crate::index::set_import_in_flight(true);
    let handle = state
        .supervisor
        .spawn("import", format!("job-{job_id}"), move |_| {
            // Import retains admission priority; committed photos remain indexable.
            struct ImportGate(std::sync::Arc<std::sync::atomic::AtomicBool>);
            impl Drop for ImportGate {
                fn drop(&mut self) {
                    self.0.store(false, std::sync::atomic::Ordering::SeqCst);
                    // 进程级镜像旗同步放行（与上面置位同点，见 launch_import 注释）
                    crate::index::set_import_in_flight(false);
                }
            }
            let _gate = ImportGate(gate);
            crate::index::scheduler::configure(&index_db_dir,&ai_manager,&ai_bus,&ai_settings);
            crate::index::scheduler::kick(index_db_dir.clone(), &index_supervisor);
            let stats = engine.run();
            // 先放行闸再恢复（M1 单导入串行：下一个导入必在本任务收尾后
            // 才能起跑，不存在互相踩暂停/恢复的交错）。
            drop(_gate);
            // 索引/AI/连拍收尾统一在导入结束后一次性触发，且仅在本轮确有
            // 已落盘文件（done_files>0；全跳过的重复导入不打扰）。取消/
            // 设备失联的会话同样放行——已落盘文件的任务必须被唤醒（journal
            // 行已终结，index 任务只写 index_tasks/assets 派生列，不与收尾
            // 收割竞争）。索引天然增量：worker 只领取 pending 任务行。
            if stats.done_files > 0 {
                crate::index::scheduler::notify(&index_db_dir);
                // 拍摄地图增量回填（2026-09-30）：新照片 GPS 归属随导入完成
                // 自动入图——指纹不变走 bincode 快路径 + asset_regions 增量
                crate::geo::install::ensure_installed(
                    geo_config_dir.clone(),
                    index_db_dir.clone(),
                    std::sync::Arc::new(ai_bus.clone()),
                    &index_supervisor,
                );
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
                        index_db_dir.clone(),
                        &ai_manager,
                        &ai_bus,
                        &index_supervisor,
                    );
                }
                // 闭眼回填（eyes 任务随导入建档；模型未装 kick 内部早退，0021；
                // AI 关闭时任务不存在也不消费，2026-10-12。blur 消费池已由
                // 上面的 pipeline::kick(ALL) 唤醒）
                if enable_selection {
                    crate::ai::selection::kick_eyes_if_ready(
                        index_db_dir.clone(),
                        &ai_manager,
                        &ai_bus,
                        &index_supervisor,
                    );
                }
                // 连拍重组：索引 worker（含 phash 通道）跑完后按当前参数重组
                crate::bursts::regroup_kick(index_db_dir, burst_params, &ai_bus, &index_supervisor);
            }
        });
    Ok(ActiveImport {
        job_id,
        source_id,
        source_roots,
        controls,
        handle: Some(handle),
    })
}

/// 启动导入（新任务）：Busy 检查 → 设备在线检查 → begin → 后台线程 run。
/// 2026-10-09 §三：导入目标 = 照片库（plan.target_library_id → 库 root）；
/// album_id 可选（纯逻辑引用，相册/子组不影响落盘布局）。
pub fn start_import(state: &AppState, plan: ImportPlan) -> Result<i64, String> {
    if plan.target_library_id.trim().is_empty() {
        return Err("导入计划缺少目标照片库（targetLibraryId）".into());
    }
    {
        let db = app_database_db(state)?;
        if let Some(album_id) = plan.album_id {
            if !db.album_exists(album_id).map_err(|e| e.to_string())? {
                return Err(format!("导入相册 {album_id} 不存在"));
            }
        }
    }
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }

    let source = import_source(state, &plan.source_id, &library_root_of(state, &plan)?)?;
    let db = app_database_db(state)?;
    let mut engine = Engine::new(db, state.bus.clone(), Box::new(source), plan);
    let job_id = engine.begin().map_err(|e| format!("导入启动失败: {e}"))?;
    *active = Some(launch_import(state, engine, job_id)?);
    Ok(job_id)
}

/// 计划目标照片库的 root 解析（import_source 的排除子树基准）。
fn library_root_of(state: &AppState, plan: &ImportPlan) -> Result<PathBuf, String> {
    let db = app_database_db(state)?;
    let library = db
        .photos_library_get(&plan.target_library_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("目标照片库不存在：{}", plan.target_library_id))?;
    Ok(PathBuf::from(library.root_path))
}

/// 从 journal 恢复导入（设备失联暂停后重连 / 显式恢复无活跃任务的任务）。
pub fn resume_import(state: &AppState, job_id: i64) -> Result<(), String> {
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if let Some(current) = active.as_ref() {
        if current.job_id == job_id {
            if current.controls.is_cancelled() || current.controls.is_device_lost() {
                return Err("导入正在收尾，请等待暂停完成后重试恢复".into());
            }
            current.controls.resume();
            state
                .bus
                .publish(crate::events::AppEvent::ImportResumed { job_id });
            return Ok(());
        }
        return Err("任务 ID 与当前活跃导入不一致".into());
    }

    let db = app_database_db(state)?;
    let (device_id, plan) = load_job_plan(&db, job_id)?;
    let source = restore_import_source(state, &db, &device_id, &library_root_of(state, &plan)?, job_id)?;
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, job_id)
        .map_err(|e| format!("恢复任务失败: {e}"))?;
    *active = Some(launch_import(state, engine, job_id)?);
    Ok(())
}

/// Reconstruct remembered local selections after restart; source listing stays
/// inside the worker so a disconnected source remains a paused, resumable job.
fn restore_import_source(state: &AppState, db: &Db, device_id: &str, library_root: &Path, job_id: i64) -> Result<ArcSource, String> {
    if !device_id.starts_with("FILES:") || state.devices.lock().expect("devices mutex poisoned").contains_key(device_id) {
        return import_source(state, device_id, library_root);
    }
    let mut stmt = db.0.prepare("SELECT path FROM import_source_roots WHERE job_id = ?1 ORDER BY path")
        .map_err(|e| e.to_string())?;
    let paths = stmt.query_map([job_id], |row| row.get::<_, String>(0)).map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    let paths: Vec<_> = paths.into_iter().filter(|path| Path::new(path).exists()).collect();
    let source = crate::devices::selection::LocalPathsSource::new(&paths).map_err(|e| e.to_string())?;
    Ok(ArcSource(Arc::new(source)))
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
    if active.as_ref().is_none_or(|current| current.job_id != job_id || current.controls.is_done()) {
        let db = app_database_db(state)?;
        let changed = db.0.execute(
            "UPDATE jobs SET status = 'cancelled', finished_at = ?2 WHERE id = ?1 AND kind = 'import' AND status = 'paused'",
            rusqlite::params![job_id, chrono::Utc::now().to_rfc3339()],
        ).map_err(|e| e.to_string())?;
        if changed == 1 {
            state.bus.publish(crate::events::AppEvent::ImportCancelled { job_id });
            return Ok(());
        }
    }
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
    let db = app_database_db(state)?;
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
    let db = app_database_db(state)?;
    db.logs_page(job_id, after_id, limit.clamp(1, 500))
        .map_err(|e| e.to_string())
}

/// 失败重试：失败行复制为 pending 新任务并立即执行。
pub fn retry_failed(state: &AppState, job_id: i64) -> Result<i64, String> {
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }
    let db = app_database_db(state)?;
    let new_id = db
        .retry_failed_into_new_job(job_id)
        .map_err(|e| e.to_string())?
        .ok_or("该任务没有可重试的失败文件")?;
    drop(db);

    let db = app_database_db(state)?;
    let (device_id, plan) = load_job_plan(&db, new_id)?;
    let source = restore_import_source(state, &db, &device_id, &library_root_of(state, &plan)?, new_id)?;
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, new_id)
        .map_err(|e| format!("重试任务失败: {e}"))?;
    *active = Some(launch_import(state, engine, new_id)?);
    Ok(new_id)
}

// ---------------------------------------------------------------------------
// M2 F1：安全清卡（候选列表 / 复验删除）
// ---------------------------------------------------------------------------

/// 清卡任务的设备源（注册表直取；离线报错）。
fn clean_source(state: &AppState, job_id: i64) -> Result<(Db, Arc<dyn DeviceSource>), String> {
    let db = app_database_db(state)?;
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
