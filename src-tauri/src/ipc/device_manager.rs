//! 唯一连接状态管理循环。I/O 只提交观察结果，不直接修改连接列表。
use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::SharedState;
use crate::devices::lifecycle::{DeviceManager, Probe};
use crate::devices::SourceKind;
use crate::events::AppEvent;

enum Observation {
    Discovery {
        revision: u64,
        kind: SourceKind,
        found: Option<Vec<(String, String)>>,
    },
    Reachability(Probe, Option<bool>),
}

pub fn spawn(state: SharedState) {
    let capabilities = crate::platform::capabilities();
    if !capabilities.volume_devices && !capabilities.portable_devices {
        return;
    }
    // 先订阅再启动系统通知，启动枚举补上 App 打开前已连接的设备。
    let mut events = state.bus.subscribe();
    std::thread::Builder::new()
        .name("device-manager".into())
        .spawn(move || {
            let mut manager = DeviceManager::default();
            let (send, receive) = mpsc::channel();
            let mut revision = 0_u64;
            let mut discovery = [
                (SourceKind::Volume, false, Instant::now()),
                (SourceKind::Mtp, false, Instant::now()),
            ]
            .into_iter()
            .filter(|(kind, ..)| match kind {
                SourceKind::Volume => capabilities.volume_devices,
                SourceKind::Mtp => capabilities.portable_devices,
                SourceKind::Folder => false,
            })
            .collect::<Vec<_>>();
            loop {
                let now = Instant::now();
                loop {
                    match events.try_recv() {
                        Ok(AppEvent::DeviceTopologyChanged { id, arrived }) => {
                            revision += 1;
                            manager.topology(&id, arrived, now);
                            if !arrived && !id.is_empty() {
                                super::reconcile::remove_device(&state, &id);
                            }
                            for (_, _, next) in &mut discovery {
                                *next = (*next).min(now + Duration::from_millis(300));
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {
                            revision += 1;
                            for (_, _, next) in &mut discovery {
                                *next = now;
                            }
                        }
                        Err(_) => break,
                    }
                }
                while let Ok(observation) = receive.try_recv() {
                    match observation {
                        Observation::Discovery {
                            revision: started,
                            kind,
                            found,
                        } => {
                            let (_, running, next) = discovery
                                .iter_mut()
                                .find(|(candidate, ..)| *candidate == kind)
                                .unwrap();
                            *running = false;
                            if started == revision {
                                manager.observe(kind, found, now);
                                *next = now + Duration::from_secs(2);
                            } else {
                                // 插拔期间取得的旧快照不具备覆盖当前连接状态的资格。
                                *next = now;
                            }
                        }
                        Observation::Reachability(probe, reachable) => {
                            manager.complete_probe(probe, reachable, now)
                        }
                    }
                }
                super::reconcile::reconcile_with_truth(&state, "manager", &manager.online());
                for (kind, running, next) in &mut discovery {
                    if *running || now < *next {
                        continue;
                    }
                    *running = true;
                    let kind = *kind;
                    let send = send.clone();
                    state
                        .supervisor
                        .spawn("discovery", format!("{kind:?}"), move |_| {
                            let found = std::panic::catch_unwind(|| match kind {
                                SourceKind::Volume => {
                                    crate::devices::present::enumerate_present_volumes()
                                        .map_err(|error| {
                                            crate::devices::diagnostics::record(format!(
                                                "volume enumeration unavailable: {error}"
                                            ));
                                        })
                                        .ok()
                                }
                                SourceKind::Mtp => crate::devices::wpd::enumerate_mtp_devices()
                                    .map_err(|error| {
                                        crate::devices::diagnostics::record(format!(
                                            "device enumeration unavailable: {error}"
                                        ));
                                    })
                                    .ok(),
                                SourceKind::Folder => unreachable!(),
                            })
                            .unwrap_or_default();
                            let _ = send.send(Observation::Discovery {
                                revision,
                                kind,
                                found,
                            });
                        });
                }
                for probe in manager.due_probes(now) {
                    let send = send.clone();
                    state.supervisor.spawn("probe", probe.id.clone(), move |_| {
                        let reachable = std::panic::catch_unwind(|| {
                            crate::devices::wpd::worker_ping(&probe.id)
                        })
                        .unwrap_or(Some(false));
                        let _ = send.send(Observation::Reachability(probe, reachable));
                    });
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })
        .expect("spawn device manager");
}
