//! IPC 命令层，按命名空间拆分模块。 AppState + 可测的核心编排函数
//! （命令层只做 tauri 参数/返回值的薄包装）。

pub mod device;
pub mod import;
pub mod settings;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use chrono::SecondsFormat;
use serde::{Deserialize, Serialize};

use crate::db::{Db, JobRow, LogRow};
use crate::devices::orchestrator::{self, DeviceSnapshot};
use crate::devices::{DeviceSource, SourceKind};
use crate::events::EventBus;
use crate::import::engine::{Engine, EngineControls, ImportPlan};
use crate::settings::Settings;

/// 接入设备注册表条目：源 + 最近扫描快照。
pub struct DeviceEntry {
    pub source: Arc<dyn DeviceSource>,
    pub snapshot: DeviceSnapshot,
}

/// 活跃导入（M1 约束：同时只允许一个）。
pub struct ActiveImport {
    pub job_id: i64,
    pub controls: EngineControls,
    pub handle: Option<std::thread::JoinHandle<()>>,
}

/// 全局应用状态：内存中的设置快照 + 配置目录 + 事件总线 +
/// 设备注册表 + 活跃导入。
pub struct AppState {
    pub settings: Mutex<Settings>,
    pub config_dir: PathBuf,
    pub bus: EventBus,
    pub devices: Mutex<HashMap<String, DeviceEntry>>,
    pub active_import: Mutex<Option<ActiveImport>>,
}

/// 设备文件条目 DTO（导入向导源树/勾选表数据；mtime 为 RFC3339 字符串）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntryDto {
    pub id: String,
    pub rel_path: String,
    pub size: u64,
    pub mtime: String,
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
    let devices = state.devices.lock().expect("devices mutex poisoned");
    let entry = devices.get(id).ok_or_else(|| format!("设备 {id} 不在线"))?;
    let files = entry
        .source
        .list()
        .map_err(|e| format!("枚举设备文件失败: {e}"))?;
    Ok(files.iter().map(FileEntryDto::from).collect())
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
    let db = Db::open(&dir.join("library.db")).map_err(|e| format!("打开库失败: {e}"))?;
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
    let snapshot = {
        let devices = state.devices.lock().expect("devices mutex poisoned");
        let entry = devices.get(id).ok_or_else(|| format!("设备 {id} 不在线"))?;
        orchestrator::scan_device(&*entry.source, &db, skip_imported)
            .map_err(|e| format!("扫描设备失败: {e}"))?
    };
    let mut devices = state.devices.lock().expect("devices mutex poisoned");
    if let Some(entry) = devices.get_mut(id) {
        entry.snapshot = snapshot.clone();
    }
    Ok(snapshot)
}

/// 回收已结束的活跃导入线程（Busy 判定前置步骤）。
fn reap_finished(active: &mut Option<ActiveImport>) {
    if let Some(current) = active.as_ref() {
        if current.controls.is_done() {
            let finished = active.take();
            if let Some(job) = finished {
                if let Some(handle) = job.handle {
                    let _ = handle.join();
                }
            }
        }
    }
}

/// 启动导入（新任务）：Busy 检查 → 设备在线检查 → begin → 后台线程 run。
pub fn start_import(state: &AppState, plan: ImportPlan) -> Result<i64, String> {
    let mut active = state
        .active_import
        .lock()
        .expect("active import mutex poisoned");
    reap_finished(&mut active);
    if active.is_some() {
        return Err("已有导入任务进行中（Busy）".into());
    }

    let source = ArcSource(
        state
            .devices
            .lock()
            .expect("devices mutex poisoned")
            .get(&plan.source_id)
            .map(|entry| Arc::clone(&entry.source))
            .ok_or_else(|| format!("设备 {} 不在线", plan.source_id))?,
    );
    let db = active_library_db(state)?;
    let mut engine = Engine::new(db, state.bus.clone(), Box::new(source), plan);
    let job_id = engine.begin().map_err(|e| format!("导入启动失败: {e}"))?;
    let controls = engine.controls();
    let handle = std::thread::Builder::new()
        .name("import-engine".into())
        .spawn(move || {
            engine.run();
        })
        .map_err(|e| format!("启动导入线程失败: {e}"))?;
    *active = Some(ActiveImport {
        job_id,
        controls,
        handle: Some(handle),
    });
    Ok(job_id)
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
    let source = ArcSource(
        state
            .devices
            .lock()
            .expect("devices mutex poisoned")
            .get(&device_id)
            .map(|entry| Arc::clone(&entry.source))
            .ok_or_else(|| format!("设备 {device_id} 不在线，无法恢复导入"))?,
    );
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, job_id)
        .map_err(|e| format!("恢复任务失败: {e}"))?;
    let controls = engine.controls();
    let handle = std::thread::Builder::new()
        .name("import-engine".into())
        .spawn(move || {
            engine.run();
        })
        .map_err(|e| format!("启动导入线程失败: {e}"))?;
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
    let source = ArcSource(
        state
            .devices
            .lock()
            .expect("devices mutex poisoned")
            .get(&device_id)
            .map(|entry| Arc::clone(&entry.source))
            .ok_or_else(|| format!("设备 {device_id} 不在线，无法重试"))?,
    );
    let engine = Engine::resume(db, state.bus.clone(), Box::new(source), plan, new_id)
        .map_err(|e| format!("重试任务失败: {e}"))?;
    let controls = engine.controls();
    let handle = std::thread::Builder::new()
        .name("import-engine".into())
        .spawn(move || {
            engine.run();
        })
        .map_err(|e| format!("启动导入线程失败: {e}"))?;
    *active = Some(ActiveImport {
        job_id: new_id,
        controls,
        handle: Some(handle),
    });
    Ok(new_id)
}
