//! 设备在位状态与媒体扫描分离。系统信号只触发调和；注册表更新和
//! 领域事件在同一把短锁内提交。源 Arc 是连接代次，旧扫描不得覆盖重连。
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{active_library_db, DeviceEntry, DeviceScan, SharedState};
use crate::devices::orchestrator::{scan_device_with_progress, DeviceSnapshot};
use crate::devices::volume::VolumeSource;
use crate::devices::wpd::WpdSource;
use crate::devices::{normalize_device_id, DeviceSource, SourceKind};
use crate::events::AppEvent;

pub fn reconcile_devices(state: &SharedState, trigger: &str) {
    let mut truth: Vec<_> = crate::devices::present::enumerate_present_volumes()
        .into_iter()
        .map(|(id, name)| (id, SourceKind::Volume, name))
        .collect();
    match crate::devices::wpd::enumerate_mtp_devices() {
        Ok(devices) => truth.extend(
            devices
                .into_iter()
                .map(|(id, name)| (id, SourceKind::Mtp, name)),
        ),
        Err(err) => {
            // 枚举失败是未知状态，不能把所有相机当作已拔出。
            eprintln!("reconcile[{trigger}]: WPD 枚举暂不可用: {err}");
            truth.extend(
                state
                    .devices
                    .lock()
                    .expect("devices mutex poisoned")
                    .iter()
                    .filter(|(_, entry)| entry.snapshot.kind == SourceKind::Mtp)
                    .map(|(id, entry)| (id.clone(), SourceKind::Mtp, entry.snapshot.name.clone())),
            );
        }
    }
    reconcile_with_truth(state, trigger, &truth);
}

/// 已确认的系统移除立即使旧连接失效；随后的到达会建立新的源 Arc。
pub fn remove_device(state: &SharedState, id: &str) {
    let id = normalize_device_id(id);
    let mut registry = state.devices.lock().expect("devices mutex poisoned");
    if let Some(entry) = registry.remove(&id) {
        if entry.snapshot.kind == SourceKind::Mtp {
            crate::devices::wpd::invalidate_device(&id);
        }
        state.bus.publish(AppEvent::DeviceRemoved { id });
    }
}

/// 注入的在位枚举与运行时走相同的离线过滤、注册与扫描逻辑。
pub fn reconcile_with_truth(
    state: &SharedState,
    trigger: &str,
    truth: &[(String, SourceKind, String)],
) {
    let offline = crate::devices::health::offline_ids();
    let truth: Vec<_> = truth
        .iter()
        .map(|(id, kind, name)| (normalize_device_id(id), *kind, name.clone()))
        .filter(|(id, _, _)| !offline.contains(id))
        .collect();
    let mut scans = Vec::new();
    {
        let mut registry = state.devices.lock().expect("devices mutex poisoned");
        let removed: Vec<_> = registry
            .iter()
            .filter(|(id, entry)| {
                entry.snapshot.kind != SourceKind::Folder
                    && !truth.iter().any(|(tid, ..)| tid == *id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in removed {
            if let Some(entry) = registry.remove(&id) {
                if entry.snapshot.kind == SourceKind::Mtp {
                    crate::devices::wpd::invalidate_device(&id);
                }
            }
            state.bus.publish(AppEvent::DeviceRemoved { id });
        }
        for (id, kind, name) in truth {
            if kind == SourceKind::Folder {
                continue;
            }
            if let Some(entry) = registry.get_mut(&id) {
                if matches!(&entry.scan, DeviceScan::Failed(_, retry) if Instant::now() >= *retry) {
                    entry.scan = DeviceScan::Scanning;
                    state.bus.publish(AppEvent::DeviceArrived {
                        id: id.clone(),
                        kind,
                        name,
                    });
                    scans.push((id, Arc::clone(&entry.source)));
                }
                continue;
            }
            let source: Arc<dyn DeviceSource> = match kind {
                SourceKind::Volume => Arc::new(VolumeSource::new(format!("{id}\\"))),
                SourceKind::Mtp => Arc::new(WpdSource::new(id.clone(), name.clone())),
                SourceKind::Folder => unreachable!(),
            };
            let snapshot = DeviceSnapshot {
                id: id.clone(),
                name: name.clone(),
                kind,
                files_by_kind: Default::default(),
                bytes_total: 0,
                new_files: 0,
            };
            registry.insert(
                id.clone(),
                DeviceEntry {
                    source: Arc::clone(&source),
                    snapshot,
                    scan: DeviceScan::Scanning,
                },
            );
            state.bus.publish(AppEvent::DeviceArrived {
                id: id.clone(),
                kind,
                name,
            });
            scans.push((id, source));
        }
    }
    for (id, source) in scans {
        spawn_scan(state, id, source, trigger);
    }
}

fn spawn_scan(state: &SharedState, id: String, source: Arc<dyn DeviceSource>, trigger: &str) {
    let supervisor = Arc::clone(&state.supervisor);
    let state = Arc::clone(state);
    let trigger = trigger.to_owned();
    supervisor.spawn("scan", source.name(), move |_| {
        crate::devices::diagnostics::record(format!("scan begin: {id}; opening library"));
        let progress_state = Arc::clone(&state);
        let progress_source = Arc::clone(&source);
        let progress_id = id.clone();
        let on_batch: crate::devices::FileBatchCallback = Arc::new(move |files| {
            let registry = progress_state
                .devices
                .lock()
                .expect("devices mutex poisoned");
            if registry
                .get(&progress_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.source, &progress_source))
            {
                progress_state.bus.publish(AppEvent::DeviceFilesProgress {
                    id: progress_id.clone(),
                    files: files.iter().map(super::FileEntryDto::from).collect(),
                });
            }
        });
        // Panic 也完成扫描状态转换，不能永远占着“扫描中”。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let skip = state
                .settings
                .lock()
                .expect("settings mutex poisoned")
                .import
                .skip_imported;
            // 没有可用的照片库仍能发现相机和扫描媒体；查重暂按全部未导入。
            let db = active_library_db(&state)
                .map_err(|error| {
                    crate::devices::diagnostics::record(format!(
                        "scan library unavailable: {error}"
                    ));
                    error
                })
                .ok();
            crate::devices::diagnostics::record(format!("scan media begin: {id}"));
            scan_device_with_progress(&*source, db.as_ref(), skip, Some(on_batch))
                .map_err(|err| err.to_string())
        }))
        .unwrap_or_else(|_| Err("设备扫描异常，请重试".into()));
        let mut registry = state.devices.lock().expect("devices mutex poisoned");
        let Some(entry) = registry
            .get_mut(&id)
            .filter(|entry| Arc::ptr_eq(&entry.source, &source))
        else {
            return;
        };
        match result {
            Ok(snapshot) => {
                crate::devices::diagnostics::record(format!(
                    "scan complete: {id}; {} files",
                    snapshot.files_by_kind.values().sum::<u64>()
                ));
                entry.snapshot = snapshot.clone();
                entry.scan = DeviceScan::Ready;
                state.bus.publish(AppEvent::DeviceScanned {
                    id,
                    name: snapshot.name.clone(),
                    kind: snapshot.kind,
                    snapshot,
                });
            }
            Err(message) => {
                crate::devices::diagnostics::record(format!(
                    "reconcile[{trigger}]: 扫描失败 {id}: {message}"
                ));
                entry.scan =
                    DeviceScan::Failed(message.clone(), Instant::now() + Duration::from_secs(10));
                state
                    .bus
                    .publish(AppEvent::DeviceScanFailed { id, message });
            }
        }
    });
}
