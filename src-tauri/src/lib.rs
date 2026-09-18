mod ipc;
pub mod settings;
mod tray;

use std::sync::Mutex;

use tauri::Manager;

use crate::ipc::AppState;
use crate::settings::{Settings, SettingsManager};

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
            // 存储分离（用户规定）：应用数据（settings/DB/缓存/日志）固定在 I:\SmartPhoto，
            // 与照片库根（如 Y:\照片）彻底分开。库根由用户在引导向导中配置。
            let config_dir = std::path::PathBuf::from(r"I:\SmartPhoto");
            std::fs::create_dir_all(&config_dir)?;
            let settings = SettingsManager::load(&config_dir).unwrap_or_else(|err| {
                eprintln!("failed to load settings, falling back to defaults: {err}");
                Settings::default()
            });
            app.manage(AppState {
                settings: Mutex::new(settings),
                config_dir,
            });

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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
