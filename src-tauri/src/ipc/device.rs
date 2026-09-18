//! device 命令：`device_list` / `device_scan` / `device_files`。

use tauri::State;

use super::{scan_by_id, AppState, DirEntryDto, FileEntryDto};
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
        }
    }
}

/// 当前接入设备（注册表缓存，含最近扫描快照）。
#[tauri::command]
pub fn device_list(state: State<AppState>) -> Vec<DeviceSnapshotInfo> {
    let devices = state.devices.lock().expect("devices mutex poisoned");
    devices
        .values()
        .map(|entry| DeviceSnapshotInfo::from(&entry.snapshot))
        .collect()
}

/// 扫描指定设备（刷新统计与 new_files），返回快照。
#[tauri::command]
pub fn device_scan(state: State<AppState>, id: String) -> Result<DeviceSnapshot, String> {
    let snapshot = scan_by_id(&state, &id)?;
    state.bus.publish(crate::events::AppEvent::DeviceScanned {
        id: snapshot.id.clone(),
        name: snapshot.name.clone(),
        kind: snapshot.kind,
        snapshot: snapshot.clone(),
    });
    Ok(snapshot)
}

/// 注册并扫描本地文件夹源（M2“从文件夹导入”），返回快照。
/// 前端随后用 device_files(id) / import_start(plan) 走与设备相同的管线。
#[tauri::command]
pub fn folder_scan(state: State<AppState>, path: String) -> Result<DeviceSnapshot, String> {
    super::scan_folder(&state, &path)
}

/// 列出指定设备的全部媒体文件（导入向导源树/勾选表）。
#[tauri::command]
pub fn device_files(state: State<AppState>, id: String) -> Result<Vec<FileEntryDto>, String> {
    super::files_by_id(&state, &id)
}

/// 文件系统目录树浏览（M2 导入向导源面板，LR 风格懒加载）：
/// parent=None → 盘符根；Some(path) → 一层子目录。读取失败返回空数组。
#[tauri::command]
pub fn fs_list_dirs(parent: Option<String>) -> Vec<DirEntryDto> {
    super::list_dirs(parent.as_deref())
}
