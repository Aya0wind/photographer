mod db;
mod devices;
mod events;
mod import;
mod ipc;
mod metadata;
pub mod settings;
mod tray;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Emitter, Manager};

use crate::devices::hotplug;
use crate::devices::orchestrator::scan_device;
use crate::devices::volume::VolumeSource;
use crate::devices::wpd::WpdSource;
use crate::devices::{DeviceSource, SourceKind};
use crate::events::{AppEvent, EventBus};
use crate::ipc::{active_library_db, AppState, DeviceEntry};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // single-instance 必须第一个注册：二次启动时显示并聚焦已有主窗口。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // 自启动默认不启用，由设置页通过命令开关。
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 存储模型（达芬奇式，设计文档 §5.11）：全局配置固定在应用标准配置目录；
            // 库（SQLite/缩略图/向量）在各自独立的 dbDir（如 I:\SmartPhoto\<库名>），
            // 照片根（如 Y:\照片）与数据库目录分离，由引导向导写入库注册表。
            let config_dir = app
                .path()
                .app_config_dir()
                .expect("failed to resolve app config dir");
            std::fs::create_dir_all(&config_dir)?;
            let settings = SettingsManager::load(&config_dir).unwrap_or_else(|err| {
                eprintln!("failed to load settings, falling back to defaults: {err}");
                Settings::default()
            });
            let bus = EventBus::new();
            app.manage(AppState {
                settings: Mutex::new(settings),
                config_dir,
                bus: bus.clone(),
                devices: Mutex::new(HashMap::new()),
                active_import: Mutex::new(None),
            });

            // 后台线程 1：领域事件转发（bus → 前端 `app://event`）
            spawn_event_forwarder(app.handle().clone(), bus.clone());
            // 后台线程 2：设备编排（热插拔 → 建源 → 扫描 → 注册表 + DeviceScanned）
            spawn_device_orchestrator(app.handle().clone());
            // 后台线程 3：系统通知（会话开始/结束 + 里程碑）
            spawn_notification_subscriber(app.handle().clone());
            // 后台线程 4：热插拔检测（message-only 窗口泵）
            let _hotplug = hotplug::spawn_hotplug_thread(bus);

            // 主窗口关闭行为：close_to_tray=true 时隐藏到托盘，否则放行正常退出。
            if let Some(main_window) = app.get_webview_window("main") {
                let window = main_window.clone();
                main_window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        let close_to_tray = window
                            .app_handle()
                            .state::<AppState>()
                            .settings
                            .lock()
                            .expect("settings mutex poisoned")
                            .system
                            .close_to_tray;
                        if close_to_tray {
                            api.prevent_close();
                            let _ = window.hide();
                        }
                    }
                });
            }

            tray::create(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::settings::settings_get,
            ipc::settings::settings_set,
            ipc::device::device_list,
            ipc::device::device_scan,
            ipc::device::device_files,
            ipc::device::folder_scan,
            ipc::device::fs_list_dirs,
            ipc::import::import_start,
            ipc::import::import_pause,
            ipc::import::import_resume,
            ipc::import::import_cancel,
            ipc::import::import_jobs_page,
            ipc::import::import_logs_page,
            ipc::import::import_retry_failed,
            ipc::import::clean_candidates,
            ipc::import::clean_apply,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 事件转发：EventBus → 前端 `app://event`（唯一的前端事件通道）。
fn spawn_event_forwarder(app: AppHandle, bus: EventBus) {
    std::thread::Builder::new()
        .name("event-forwarder".into())
        .spawn(move || {
            let mut rx = bus.subscribe();
            loop {
                match rx.blocking_recv() {
                    Ok(event) => {
                        let _ = app.emit("app://event", &event);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .expect("spawn event forwarder");
}

/// 设备编排：启动存量设备枚举（补扫）+ DeviceArrived → 建源 → 扫描 →
/// 注册表 + DeviceScanned；DeviceRemoved → 注销。
fn spawn_device_orchestrator(app: AppHandle) {
    std::thread::Builder::new()
        .name("device-orchestrator".into())
        .spawn(move || {
            let bus = app.state::<AppState>().bus.clone();
            let mut rx = bus.subscribe();
            // 启动存量设备枚举（相机检测不到的修复，2026-09-18）：必须在
            // 订阅之后发布——app 重启窗口内插入的设备收不到热插事件，
            // 这里把当下在位的设备走与热插**完全相同**的 DeviceArrived 链路
            // 补发一遍（卷仅含有媒体的可移动介质；网络盘/本地盘在
            // present::probe_volume 统一过滤）。
            let present = devices::present::enumerate_present_devices();
            eprintln!("启动存量设备枚举：发现 {} 台设备", present.len());
            for (id, kind, name) in present {
                eprintln!("存量设备在位：{name}（kind={kind:?}, id={id}）");
                bus.publish(AppEvent::DeviceArrived { id, kind, name });
            }
            loop {
                let Ok(event) = rx.blocking_recv() else {
                    continue;
                };
                match event {
                    AppEvent::DeviceArrived { id, kind, name } => {
                        handle_device_arrived(&app, id, kind, name);
                    }
                    AppEvent::DeviceRemoved { id } => {
                        app.state::<AppState>()
                            .devices
                            .lock()
                            .expect("devices mutex poisoned")
                            .remove(&id);
                    }
                    _ => {}
                }
            }
        })
        .expect("spawn device orchestrator");
}

fn handle_device_arrived(app: &AppHandle, id: String, kind: SourceKind, name: String) {
    // 失败日志必须带 id/kind 上下文（2026-09-18 排查教训：静默吞错全靠猜）
    let kind_tag = format!("{kind:?}");
    let source: Arc<dyn DeviceSource> = match kind {
        // 卷事件 id 形如 "E:"，根路径必须补尾反斜杠（"E:" 是该盘当前目录）
        SourceKind::Volume => Arc::new(VolumeSource::new(format!("{id}\\"))),
        SourceKind::Folder => {
            // 文件夹源不经热插拔/启动枚举产生（仅 folder_scan 注册），忽略
            return;
        }
        SourceKind::Mtp => match devices::wpd::enumerate_mtp_devices() {
            Ok(list) => match list.into_iter().find(|(pnp, _)| *pnp == id) {
                Some((pnp, friendly)) => Arc::new(WpdSource::new(pnp, friendly)),
                None => {
                    eprintln!(
                        "设备到达处理失败 kind={kind_tag} id={id}: WPD 枚举未找到该设备，忽略"
                    );
                    return;
                }
            },
            Err(err) => {
                eprintln!(
                    "设备到达处理失败 kind={kind_tag} id={id}: WPD 枚举失败（相机未切 PC 模式？）: {err}"
                );
                return;
            }
        },
    };

    let state = app.state::<AppState>();
    let skip_imported = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .import
        .skip_imported;
    let Ok(db) = active_library_db(&state) else {
        eprintln!("设备 {id}（kind={kind_tag}）到达但尚未创建库，跳过扫描");
        return;
    };
    let snapshot = match scan_device(&*source, &db, skip_imported) {
        Ok(snapshot) => snapshot,
        Err(err) => {
            eprintln!("设备到达处理失败 kind={kind_tag} id={id}: 扫描设备失败: {err}");
            return;
        }
    };
    let total_files: u64 = snapshot.files_by_kind.values().sum();
    state
        .devices
        .lock()
        .expect("devices mutex poisoned")
        .insert(
            id.clone(),
            DeviceEntry {
                source,
                snapshot: snapshot.clone(),
            },
        );
    eprintln!("设备已注册并扫描：{name}（kind={kind_tag}, id={id}, {total_files} 个媒体文件）");
    state.bus.publish(AppEvent::DeviceScanned {
        id,
        name,
        kind,
        snapshot,
    });
}

/// 系统通知：会话开始/结束 + 里程碑（读 settings.import.notify_milestones）。
fn spawn_notification_subscriber(app: AppHandle) {
    use tauri_plugin_notification::NotificationExt;

    fn notify_enabled(app: &AppHandle) -> bool {
        app.state::<AppState>()
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .import
            .notify_milestones
    }
    fn notify(app: &AppHandle, title: &str, body: &str) {
        let _ = app.notification().builder().title(title).body(body).show();
    }

    std::thread::Builder::new()
        .name("notify-subscriber".into())
        .spawn(move || {
            let bus = app.state::<AppState>().bus.clone();
            let mut rx = bus.subscribe();
            loop {
                let Ok(event) = rx.blocking_recv() else {
                    continue;
                };
                let message = match event {
                    AppEvent::ImportSessionStarted { total_files, .. } => {
                        Some(("导入已开始", format!("共 {total_files} 个文件")))
                    }
                    AppEvent::ImportMilestoneReached { percent, .. } => {
                        Some(("导入进度", format!("已完成 {percent}%")))
                    }
                    AppEvent::ImportSessionFinished { stats, .. } => Some((
                        "导入完成",
                        format!(
                            "成功 {} · 跳过 {} · 失败 {}",
                            stats.done_files, stats.skipped_duplicates, stats.failed_files
                        ),
                    )),
                    _ => None,
                };
                let Some((title, body)) = message else {
                    continue;
                };
                if !notify_enabled(&app) {
                    continue;
                }
                notify(&app, title, &body);
            }
        })
        .expect("spawn notify subscriber");
}

use crate::settings::{Settings, SettingsManager};
