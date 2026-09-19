//! 设备状态调和器（Reconciler）语义测试：真值→增/删、幂等重复触发、
//! 大小写变体同设备、健康离线剔除/恢复经真值生效、启动真值调和烟测。
//! 用注入真值 `reconcile_with_truth` 保证确定性（真实真值依赖本机设备）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs};

use std::sync::Arc;
use std::time::Duration;

use common::{build_many, state_with_library};
use devices::SourceKind;
use events::AppEvent;
use ipc::reconcile::reconcile_with_truth;

/// 等 registry 达到谓词（上限 5s；扫描经 supervisor 异步）。
fn eventually_registry(state: &ipc::AppState, cond: impl Fn(&str) -> bool) -> bool {
    let started = std::time::Instant::now();
    loop {
        let hit = state
            .devices
            .lock()
            .unwrap()
            .iter()
            .any(|(id, entry)| cond(id) && matches!(entry.scan, ipc::DeviceScan::Ready));
        if hit {
            return true;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// 收集总线上的 DeviceRemoved（订阅-快照式）。
fn drain_removed(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::DeviceRemoved { id } = ev {
            out.push(id);
        }
    }
    out
}

#[test]
fn truth_adds_missing_device_via_scan_and_emits_scanned() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 3);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    // 清空预注册的卷源（真实启动状态是空注册表）
    state.devices.lock().unwrap().clear();
    let mut rx = state.bus.subscribe();

    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());
    reconcile_with_truth(
        &state,
        "test",
        &[(volume_id.clone(), SourceKind::Volume, "测试卡".into())],
    );

    // 扫描异步完成 → 注册 + DeviceScanned
    assert!(
        eventually_registry(&state, |id| id == volume_id),
        "真值设备应被扫描注册"
    );
    let mut scanned = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::DeviceScanned { id, snapshot, .. } = ev {
            assert_eq!(devices::normalize_device_id(&id), volume_id);
            assert_eq!(snapshot.new_files, 3, "扫描结果入快照");
            scanned = true;
        }
    }
    assert!(scanned, "必须发布 DeviceScanned（前端弹窗依赖）");
}

#[test]
fn truth_removal_drops_device_and_emits_removed() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let mut rx = state.bus.subscribe();
    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());

    // 预注册（state_with_library 已注册该卷源）→ 真值空 → 摘除
    reconcile_with_truth(&state, "test", &[]);
    assert!(
        state
            .devices
            .lock()
            .unwrap()
            .keys()
            .all(|id| id != &volume_id),
        "不在真值的设备应被摘除"
    );
    let removed = drain_removed(&mut rx);
    assert!(
        removed
            .iter()
            .any(|id| devices::normalize_device_id(id) == volume_id),
        "摘除必须发 DeviceRemoved（前端摘 UI 依赖）: {removed:?}"
    );
}

#[test]
fn repeated_reconcile_with_same_truth_is_idempotent() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());
    let truth = vec![(volume_id.clone(), SourceKind::Volume, "测试卡".into())];

    reconcile_with_truth(&state, "t1", &truth);
    assert!(eventually_registry(&state, |id| id == volume_id));
    let after_first = state.devices.lock().unwrap().len();

    // 同真值重复调和（同信号去抖后仍会到达）：无增无删、无重复事件
    let mut rx = state.bus.subscribe();
    for i in 0..3 {
        reconcile_with_truth(&state, &format!("t{i}"), &truth);
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(state.devices.lock().unwrap().len(), after_first, "幂等");
    assert!(drain_removed(&mut rx).is_empty(), "幂等调和不得发移除事件");

    // 大小写变体 = 同设备，不动（归一仅对 PnP 形态生效——文件系统路径
    // 大小写敏感本就不归一，此处用 WPD 形态验证）
    let pnp_lower =
        r"\\?\usb#vid_054c&pid_0e0b#serial#{6ac27878-a6fa-4155-ba85-f98f491d4f33}".to_string();
    let pnp_upper = pnp_lower.to_uppercase();
    let source: std::sync::Arc<dyn devices::DeviceSource> =
        std::sync::Arc::new(devices::volume::VolumeSource::new(src.path()));
    state.devices.lock().unwrap().insert(
        pnp_lower.clone(),
        ipc::DeviceEntry {
            scan: ipc::DeviceScan::Ready,
            source,
            snapshot: devices::orchestrator::DeviceSnapshot {
                id: pnp_lower.clone(),
                name: "相机".into(),
                kind: devices::SourceKind::Mtp,
                files_by_kind: Default::default(),
                bytes_total: 0,
                new_files: 0,
            },
        },
    );
    let count_with_camera = state.devices.lock().unwrap().len();
    reconcile_with_truth(
        &state,
        "t-case",
        &[
            (pnp_upper, SourceKind::Mtp, "相机".into()),
            // 已在册的卷设备仍在真值中（否则它是合法摘除，不算大小写用例）
            (volume_id.clone(), SourceKind::Volume, "测试卡".into()),
        ],
    );
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        state.devices.lock().unwrap().len(),
        count_with_camera,
        "PnP 大小写变体不得视为增删"
    );
    assert!(drain_removed(&mut rx).is_empty());
}

#[test]
fn manager_disconnect_and_recovery_update_projection() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());
    let truth = vec![(volume_id.clone(), SourceKind::Volume, "卡".into())];
    reconcile_with_truth(&state, "t", &truth);
    assert!(eventually_registry(&state, |id| id == volume_id));

    // 管理器确认离线后，扫描投影同步摘除。
    reconcile_with_truth(&state, "manager-offline", &[]);
    assert!(
        eventually_absent(&state, &volume_id),
        "离线剔除应从真值生效（设备摘除）"
    );

    // 恢复在线 → 重回真值 → 重建
    reconcile_with_truth(&state, "health", &truth);
    assert!(
        eventually_registry(&state, |id| id == volume_id),
        "恢复后应重建注册"
    );
}

/// 等 registry 中该 id 消失（上限 5s；消失返回 true，超时仍在返回 false）。
fn eventually_absent(state: &ipc::AppState, target: &str) -> bool {
    let started = std::time::Instant::now();
    loop {
        if !state.devices.lock().unwrap().contains_key(target) {
            return true;
        }
        if started.elapsed() > Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn discovery_without_library_still_scans_and_folder_sources_survive_poll() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 2);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::ZERO,
    ));
    let folder = ipc::scan_folder(&state, &src.path().to_string_lossy()).unwrap();
    state.settings.lock().unwrap().active_library_id = None;
    let id = devices::normalize_device_id(&src.path().to_string_lossy());
    ipc::reconcile::remove_device(&state, &id);
    reconcile_with_truth(
        &state,
        "startup",
        &[(id.clone(), SourceKind::Volume, "卡".into())],
    );
    assert!(
        state.devices.lock().unwrap().contains_key(&id),
        "连接应立即可见"
    );
    assert!(eventually_registry(&state, |key| key == id));
    assert_eq!(state.devices.lock().unwrap()[&id].snapshot.new_files, 2);
    assert!(state.devices.lock().unwrap().contains_key(&folder.id));
}

#[test]
fn rapid_remove_arrive_replaces_connection_and_emits_new_scan() {
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 1);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::ZERO,
    ));
    let id = devices::normalize_device_id(&src.path().to_string_lossy());
    let old = Arc::clone(&state.devices.lock().unwrap()[&id].source);
    let mut rx = state.bus.subscribe();
    ipc::reconcile::remove_device(&state, &id);
    reconcile_with_truth(
        &state,
        "dbt",
        &[(id.clone(), SourceKind::Volume, "相机".into())],
    );
    assert!(eventually_registry(&state, |key| key == id));
    assert!(!Arc::ptr_eq(
        &old,
        &state.devices.lock().unwrap()[&id].source
    ));
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert!(matches!(&events[0], AppEvent::DeviceRemoved { .. }));
    assert!(matches!(&events[1], AppEvent::DeviceArrived { .. }));
    assert!(events
        .iter()
        .any(|event| matches!(event, AppEvent::DeviceScanned { .. })));
}
