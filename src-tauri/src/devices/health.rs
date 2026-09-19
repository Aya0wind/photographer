//! MTP 设备健康监控（幽灵设备探活，2026-09-18 P0）：
//!
//! OS 层真相：相机关 MTP/待机但 USB 仍在 → Windows **不发移除事件**，
//! WPD 接口仍在（Get-PnpDevice Status OK）——注册表里留下“在线”的幽灵
//! 设备。本模块按固定节奏对注册中的 MTP 设备轻量 ping（worker 消息：
//! 打开会话 + 列举顶层 1 个对象，超时 5s）：
//!
//! - 连续 [`FAILURE_THRESHOLD`] 次失败 → `MarkOffline`（registry 摘除 +
//!   `DeviceRemoved` 事件 + 日志；活跃导入走既有 Disconnected→自动暂停链，
//!   引擎持有的 Arc 保证不提前释放）。
//! - 摘除后进 graveyard（探回名单）：恢复可达 → `Revive`（重新发布
//!   DeviceArrived → 重新注册扫描）；DBT 再到达同样接住（幂等）。
//! - 运行时每轮间隔 3 秒，离线设备每轮探回；单次探活最多等待 5 秒。
//!
//! 决策核心 [`monitor_step`] 为纯函数（ping 经闭包注入，测试可 mock）；
//! 线程接线在 lib.rs（TaskSupervisor 任务 `task-health`）。

use std::collections::HashMap;
use std::sync::Mutex;

/// 连续失败多少次判定离线。
pub const FAILURE_THRESHOLD: u32 = 2;

/// 真值修正存储：健康监控标记离线的 MTP 设备 `(id, name)` 集合
///（reconcile 的真值 = enumerate_present_devices() 剔除本集合）。
/// 单写者（健康监控任务），多读者（各 reconcile 信号源）。
static OFFLINE: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

/// 当前离线（快照）。
pub fn offline_ids() -> Vec<String> {
    OFFLINE
        .lock()
        .expect("health offline mutex poisoned")
        .iter()
        .map(|(id, _)| id.clone())
        .collect()
}

/// 标记离线（真值剔除；重复标记幂等——以 name 更新）。
pub fn set_offline(id: &str, name: &str) {
    let mut offline = OFFLINE.lock().expect("health offline mutex poisoned");
    if let Some(entry) = offline.iter_mut().find(|(eid, _)| eid == id) {
        entry.1 = name.to_string();
    } else {
        offline.push((id.to_string(), name.to_string()));
    }
}

/// 恢复在线（重回真值）。
pub fn clear_offline(id: &str) {
    OFFLINE
        .lock()
        .expect("health offline mutex poisoned")
        .retain(|(eid, _)| eid != id);
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn reset_offline_for_test() {
    OFFLINE
        .lock()
        .expect("health offline mutex poisoned")
        .clear();
}

/// graveyard 视图（监控任务的探回名单 = OFFLINE 集合快照；step 后经
/// [`apply_graveyard`] 写回——Revive 的移除由 set 侧 clear_offline 生效，
/// 此处仅同步非 Revive 差异防止漂移）。
pub fn offline_graveyard() -> Vec<(String, String)> {
    OFFLINE
        .lock()
        .expect("health offline mutex poisoned")
        .clone()
}

/// 写回监控循环处理后的 graveyard（以 step 输出为准；Revive 项已被
/// monitor_step 移除，对应 clear_offline 由调用方执行）。
pub fn apply_graveyard(graveyard: &[(String, String)]) {
    let mut offline = OFFLINE.lock().expect("health offline mutex poisoned");
    // 保留：仍在 graveyard 中的（未恢复）；以最新 name 更新
    *offline = graveyard.to_vec();
}

/// 探活动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthAction {
    /// 判定离线：摘除注册 + DeviceRemoved（携带 id 与展示名供探回）。
    MarkOffline { id: String, name: String },
    /// graveyard 中恢复可达：重新 DeviceArrived（重新注册扫描）。
    Revive { id: String, name: String },
}

/// 一轮健康决策（纯函数）。
///
/// - `registered`：注册中的 MTP 设备 `(规范化 id, 展示名)`；
/// - `graveyard`：已离线待探回的 `(id, name)`（in-out：Revive 时移除；
///   真实接线中即 OFFLINE 集合快照，见 [`graveyard_snapshot`]）；
/// - `failures`：连续失败计数（in-out：成功清零，判离线后消费）；
/// - `ping`：探活闭包（返回是否可达；真实接线为 worker ping + 5s 超时）；
/// - `probe_graveyard`：本轮是否探 graveyard（退避：每 3 轮一次）。
pub fn monitor_step<P: Into<Option<bool>>>(
    registered: &[(String, String)],
    graveyard: &mut Vec<(String, String)>,
    failures: &mut HashMap<String, u32>,
    ping: &mut dyn FnMut(&str) -> P,
    probe_graveyard: bool,
) -> Vec<HealthAction> {
    let mut actions = Vec::new();
    // 注册中设备：逐个探活（单设备并发 1：顺序执行）
    for (id, name) in registered {
        let Some(reachable) = ping(id).into() else {
            continue;
        };
        if reachable {
            failures.remove(id);
        } else {
            let count = failures.entry(id.clone()).or_insert(0);
            *count += 1;
            if *count >= FAILURE_THRESHOLD {
                actions.push(HealthAction::MarkOffline {
                    id: id.clone(),
                    name: name.clone(),
                });
                failures.remove(id);
            }
        }
    }
    // graveyard 探回（退避轮空时不探，避免对死设备风暴重试）
    if probe_graveyard {
        let mut index = 0;
        while index < graveyard.len() {
            let (id, name) = &graveyard[index];
            if ping(id).into() == Some(true) {
                actions.push(HealthAction::Revive {
                    id: id.clone(),
                    name: name.clone(),
                });
                graveyard.remove(index);
            } else {
                index += 1;
            }
        }
    }
    actions
}
