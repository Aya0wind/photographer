//! device 命令：`device_list` / `device_scan` / `device_files` /
//! `folder_scan` / `fs_list_dirs`。
//!
//! 铁律（2026-09-18）：涉及磁盘 IO / WPD COM / 网络(NAS) 的命令一律
//! async + spawn_blocking 后台执行（同步命令跑主线程会冻结窗口）；
//! `device_list` 只读内存注册表快照，保留同步。

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
            scan_status: "ready",
            scan_error: None,
        }
    }
}

/// 当前接入设备（注册表缓存，含最近扫描快照）。纯内存读，保留同步。
#[tauri::command]
pub fn device_list(state: State<SharedState>) -> Vec<DeviceSnapshotInfo> {
    let devices = state.devices.lock().expect("devices mutex poisoned");
    devices
        .values()
        .map(|entry| {
            let mut info = DeviceSnapshotInfo::from(&entry.snapshot);
            match &entry.scan {
                super::DeviceScan::Scanning => info.scan_status = "scanning",
                super::DeviceScan::Ready => {},
                super::DeviceScan::Failed(message, _) => {
                    info.scan_status = "failed";
                    info.scan_error = Some(message.clone());
                }
            }
            info
        })
        .collect()
}

/// 扫描指定设备（刷新统计与 new_files），返回快照。
/// 设备枚举（WPD 秒级/大目录分钟级）在后台线程执行。
#[tauri::command]
pub async fn device_scan(
    state: State<'_, SharedState>,
    id: String,
) -> Result<DeviceSnapshot, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        let snapshot = scan_by_id(state, &id)?;
        state.bus.publish(crate::events::AppEvent::DeviceScanned {
            id: snapshot.id.clone(),
            name: snapshot.name.clone(),
            kind: snapshot.kind,
            snapshot: snapshot.clone(),
        });
        Ok(snapshot)
    })
    .await
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
/// 磁盘/NAS IO，后台线程执行；读取失败（含后台 join 失败）返回空数组，
/// 返回类型不变（前端按空 children 处理，不报错）。
#[tauri::command]
pub async fn fs_list_dirs(parent: Option<String>) -> Vec<DirEntryDto> {
    tauri::async_runtime::spawn_blocking(move || super::list_dirs(parent.as_deref()))
        .await
        .unwrap_or_default()
}

/// 事件链路自检（同步、立即）：发布 Probe{ts} → 转发器 emit → 前端
/// `app://event`。返回 ts 供对账；前端收到即整链通（纯内存发布，豁免
/// 异步铁律）。
#[tauri::command]
pub fn event_ping(state: State<SharedState>) -> String {
    super::event_ping(&state.bus)
}
