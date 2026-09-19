mod ai;
mod db;
mod devices;
mod events;
mod import;
mod index;
mod ipc;
mod metadata;
mod migrate;
pub mod settings;
mod tasks;
mod thumbs;
mod tray;

use std::collections::HashMap;
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::devices::hotplug;
use crate::events::{AppEvent, EventBus};
use crate::ipc::AppState;

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
            devices::diagnostics::init(&config_dir);
            let settings = SettingsManager::load(&config_dir).unwrap_or_else(|err| {
                eprintln!("failed to load settings, falling back to defaults: {err}");
                Settings::default()
            });
            // 索引任务启动恢复目标（settings 交管 AppState 前先取）
            let active_db_dir: Option<std::path::PathBuf> = settings
                .active_library()
                .map(|l| std::path::PathBuf::from(&l.db_dir));
            let bus = EventBus::new();
            // 托管 Arc<AppState>（SharedState）：async 慢命令壳需要 'static
            // clone 进 spawn_blocking 闭包（铁律：慢操作不上主线程）。
            // supervisor：统一后台任务框架（panic 捕获 + 命名 + 软取消）。
            let supervisor = tasks::TaskSupervisor::new(bus.clone());
            let supervisor_handle = std::sync::Arc::clone(&supervisor);
            let ai = ai::ModelManager::new(
                config_dir.join("models"),
                bus.clone(),
                std::sync::Arc::clone(&supervisor),
            );
            app.manage(std::sync::Arc::new(AppState {
                settings: Mutex::new(settings),
                config_dir,
                bus: bus.clone(),
                devices: Mutex::new(HashMap::new()),
                active_import: Mutex::new(None),
                supervisor,
                thumb_queue: ipc::thumb::ThumbQueue::new(),
                migrations: Mutex::new(std::collections::HashSet::new()),
                ai,
            }));

            // 索引任务启动恢复（导入/索引分离）：遗留 running 复位 pending
            // → indexTaskResumed 事件 → 后台 worker 全核续跑。
            if let Some(db_dir) = active_db_dir {
                index::resume_and_kick(db_dir, &bus, &supervisor_handle);
            }

            // 后台线程 1：领域事件转发（bus → 前端 `app://event`）
            spawn_event_forwarder(app.handle().clone(), bus.clone(), supervisor_handle.clone());
            // 后台线程 2：设备编排（热插拔 → 建源 → 扫描 → 注册表 + DeviceScanned）
            ipc::device_manager::spawn(app.state::<ipc::SharedState>().inner().clone());
            // 后台线程 3：系统通知（会话开始/结束 + 里程碑）
            spawn_notification_subscriber(app.handle().clone());
            // 后台线程 4：热插拔检测（隐藏顶层窗口泵）
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
            ipc::assets::assets_page,
            ipc::assets::asset_group_dates,
            ipc::assets::asset_detail,
            ipc::assets::camera_list,
            ipc::thumb::asset_thumb_get,
            ipc::thumb::thumb_get_by_path,
            ipc::migrate::db_dir_migrate,
            ipc::migrate::photo_root_switch,
            ipc::ai::ai_models_status,
            ipc::ai::ai_model_download,
            ipc::ai::ai_model_cancel,
            ipc::ai::ai_model_delete,
            ipc::device::event_ping,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// 事件转发：EventBus → 前端 `app://event`（唯一的前端事件通道）。
///
/// 关键时序（P0 修复，2026-09-18）：**订阅在 setup 线程同步完成**后
/// receiver 才移入任务线程——spawn 返回即可保证后续任何发布（存量枚举/
/// 热插/探活）必被转发器收到（此前订阅在新线程内执行，存在晚于首条
/// 事件发布的竞态）。转发循环带 panic 捕获自恢复（转发器死亡 = 前端
/// 全盲），每 100 条打活性心跳日志。
fn spawn_event_forwarder(
    app: AppHandle,
    bus: EventBus,
    supervisor: std::sync::Arc<tasks::TaskSupervisor>,
) {
    let mut rx = bus.subscribe(); // setup 线程内同步订阅：先于一切发布
    let emit_app = app.clone();
    let report_bus = bus.clone();
    eprintln!("事件转发已启动（app://event 通道就绪）");
    supervisor.spawn("event-forward", "app://event 转发".into(), move |_| {
        let emit: std::sync::Arc<dyn Fn(&AppEvent) + Send + Sync> =
            std::sync::Arc::new(move |event| {
                if !matches!(event, AppEvent::DeviceTopologyChanged { .. }) {
                    let _ = emit_app.emit("app://event", event);
                }
            });
        events::forward_supervised(&mut rx, emit, move |msg| {
            report_bus.publish(AppEvent::AppError {
                level: "error".into(),
                message: msg,
                recoverable: true,
            });
        });
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
