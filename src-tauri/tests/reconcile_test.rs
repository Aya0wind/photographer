//! 设备状态调和器（Reconciler）语义测试：真值→增/删、幂等重复触发、
//! 大小写变体同设备、健康离线剔除/恢复经真值生效、启动真值调和烟测。
//! 用注入真值 `reconcile_with_truth` 保证确定性（真实真值依赖本机设备）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings, tasks, thumbs};

use std::sync::Arc;
use std::time::Duration;

use common::{build_many, state_with_library};
use devices::SourceKind;
use events::AppEvent;
use ipc::reconcile::{reconcile_devices, reconcile_with_truth};

/// 等 registry 达到谓词（上限 5s；扫描经 supervisor 异步）。
fn eventually_registry(state: &ipc::AppState, cond: impl Fn(&str) -> bool) -> bool {
    let started = std::time::Instant::now();
    loop {
        let hit = state.devices.lock().unwrap().keys().any(|id| cond(id));
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
fn health_offline_exclusion_and_recovery_flow_through_truth() {
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

    // 健康监控标记离线（真值修正）→ 即便枚举真值仍含该设备，调和也摘除
    devices::health::set_offline(&volume_id, "卡");
    reconcile_with_truth(&state, "health", &truth);
    assert!(
        eventually_absent(&state, &volume_id),
        "离线剔除应从真值生效（设备摘除）"
    );

    // 恢复在线 → 重回真值 → 重建
    devices::health::clear_offline(&volume_id);
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
fn real_truth_reconcile_smoke() {
    // 真实真值调和烟测：不 panic；注册表与真实真值一致（测试注册的
    // tempdir 卷源不在真实真值 → 被摘除；真实可移动卷/WPD 由本机决定）
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 1);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());

    reconcile_devices(&state, "smoke");
    assert!(
        eventually_absent(&state, &volume_id),
        "真实真值不含 tempdir 卷源 → 必须摘除（结构性一致）"
    );
}

#[test]
fn h1_stale_offline_mark_forces_rescan_replacement() {
    // 场景 H1：先连相机 → 关电源 → 重开并切 MTP（Windows 发 remove+arrival
    // 对，但摘除未及生效）——注册表仍有幽灵条目 + 探活离线标记仍在 +
    // 到达信号触发 reconcile（真值含该设备）：
    // 必须清除陈旧标记、摘除幽灵条目并**重扫替换**（不得因已注册跳过）。
    let src = tempfile::tempdir().unwrap();
    build_many(src.path(), 3);
    let db_dir = tempfile::tempdir().unwrap();
    let state = Arc::new(state_with_library(
        db_dir.path(),
        src.path(),
        Duration::from_millis(1),
    ));
    let volume_id = devices::normalize_device_id(&src.path().to_string_lossy());

    // 幽灵态：在册 + 探活已标记离线（连续失败）
    devices::health::set_offline(&volume_id, "旧名");

    let mut rx = state.bus.subscribe();
    let truth = vec![(volume_id.clone(), SourceKind::Volume, "新扫描".into())];
    reconcile_with_truth(&state, "h1-arrival", &truth);

    // 重扫完成：注册表仍含该设备（条目被替换为新鲜扫描结果）
    assert!(
        eventually_registry(&state, |id| id == volume_id),
        "H1：必须重扫替换而非跳过"
    );
    // 陈旧离线标记已清除
    assert!(
        !devices::health::offline_ids().contains(&volume_id),
        "H1：真值在场胜过陈旧离线标记"
    );
    // 幽灵条目摘除 + 重扫发布的事件序列（DeviceRemoved + DeviceScanned）
    let mut removed_hit = false;
    let mut scanned_hit = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            AppEvent::DeviceRemoved { id } if devices::normalize_device_id(&id) == volume_id => {
                removed_hit = true;
            }
            AppEvent::DeviceScanned { id, snapshot, .. }
                if devices::normalize_device_id(&id) == volume_id =>
            {
                assert_eq!(snapshot.new_files, 3, "新扫描结果（非旧幽灵快照）");
                scanned_hit = true;
            }
            _ => {}
        }
    }
    assert!(removed_hit, "幽灵条目必须摘除（替换语义）");
    assert!(scanned_hit, "重扫必须发布 DeviceScanned");
}
