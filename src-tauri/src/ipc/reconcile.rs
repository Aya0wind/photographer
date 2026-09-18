//! 设备状态调和器（Reconciler，2026-09-18 架构转向）：
//!
//! **电平触发**替代四条特殊路径（热插到达/移除分支、启动存量枚举、
//! 幂等守卫、探活直发事件）。唯一真值 = `present::enumerate_present_devices()`
//! （WPD GetDevices + 有媒体可移动卷探测）再剔除健康监控标记离线的
//! MTP 设备（探活连续失败的真值修正）。所有信号源——DBT 事件（到达/
//! 移除不分语义）、启动、探活 tick——都只是触发器（调用方去抖合并），
//! 统一收敛到 [`reconcile_devices`]：
//!
//! - 真值有、registry 无 → 建源扫描（supervisor 任务，幂等占位防重复）
//!   → 注册 → `DeviceScanned`；
//! - registry 有、真值无 → 摘除 + `DeviceRemoved`（活跃导入持有的源 Arc
//!   由既有 Disconnected→自动暂停链处理）；
//! - 都有 → 不动（天然幂等，含大小写归一后的同设备判定）。
//!
//! 扫描完成落库前再核真值（防"扫描期间设备拔出"的幽灵注册）。
//! 每次调和打一行日志（信号源 + 真值 N 台 + 增/删）。
//!
//! 核心带注入真值版本 [`reconcile_with_truth`]（集成测试可控）；
//! [`reconcile_devices`] 枚举真实真值。

use std::sync::Mutex;

use crate::devices::orchestrator::scan_device;
use crate::devices::volume::VolumeSource;
use crate::devices::wpd::WpdSource;
use crate::devices::{normalize_device_id, DeviceSource, SourceKind};
use crate::events::AppEvent;

use super::{active_library_db, SharedState};

/// 进行中扫描占位（防同一设备重复 spawn 扫描任务；进程级）。
/// Vec 而非 HashSet：static 初始化须 const 构造（数量级为设备数，线性无碍）。
static SCAN_IN_FLIGHT: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// 枚举真值（唯一真值来源）：存量设备 + 健康离线剔除。
pub fn present_truth() -> Vec<(String, SourceKind, String)> {
    let offline = crate::devices::health::offline_ids();
    crate::devices::present::enumerate_present_devices()
        .into_iter()
        .filter(|(id, ..)| !offline.iter().any(|off| off == id))
        .collect()
}

/// 调和：真值 ↔ registry 求差 → 增（扫描任务）/删（摘除+事件）。
pub fn reconcile_devices(state: &SharedState, trigger: &str) {
    let truth = present_truth();
    reconcile_impl(state, trigger, &truth, TruthSource::Real);
}

/// 带注入真值的调和核心（测试用；真值 id 逐一归一）。
/// 带注入真值的调和入口（集成测试专用；lib 目标内无调用点）。
#[allow(dead_code)]
pub fn reconcile_with_truth(
    state: &SharedState,
    trigger: &str,
    truth: &[(String, SourceKind, String)],
) {
    reconcile_impl(state, trigger, truth, TruthSource::Injected);
}

fn reconcile_impl(
    state: &SharedState,
    trigger: &str,
    truth: &[(String, SourceKind, String)],
    trigger_source: TruthSource,
) {
    // 场景 H1（关机重开）：设备重新出现在真值中但仍挂着陈旧的探活离线
    // 标记（摘除还没生效/未恢复）——枚举在场胜过陈旧标记：清除标记并把
    // 该设备视为**不在册**（即使幽灵条目还在 → 摘除 + 重扫替换，绝不能
    // 因「已注册」跳过）。
    let mut stale_offline: Vec<String> = Vec::new();
    let truth: Vec<(String, SourceKind, String)> = truth
        .iter()
        .map(|(id, kind, name)| (normalize_device_id(id), *kind, name.clone()))
        .filter(|(id, ..)| {
            if crate::devices::health::offline_ids()
                .iter()
                .any(|off| off == id)
            {
                crate::devices::health::clear_offline(id);
                stale_offline.push(id.clone());
                eprintln!(
                    "reconcile[{trigger}]: 离线标记与真值冲突（设备已回来），清除并强制重扫: {id}"
                );
            }
            true // 在场设备保留在真值
        })
        .collect();

    // diff + 摘除（持锁做纯内存操作；扫描任务锁外 spawn）
    let truth_count = truth.len();
    let truth_ids: Vec<String> = truth.iter().map(|(id, ..)| id.clone()).collect();
    let (removed, added) = {
        let mut registry = state.devices.lock().expect("devices mutex poisoned");
        let mut removed: Vec<String> = registry
            .keys()
            .filter(|id| {
                !truth.iter().any(|(tid, ..)| tid == *id)
                    || stale_offline.iter().any(|sid| sid == *id) // 幽灵态：摘除待替换
            })
            .cloned()
            .collect();
        removed.sort();
        removed.dedup();
        let mut claims = SCAN_IN_FLIGHT
            .lock()
            .expect("scan in-flight mutex poisoned");
        let added: Vec<(String, SourceKind, String)> = truth
            .into_iter()
            .filter(|(id, ..)| {
                // 强制重扫集合无视「已注册」（claims 仍防重复 spawn）
                stale_offline.iter().any(|sid| sid == id)
                    || (!registry.keys().any(|rid| rid == id) && !claims.iter().any(|c| c == id))
            })
            .collect();
        for (id, ..) in &added {
            claims.push(id.clone());
        }
        for id in &removed {
            registry.remove(id);
        }
        (removed, added)
    };

    eprintln!(
        "reconcile[{trigger}]: 真值 {truth_count} 台；+{} -{}",
        added.len(),
        removed.len()
    );

    for id in removed {
        eprintln!("reconcile[{trigger}]: 设备不在真值中，摘除: {id}");
        state.bus.publish(AppEvent::DeviceRemoved { id });
    }
    let guard = match trigger_source {
        TruthSource::Real => TruthGuard::Real,
        TruthSource::Injected => TruthGuard::Snapshot(truth_ids),
    };
    for (id, kind, name) in added {
        spawn_scan(state, id, kind, name, trigger, guard.clone());
    }
}

/// 调和触发来源：真实枚举走 Real（落库前重新枚举复核）；注入真值走
/// Injected（用快照复核，测试确定性）。
#[derive(Clone, Copy)]
enum TruthSource {
    Real,
    /// 测试注入（reconcile_with_truth 入口）。
    #[allow(dead_code)]
    Injected,
}

/// 真值来源（真实枚举 / 测试注入）——决定扫描落库前复核用的真值。
#[derive(Clone)]
enum TruthGuard {
    /// 重新枚举真实真值（运行时：扫描期间设备状态可能已变）。
    Real,
    /// 用触发调和时的真值快照（测试确定性）。
    Snapshot(Vec<String>),
}

impl TruthGuard {
    fn contains(&self, id: &str) -> bool {
        match self {
            TruthGuard::Real => present_truth().iter().any(|(tid, ..)| tid == id),
            TruthGuard::Snapshot(ids) => ids.iter().any(|tid| tid == id),
        }
    }
}

/// 建源扫描任务（supervisor；完成时注册 + DeviceScanned，幂等占位释放）。
/// 落库前再核真值（扫描期间拔出 → 不注册；下一轮调和自然清理）。
fn spawn_scan(
    state: &SharedState,
    id: String,
    kind: SourceKind,
    name: String,
    trigger: &str,
    guard: TruthGuard,
) {
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    let state = std::sync::Arc::clone(state);
    let trigger = trigger.to_string();
    supervisor.spawn(
        "scan",
        name.clone(),
        move |_| {
            let source: std::sync::Arc<dyn DeviceSource> = match kind {
                // 卷 id 形如 "E:" 或测试目录路径：根路径补尾反斜杠
                SourceKind::Volume => {
                    std::sync::Arc::new(VolumeSource::new(format!("{id}\\")))
                }
                SourceKind::Mtp => std::sync::Arc::new(WpdSource::new(id.clone(), name.clone())),
                // 文件夹源不经真值产生（仅 folder_scan 注册）
                SourceKind::Folder => {
                    release_claim(&id);
                    return;
                }
            };
            let result = (|| -> Result<crate::devices::orchestrator::DeviceSnapshot, String> {
                let skip_imported = state
                    .settings
                    .lock()
                    .expect("settings mutex poisoned")
                    .import
                    .skip_imported;
                let db = active_library_db(&state)?;
                scan_device(&*source, &db, skip_imported).map_err(|e| e.to_string())
            })();

            match result {
                Ok(snapshot) => {
                    // 落库前再核真值（防扫描期间拔出的幽灵注册；含健康离线剔除；
                    // 真值来源与触发调和一致）
                    if !guard.contains(&id) {
                        eprintln!("reconcile[{trigger}]: 扫描完成但设备已不在真值，放弃注册: {id}");
                    } else {
                        let total: u64 = snapshot.files_by_kind.values().sum();
                        state.devices.lock().expect("devices mutex poisoned").insert(
                            id.clone(),
                            super::DeviceEntry {
                                source,
                                snapshot: snapshot.clone(),
                            },
                        );
                        eprintln!(
                            "reconcile[{trigger}]: 设备已注册并扫描：{name}（id={id}, {total} 个媒体文件）"
                        );
                        state.bus.publish(AppEvent::DeviceScanned {
                            id: id.clone(),
                            name,
                            kind,
                            snapshot,
                        });
                    }
                }
                Err(err) => {
                    eprintln!(
                        "reconcile[{trigger}]: 设备到达处理失败 kind={kind:?} id={id}: 扫描设备失败: {err}"
                    );
                }
            }
            release_claim(&id);
        },
    );
}

fn release_claim(id: &str) {
    SCAN_IN_FLIGHT
        .lock()
        .expect("scan in-flight mutex poisoned")
        .retain(|c| c != id);
}
