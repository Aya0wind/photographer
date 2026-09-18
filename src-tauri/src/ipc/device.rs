//! device 命令：`device_list` / `device_scan` / `device_files`。

use tauri::State;

use super::{scan_by_id, AppState, FileEntryDto};
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

/// 列出指定设备的全部媒体文件（导入向导源树/勾选表）。
#[tauri::command]
pub fn device_files(state: State<AppState>, id: String) -> Result<Vec<FileEntryDto>, String> {
    super::files_by_id(&state, &id)
}
