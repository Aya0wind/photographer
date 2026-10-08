//! device 命令：`device_list` / `device_scan` / `device_files` /
//! `folder_scan` / `fs_list_dirs`。
//!
//! 铁律（2026-09-18）：涉及磁盘 IO / WPD COM / 网络(NAS) 的命令一律
//! async + spawn_blocking 后台执行（同步命令跑主线程会冻结窗口）；
//! `device_list` 还查询空读卡器的介质状态，同样放到后台。

use tauri::State;

use super::{run_blocking, scan_by_id, DirEntryDto, FileEntryDto, SharedState};
use crate::devices::orchestrator::DeviceSnapshot;
use crate::events::{AssetKind, SourceKind};
use serde::Serialize;

/// 设备快照 + 在线状态（device_list 返回；注册表内设备恒在线）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshotInfo {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    pub files_by_kind: std::collections::BTreeMap<AssetKind, u64>,
    pub bytes_total: u64,
    pub new_files: u64,
    pub connected: bool,
    pub media_present: bool,
    pub scan_status: &'static str,
    pub scan_error: Option<String>,
}

impl From<&DeviceSnapshot> for DeviceSnapshotInfo {
    fn from(snapshot: &DeviceSnapshot) -> Self {
        Self {
            id: snapshot.id.clone(),
            name: snapshot.name.clone(),
            kind: snapshot.kind,
            files_by_kind: snapshot.files_by_kind.clone(),
            bytes_total: snapshot.bytes_total,
            new_files: snapshot.new_files,
            connected: true,
            media_present: true,
            scan_status: "ready",
            scan_error: None,
        }
    }
}

/// 当前接入设备与空读卡器；磁盘探测在后台执行，且不持有注册表锁。
#[tauri::command]
pub async fn device_list(state: State<'_, SharedState>) -> Result<Vec<DeviceSnapshotInfo>, String> {
    run_blocking(state.inner().clone(), |state| {
        let empty_readers = if crate::platform::capabilities().volume_devices {
            crate::devices::present::enumerate_empty_readers().map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        let devices = state.devices.lock().expect("devices mutex poisoned");
        let mut result: Vec<_> = devices
            .values()
            .map(|entry| {
                let mut info = DeviceSnapshotInfo::from(&entry.snapshot);
                match &entry.scan {
                    super::DeviceScan::Scanning => info.scan_status = "scanning",
                    super::DeviceScan::Ready => {}
                    super::DeviceScan::Failed(message, _) => {
                        info.scan_status = "failed";
                        info.scan_error = Some(message.clone());
                    }
                }
                info
            })
            .filter(|info| !empty_readers.contains(&info.id))
            .collect();
        for id in empty_readers {
            result.push(DeviceSnapshotInfo {
                name: format!("读卡器 ({id})"),
                id,
                kind: SourceKind::Volume,
                files_by_kind: Default::default(),
                bytes_total: 0,
                new_files: 0,
                connected: true,
                media_present: false,
                scan_status: "ready",
                scan_error: None,
            });
        }
        result.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(result)
    })
    .await
}

/// 扫描指定设备（刷新统计与 new_files），返回快照。
/// 设备枚举（WPD 秒级/大目录分钟级）在后台线程执行。
#[tauri::command]
pub async fn device_scan(
    state: State<'_, SharedState>,
    id: String,
) -> Result<DeviceSnapshot, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| scan_by_id(state, &id)).await
}

/// 元数据查询在后台进行；克隆源后释放设备锁，避免系统探测阻塞设备事件。
#[tauri::command]
pub async fn device_copy_only(state: State<'_, SharedState>, id: String) -> Result<bool, String> {
    run_blocking(state.inner().clone(), move |state| {
        let key = crate::devices::normalize_device_id(&id);
        let source = state.devices.lock().expect("devices mutex poisoned")
            .get(&key).map(|entry| std::sync::Arc::clone(&entry.source))
            .ok_or_else(|| "来源已断开，请重新选择".to_string())?;
        Ok(source.copy_only())
    }).await
}

/// 注册并扫描本地文件夹源（M2“从文件夹导入”），返回快照。
/// NAS 大目录扫描分钟级，后台线程执行。
/// 前端随后用 device_files(id) / import_start(plan) 走与设备相同的管线。
#[tauri::command]
pub async fn folder_scan(
    state: State<'_, SharedState>,
    path: String,
) -> Result<DeviceSnapshot, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| super::scan_folder(state, &path)).await
}

/// 列出指定设备的全部媒体文件（导入向导源树/勾选表）。
/// WPD/大目录枚举秒级以上，后台线程执行（真机 1161 文件清单即此路径）。
#[tauri::command]
pub async fn device_files(
    state: State<'_, SharedState>,
    id: String,
) -> Result<Vec<FileEntryDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| super::files_by_id(state, &id)).await
}

/// 文件系统目录树浏览（M2 导入向导源面板，LR 风格懒加载）：
/// parent=None → 盘符根；Some(path) → 一层子目录。
/// 磁盘/NAS IO，后台线程执行；根枚举不支持/失败返回明确错误。
#[tauri::command]
pub async fn fs_list_dirs(parent: Option<String>) -> Result<Vec<DirEntryDto>, String> {
    tauri::async_runtime::spawn_blocking(move || match parent {
        None => super::list_root_dirs(),
        Some(path) => Ok(super::list_dirs(Some(&path))),
    })
    .await
    .map_err(|e| format!("目录枚举后台任务失败: {e}"))?
}

/// 事件链路自检（同步、立即）：发布 Probe{ts} → 转发器 emit → 前端
/// `app://event`。返回 ts 供对账；前端收到即整链通（纯内存发布，豁免
/// 异步铁律）。
#[tauri::command]
pub fn event_ping(state: State<SharedState>) -> String {
    super::event_ping(&state.bus)
}

/// 平台能力快照：不支持的功能不应呈现为“没有设备”。
#[tauri::command]
pub fn platform_capabilities() -> crate::platform::PlatformCapabilities {
    crate::platform::capabilities()
}
