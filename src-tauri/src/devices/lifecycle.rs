//! 单写者连接状态机。系统枚举只证明接口在位，MTP 探测成功才允许扫描。
//! 不做 I/O；异步结果带单调递增的代次，移除后的旧结果无法复活设备。
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::{normalize_device_id, SourceKind};

pub type DeviceInfo = (String, SourceKind, String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Connection {
    Waiting,
    Online,
}

struct Device {
    kind: SourceKind,
    name: String,
    generation: u64,
    connection: Connection,
    failures: u32,
    next_probe: Instant,
    probing: bool,
}

#[derive(Debug, Clone)]
pub struct Probe {
    pub id: String,
    pub generation: u64,
}

#[derive(Default)]
pub struct DeviceManager {
    devices: HashMap<String, Device>,
    generation: u64,
}

impl DeviceManager {
    /// None 表示枚举失败（未知），不得当成空列表删除设备。
    pub fn observe(
        &mut self,
        kind: SourceKind,
        found: Option<Vec<(String, String)>>,
        now: Instant,
    ) {
        let Some(found) = found else { return };
        let found: HashMap<_, _> = found
            .into_iter()
            .map(|(id, name)| (normalize_device_id(&id), name))
            .collect();
        self.devices
            .retain(|id, device| device.kind != kind || found.contains_key(id));
        for (id, name) in found {
            if let Some(device) = self.devices.get_mut(&id) {
                device.name = name;
                continue;
            }
            self.generation += 1;
            self.devices.insert(
                id,
                Device {
                    kind,
                    name,
                    generation: self.generation,
                    connection: if kind == SourceKind::Mtp {
                        Connection::Waiting
                    } else {
                        Connection::Online
                    },
                    failures: 0,
                    next_probe: now,
                    probing: false,
                },
            );
        }
    }

    pub fn topology(&mut self, id: &str, arrived: bool, now: Instant) {
        let id = normalize_device_id(id);
        if !arrived {
            self.devices.remove(&id);
        } else if let Some(device) = self.devices.get_mut(&id) {
            // 到达信号允许提前重试，但不能冒充一次成功探测。
            device.next_probe = now;
        }
    }

    pub fn due_probes(&mut self, now: Instant) -> Vec<Probe> {
        self.devices
            .iter_mut()
            .filter_map(|(id, device)| {
                if device.kind != SourceKind::Mtp || device.probing || now < device.next_probe {
                    return None;
                }
                device.probing = true;
                Some(Probe {
                    id: id.clone(),
                    generation: device.generation,
                })
            })
            .collect()
    }

    pub fn complete_probe(&mut self, probe: Probe, reachable: Option<bool>, now: Instant) {
        let Some(device) = self
            .devices
            .get_mut(&probe.id)
            .filter(|device| device.generation == probe.generation && device.probing)
        else {
            return;
        };
        device.probing = false;
        device.next_probe = now + Duration::from_secs(3);
        match reachable {
            Some(true) => {
                device.failures = 0;
                device.connection = Connection::Online;
            }
            Some(false) => {
                device.failures = device.failures.saturating_add(1);
                if device.failures >= 2 || device.connection == Connection::Waiting {
                    device.connection = Connection::Waiting;
                    // 初始就绪与 MTP 恢复使用同一路径，重试间隔 1/2/4/8 秒。
                    device.next_probe =
                        now + Duration::from_secs(1 << device.failures.saturating_sub(1).min(3));
                }
            }
            None => {} // 扫描/传输占用会话，既不判离线，也不恢复上线。
        }
    }

    pub fn online(&self) -> Vec<DeviceInfo> {
        self.devices
            .iter()
            .filter(|(_, device)| device.connection == Connection::Online)
            .map(|(id, device)| (id.clone(), device.kind, device.name.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn observe(manager: &mut DeviceManager, now: Instant) {
        manager.observe(
            SourceKind::Mtp,
            Some(vec![("camera".into(), "相机".into())]),
            now,
        );
    }
    fn probe(manager: &mut DeviceManager, now: Instant, result: Option<bool>) {
        let work = manager.due_probes(now).pop().unwrap();
        manager.complete_probe(work, result, now);
    }
    #[test]
    fn startup_orders_and_mtp_toggle_converge() {
        for app_first in [true, false] {
            let mut manager = DeviceManager::default();
            let mut now = Instant::now();
            if app_first {
                manager.observe(SourceKind::Mtp, Some(vec![]), now);
            }
            observe(&mut manager, now);
            assert!(manager.online().is_empty());
            probe(&mut manager, now, Some(false)); // 接口先出现，会话尚未就绪
            now += Duration::from_secs(1);
            probe(&mut manager, now, Some(true));
            assert_eq!(manager.online().len(), 1);
            for _ in 0..2 {
                now += Duration::from_secs(3);
                probe(&mut manager, now, Some(false));
            }
            assert!(manager.online().is_empty()); // 关 MTP，但系统仍枚举得到
            observe(&mut manager, now);
            assert!(manager.online().is_empty());
            now += Duration::from_secs(8);
            probe(&mut manager, now, Some(true));
            assert_eq!(manager.online().len(), 1);
        }
    }
    #[test]
    fn replug_rejects_previous_probe_and_resets_backoff() {
        let mut manager = DeviceManager::default();
        let now = Instant::now();
        observe(&mut manager, now);
        let old = manager.due_probes(now).pop().unwrap();
        manager.topology("camera", false, now);
        observe(&mut manager, now);
        manager.complete_probe(old, Some(true), now);
        assert!(manager.online().is_empty());
        probe(&mut manager, now, Some(true));
        assert_eq!(manager.online().len(), 1);
    }
    #[test]
    fn unknown_enumeration_busy_probe_and_duplicate_arrival_preserve_online() {
        let mut manager = DeviceManager::default();
        let now = Instant::now();
        observe(&mut manager, now);
        probe(&mut manager, now, Some(true));
        manager.observe(SourceKind::Mtp, None, now);
        manager.topology("camera", true, now);
        probe(&mut manager, now, None);
        assert_eq!(manager.online().len(), 1);
        manager.observe(SourceKind::Mtp, Some(vec![]), now);
        assert!(manager.online().is_empty());
    }
    #[test]
    fn polls_do_not_reset_retry_or_duplicate_inflight_probe() {
        let mut manager = DeviceManager::default();
        let now = Instant::now();
        observe(&mut manager, now);
        let work = manager.due_probes(now).pop().unwrap();
        observe(&mut manager, now);
        assert!(manager.due_probes(now).is_empty());
        manager.complete_probe(work, Some(false), now);
        observe(&mut manager, now);
        assert!(manager.due_probes(now).is_empty());
        assert_eq!(manager.due_probes(now + Duration::from_secs(1)).len(), 1);
    }

    #[test]
    fn repeated_replug_never_accepts_stale_results() {
        let mut manager = DeviceManager::default();
        let now = Instant::now();
        let mut stale = Vec::new();
        for _ in 0..100 {
            observe(&mut manager, now);
            stale.extend(manager.due_probes(now));
            manager.topology("camera", false, now);
        }
        observe(&mut manager, now);
        probe(&mut manager, now, Some(true));
        for work in stale {
            manager.complete_probe(work, Some(false), now);
        }
        assert_eq!(manager.online().len(), 1);
    }

    #[test]
    fn busy_does_not_erase_failure_and_other_devices_remain_independent() {
        let mut manager = DeviceManager::default();
        let mut now = Instant::now();
        manager.observe(
            SourceKind::Mtp,
            Some(vec![("a".into(), "A".into()), ("b".into(), "B".into())]),
            now,
        );
        for work in manager.due_probes(now) {
            manager.complete_probe(work, Some(true), now);
        }
        for result in [Some(false), None, Some(false)] {
            now += Duration::from_secs(3);
            for work in manager.due_probes(now) {
                let value = if work.id == "a" { result } else { Some(true) };
                manager.complete_probe(work, value, now);
            }
        }
        let online = manager.online();
        assert_eq!(online.len(), 1);
        assert_eq!(online[0].0, "b");
        manager.topology("a", true, now);
        for work in manager.due_probes(now) {
            manager.complete_probe(work, Some(true), now);
        }
        assert_eq!(manager.online().len(), 2);
    }
}
