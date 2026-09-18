mod db;
mod devices;
mod events;
mod import;
mod ipc;
mod metadata;
pub mod settings;
mod tasks;
mod thumbs;
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
            // 托管 Arc<AppState>（SharedState）：async 慢命令壳需要 'static
            // clone 进 spawn_blocking 闭包（铁律：慢操作不上主线程）。
            // supervisor：统一后台任务框架（panic 捕获 + 命名 + 软取消）。
            let supervisor = tasks::TaskSupervisor::new(bus.clone());
            app.manage(std::sync::Arc::new(AppState {
                settings: Mutex::new(settings),
                config_dir,
                bus: bus.clone(),
                devices: Mutex::new(HashMap::new()),
                active_import: Mutex::new(None),
                supervisor,
            }));

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
                            .state::<ipc::SharedState>()
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
        // 铁律（用户定规矩，2026-09-18）：耗时的操作一律不准在 UI/主线程做。
        // Tauri 同步命令跑在主线程（tao 事件循环），任何磁盘 IO / WPD COM /
        // 网络(SMB/NAS) / 大结果集 DB 查询 / 哈希计算都会冻结窗口——
        // 新命令**默认 async + spawn_blocking**（ipc::run_blocking 壳），
        // 除非能证明纯内存/原子操作（现存豁免：settings_get、settings_set
        // 【用户规定：设置需同步生效确认，本地小文件写不值得异步】、
        // device_list、import_pause、import_cancel）。此清单是新命令评审的
        // 准入模板。
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
            ipc::thumb::thumb_get,
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
            let bus = app.state::<ipc::SharedState>().bus.clone();
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
                        // 移除与注册同一规范化 key（WPD 大小写变体不漏删）。
                        // 摘出后不在本线程 drop：源可能持有 COM 资源，
                        // Release 转移到专用 MTA 线程（断开闪退修复 2026-09-18）。
                        let key = devices::normalize_device_id(&id);
                        let removed = app
                            .state::<ipc::SharedState>()
                            .devices
                            .lock()
                            .expect("devices mutex poisoned")
                            .remove(&key);
                        if removed.is_some() {
                            eprintln!("设备已移除（注册表清理；WPD 代理无 COM 资源，就地 drop 安全）: {key}");
                        }
                    }
                    _ => {}
                }
            }
        })
        .expect("spawn device orchestrator");
}

/// 到达处理入口：**扫描放后台线程**（2349 文件 WPD 枚举数十秒——在编排
/// 线程串行会卡掉后续到达/移除事件，前端也迟迟看不到响应），本函数瞬间
/// 返回；注册/DeviceScanned 在扫描线程完成（幂等：重复到达同 key 覆盖刷新）。
fn handle_device_arrived(app: &AppHandle, id: String, kind: SourceKind, name: String) {
    let app = app.clone();
    let supervisor = std::sync::Arc::clone(&app.state::<ipc::SharedState>().supervisor);
    supervisor.spawn("scan", name.clone(), move |_| {
        handle_device_arrived_blocking(&app, id, kind, name);
    });
}

fn handle_device_arrived_blocking(app: &AppHandle, id: String, kind: SourceKind, name: String) {
    // 失败日志必须带 id/kind 上下文（2026-09-18 排查教训：静默吞错全靠猜）。
    // id 一律先规范化（WPD 大小写变体收敛为注册表单一 key；卷/FOLDER 原样）
    let id = devices::normalize_device_id(&id);
    let kind_tag = format!("{kind:?}");
    let state = app.state::<ipc::SharedState>();
    let source: Arc<dyn DeviceSource> = match kind {
        // 卷事件 id 形如 "E:"，根路径必须补尾反斜杠（"E:" 是该盘当前目录）
        SourceKind::Volume => Arc::new(VolumeSource::new(format!("{id}\\"))),
        SourceKind::Folder => {
            // 文件夹源不经热插拔/启动枚举产生（仅 folder_scan 注册），忽略
            return;
        }
        SourceKind::Mtp => match devices::wpd::enumerate_mtp_devices() {
            Ok(list) => match list
                .into_iter()
                .find(|(pnp, _)| devices::normalize_device_id(pnp) == id)
            {
                Some((pnp, friendly)) => Arc::new(WpdSource::new(pnp, friendly)),
                None => {
                    // 枚举空窗期（接口重枚举/会话互斥）可能暂时列不出设备：
                    // 已注册的同设备重复到达按幂等忽略，不动既有条目、不报错
                    if ipc::device_registered(&state, &id) {
                        eprintln!("WPD 设备重复到达（已在库，接口重枚举），忽略: {name} ({id})");
                    } else {
                        eprintln!(
                            "设备到达处理失败 kind={kind_tag} id={id}: WPD 枚举未找到该设备，忽略"
                        );
                    }
                    return;
                }
            },
            Err(err) => {
                if ipc::device_registered(&state, &id) {
                    eprintln!("WPD 设备重复到达（已在库，枚举暂失败），忽略: {name} ({id}): {err}");
                } else {
                    eprintln!(
                        "设备到达处理失败 kind={kind_tag} id={id}: WPD 枚举失败（相机未切 PC 模式？）: {err}"
                    );
                }
                return;
            }
        },
    };

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
    let replaced = state
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
    drop(replaced); // 覆盖刷新：旧条目（纯数据代理）就地释放安全
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
        app.state::<ipc::SharedState>()
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
            let bus = app.state::<ipc::SharedState>().bus.clone();
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
