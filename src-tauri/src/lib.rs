mod ai;
mod bursts;
mod db;
mod devices;
mod edit;
mod events;
mod geo;
mod import;
mod index;
mod ipc;
mod metadata;
mod migrate;
mod platform;
pub mod settings;
mod tasks;
mod tethering;
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
    let app = tauri::Builder::default()
        // single-instance 必须第一个注册：二次启动时显示并聚焦已有主窗口。
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
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
            // 启动画面安全网（2026-09-29）：主窗 visible=false 由前端 reveal
            // （lib/windowReveal）；前端极端卡死时 8s 后强制显示，避免不可见窗口
            {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(8));
                    if let (Some(main), Some(splash)) = (
                        handle.get_webview_window("main"),
                        handle.get_webview_window("splash"),
                    ) {
                        let _ = main.show();
                        let _ = splash.close();
                    }
                });
            }
            // 联拍 libgphoto2 随包目录注入：Tauri 资源根（Windows 与 exe 同
            // 目录，macOS 为 Contents/Resources）——覆盖所有平台的安装布局。
            if std::env::var_os("PHOTO_HUB_GPHOTO_DLL").is_none() {
                if let Ok(resource_dir) = app.path().resource_dir() {
                    let bundled = resource_dir
                        .join("gphoto")
                        .join(tethering::gphoto_backend::bundle_dll_name());
                    if bundled.is_file() {
                        std::env::set_var("PHOTO_HUB_GPHOTO_DLL", &bundled);
                    }
                }
            }
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
            // settings/ai 在 manage(move) 前先取启动自愈所需快照
            let (enable_clip, enable_face) = (settings.ai.enable_clip, settings.ai.enable_face);
            let burst_params = crate::bursts::BurstParams::from_settings(&settings.ai);
            // 缩略图缓存 LRU 上限（M8-③；0 = 不限）
            thumbs::set_thumb_cache_cap_bytes(
                u64::from(settings.storage.thumb_cache_max_gb) * 1024 * 1024 * 1024,
            );
            // AI 索引参数投影到推理层（embed 输入档位 / 人脸阈值 / GPU 开关 /
            // 画质档位——三档画质 2026-09-28）
            ai.set_ai_params(crate::ai::AiIndexParams {
                embed_input_size: settings.ai.embed_input_size,
                face_detect_threshold: settings.ai.face_detect_threshold,
                face_cluster_threshold: settings.ai.face_cluster_threshold,
                use_gpu: settings.ai.use_gpu,
                quality_tier: crate::ai::QualityTier::from_setting(&settings.ai.quality_tier)
                    .unwrap_or_default(),
            });
            // 选片分析参数快照（blur 软阈值 + eyes EAR 阈值 worker 侧读取，0021）
            crate::ai::selection::set_blur_soft_threshold(settings.ai.blur_soft_threshold);
            crate::ai::selection::set_eyes_ear_thresholds(
                settings.ai.eyes_ear_closed,
                settings.ai.eyes_ear_maybe,
            );
            let ai_for_kick = ai.clone();
            let ai_settings_snapshot = settings.ai.clone();
            app.manage(std::sync::Arc::new(AppState {
                settings: Mutex::new(settings),
                config_dir,
                bus: bus.clone(),
                devices: Mutex::new(HashMap::new()),
                active_import: Mutex::new(None),
                import_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                supervisor,
                thumb_queue: ipc::thumb::ThumbQueue::new(),
                migrations: Mutex::new(std::collections::HashSet::new()),
                ai,
            }));

            // 索引任务启动恢复（导入/索引分离）：遗留 running 复位 pending
            // → indexTaskResumed 事件 → 后台 worker 全核续跑。
            // 真机修复（2026-09-19）：AI 回填原只挂在「下载完成 watcher +
            // 导入收尾」，存量资产在模型就位前导入则永远无人补触发——启动
            // 即自愈（幂等：回填只处理 *_indexed_at IS NULL）。
            if let Some(db_dir) = active_db_dir {
                index::resume_and_kick(db_dir.clone(), &bus, &supervisor_handle);
                // 拍摄地图：地理数据包就绪则后台跑索引管线（加载/入库/回填），
                // 幂等（下载接力或 NotInstalled 之外的 phase 不重入）
                geo::backfill::ensure_backfill(db_dir.clone(), &bus, &supervisor_handle);
                // AI 空闲卸载看护（内存审计 2026-09-29）：模型/向量索引
                // 空闲 10 分钟统一释放，下次使用惰性重载
                ai::idle::spawn_idle_unloader(&supervisor_handle);
                // 孤儿导入任务终老（2026-09-21）：进程重启后引擎会话清零，
                // 遗留 running/paused 是跨会话死任务（任务抽屉挂死 +
                // import_job_delete 拒删）。统一转 cancelled + 日志；
                // journal 保留，设备回连后 resume 仍可手动续传。
                {
                    let db_dir = db_dir.clone();
                    supervisor_handle.spawn("import", "orphan-jobs-reap".into(), move |_| {
                        if let Ok(db) = ipc::open_library_db(&db_dir) {
                            match db.reap_orphan_import_jobs() {
                                Ok(reaped) => {
                                    for id in reaped {
                                        eprintln!("启动自愈：孤儿导入任务 {id} 已终老为 cancelled");
                                    }
                                }
                                Err(e) => eprintln!("启动自愈：孤儿任务清理失败: {e}"),
                            }
                        }
                    });
                }
                // RAW 缩略图源代际升级自愈（v1 取第一段小预览 → v2 取最大段）：
                // 一次性重排 RAW thumb 任务重建（dbDir 标记文件防重入）
                index::refresh_raw_thumbs_for_generation(db_dir.clone(), &bus, &supervisor_handle);
                // EXIF 深提取代际自愈（gen-2 / migration 0008）：存量资产
                // 补齐方向/闪光/GPS 等 10 列（dbDir 标记文件防重入）
                index::refresh_exif_for_generation(db_dir.clone(), &bus, &supervisor_handle);
                // 哈希补算代际自愈（gen-1 / migration 0014）：xxh=0 哨兵补齐
                index::refresh_hash_for_generation(db_dir.clone(), &bus, &supervisor_handle);
                // 选片分析代际自愈（gen-1 / migration 0021）：eyes/blur 任务
                // 账建档（闭眼模型未收录→eyes 通道跳过并计数；blur 始终可用）
                index::refresh_selection_for_generation(db_dir.clone(), &bus, &supervisor_handle);
                // 闭眼回填钩子（模型未收录恒早退；收录后自动续跑）
                ai::selection::kick_eyes_if_ready(
                    db_dir.clone(),
                    &ai_for_kick,
                    &bus,
                    &supervisor_handle,
                );
                // 缩略图缓存 LRU：启动扫一次（超限后台淘汰最旧）
                thumbs::kick_startup_evict(db_dir.clone());
                // pHash 代际自愈（gen-1 / migration 0012）：存量资产补算
                // pHash + 完成后连拍重组
                index::refresh_phash_for_generation(
                    db_dir.clone(),
                    &bus,
                    &supervisor_handle,
                    burst_params,
                );
                if enable_clip {
                    ai::semantic::kick_semantic_if_ready(
                        db_dir.clone(),
                        &ai_for_kick,
                        &bus,
                        &supervisor_handle,
                    );
                }
                if enable_face {
                    ai::face::kick_face_if_ready(
                        db_dir.clone(),
                        &ai_for_kick,
                        &bus,
                        &supervisor_handle,
                    );
                }
                // AI 索引参数指纹比对（用户 2026-09-20：改参数自动重建对应
                // 通道；dbDir 标记防每启动重做，手动按钮走 index_rebuild）
                let shared = app.state::<ipc::SharedState>().inner().clone();
                supervisor_handle.spawn("index", "params-fingerprint-check".into(), move |_| {
                    ipc::indexing::check_params_and_rebuild(
                        &shared,
                        &db_dir,
                        &ai_settings_snapshot,
                    );
                });
            }

            // 后台线程 1：领域事件转发（bus → 前端 `app://event`）
            spawn_event_forwarder(app.handle().clone(), bus.clone(), supervisor_handle.clone());
            // 后台线程 2：设备编排（热插拔 → 建源 → 扫描 → 注册表 + DeviceScanned）
            ipc::device_manager::spawn(app.state::<ipc::SharedState>().inner().clone());
            // 后台线程 4：热插拔检测（隐藏顶层窗口泵）
            let monitor = hotplug::start_if_supported(bus.clone());
            app.manage(Mutex::new(monitor));
            // 后台线程 5：监视文件夹轮询（F4 v1：5min 一轮，新文件自动入册）
            ipc::watch::spawn_watch_worker(
                app.state::<ipc::SharedState>().inner().clone(),
                &bus,
                &supervisor_handle,
            );

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
            ipc::map::map_geo_status,
            ipc::map::map_geo_download_start,
            ipc::map::map_geo_cache_url,
            ipc::map::map_clusters,
            ipc::settings::settings_set,
            ipc::settings::library_delete,
            ipc::settings::library_relocate,
            ipc::device::device_list,
            ipc::device::device_scan,
            ipc::device::device_files,
            ipc::device::folder_scan,
            ipc::device::fs_list_dirs,
            ipc::device::platform_capabilities,
            ipc::import::import_start,
            ipc::import::import_pause,
            ipc::import::import_resume,
            ipc::import::import_cancel,
            ipc::import::import_jobs_page,
            ipc::import::import_logs_page,
            ipc::import::import_job_delete,
            ipc::import::import_retry_failed,
            ipc::import::clean_candidates,
            ipc::import::clean_apply,
            ipc::assets::assets_page,
            ipc::assets::assets_count,
            ipc::assets::asset_group_dates,
            ipc::assets::asset_detail,
            edit::metadata::asset_metadata_get,
            edit::metadata::asset_metadata_save,
            ipc::assets::camera_list,
            ipc::assets::burst_stats,
            ipc::insights::on_this_day,
            ipc::insights::gear_stats,
            ipc::insights::sidebar_counts,
            ipc::duplicates::duplicates_list,
            ipc::assets::lens_list,
            ipc::assets::format_list,
            ipc::assets::assets_by_ids,
            ipc::thumb::asset_thumb_get,
            ipc::thumb::thumb_get_by_path,
            ipc::thumb::device_thumb_get,
            ipc::system::open_with_system,
            ipc::system::clipboard_copy_files,
            ipc::system::reveal_in_explorer,
            ipc::migrate::db_dir_migrate,
            ipc::migrate::photo_root_switch,
            ipc::ai::ai_models_status,
            ipc::ai::ai_model_download,
            ipc::ai::ai_model_cancel,
            ipc::ai::ai_model_delete,
            ipc::ai::search_semantic,
            ipc::ai::ai_face_data_clear,
            ipc::people::people_list,
            ipc::people::people_assets,
            ipc::people::person_rename,
            ipc::people::person_delete,
            ipc::album::album_list,
            ipc::album::album_create,
            ipc::album::album_rename,
            ipc::album::album_delete,
            ipc::album::album_cover_set,
            ipc::album::album_add_assets,
            ipc::album::album_subgroups,
            ipc::album::album_item_move_subgroup,
            ipc::album::album_assets_page,
            ipc::album::asset_albums,
            ipc::album::album_dir_rename,
            ipc::claim::album_claim_assets,
            ipc::indexing::index_kick_now,
            ipc::indexing::index_task_pause,
            ipc::indexing::index_task_resume,
            ipc::indexing::index_status,
            ipc::indexing::index_rebuild,
            ipc::rating::asset_rating_set,
            ipc::rating::asset_flag_set,
            ipc::rating::recent_assets,
            ipc::rating::asset_view_mark,
            ipc::rating::recent_viewed,
            ipc::selection::asset_label_set,
            ipc::selection::asset_reject_set,
            ipc::selection::asset_trash_move,
            ipc::selection::trash_list,
            ipc::selection::trash_restore,
            ipc::selection::trash_purge,
            ipc::selection::assets_purge_missing,
            ipc::versions::asset_versions,
            edit::ipc::edit_recipe_get,
            edit::ipc::edit_recipe_save,
            edit::ipc::edit_recipe_delete,
            edit::ipc::export_run,
            ipc::watch::watch_folders_list,
            ipc::watch::watch_folder_add,
            ipc::watch::watch_folder_remove,
            ipc::device::event_ping,
            // 联机拍摄（阶段 E-1；tethering_ 前缀原因见 ipc/tethering.rs——
            // camera_list 已被相机型号统计占用，Tauri 命令名全局唯一）
            ipc::tethering::tethering_camera_list,
            ipc::tethering::camera_probe,
            ipc::tethering::camera_capture,
            ipc::tethering::tethering_start,
            ipc::tethering::tethering_session,
            ipc::tethering::tethering_settings,
            ipc::tethering::tethering_setting_set,
            ipc::tethering::tethering_focus_at,
            ipc::tethering::tethering_capture,
            ipc::tethering::tethering_frame,
            ipc::tethering::tethering_photo_preview,
            ipc::tethering::tethering_stop,
            // 选片会话（0024 V1）：会话持久化 + 决定读写 + 收尾映射
            ipc::culling::cull_session_create,
            ipc::culling::cull_session_list,
            ipc::culling::cull_session_open,
            ipc::culling::cull_decision_apply,
            ipc::culling::cull_session_rename,
            ipc::culling::cull_session_discard,
            ipc::culling::cull_session_finish,
            // AI 挑图预扫（V3）：规则引擎预标记建议（只建议不自动决定）
            ipc::culling::cull_ai_prescan,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");
    app.run(stop_hotplug_on_exit);
}

fn stop_hotplug_on_exit(app: &AppHandle, event: tauri::RunEvent) {
    if !matches!(event, tauri::RunEvent::Exit) {
        return;
    }
    let Some(monitor) = app.try_state::<Mutex<Option<hotplug::HotplugHandle>>>() else {
        return;
    };
    let handle = monitor
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take();
    if let Some(handle) = handle {
        hotplug::stop(handle);
    }
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

use crate::settings::{Settings, SettingsManager};
