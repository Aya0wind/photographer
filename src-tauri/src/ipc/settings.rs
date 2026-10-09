//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。
//! settings_set 附加两件事（2026-09-20）：① AI 索引参数投影到推理层
//! （embed 输入档位/人脸阈值即时生效）；② 参数指纹比对——变了就后台
//! 重建对应通道（改参数即自动重建，设置页手动按钮是兜底入口）。
//!
//! 2026-10-09 单数据库多照片库（M2a）：settings.json 只留应用级设置；
//! 旧 library_delete / library_relocate 命令随库注册表退役（照片库登记
//! 改由 photo_library_* 命令族承载，见 `ipc::photo_library`）。

use tauri::{AppHandle, Emitter, State};

use super::SharedState;
use crate::settings::{Settings, SettingsManager};

/// 读取当前设置（内存快照）。
#[tauri::command]
pub fn settings_get(state: State<SharedState>) -> Settings {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone()
}

/// 保存设置到磁盘，更新内存快照，并广播 `settings://changed`。
#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    state: State<SharedState>,
    mut settings: Settings,
) -> Result<(), String> {
    // 画质档位硬校验（2026-09-28 三档画质）：档位驱动模型件选择与指纹
    // 重建，脏值拒绝落盘（读取侧另有 load 兜底，双保险）。
    crate::settings::validate_ai_settings(&settings.ai)?;
    let previous = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    // 「数据库位置」规范化（2026-10-09 单数据库多照片库 §一）：Some 走
    // 绝对路径归一 + 盘根拒绝；None（默认应用数据目录）原样通过。
    crate::settings::normalize_settings_database_dir(&mut settings)?;
    // 数据库位置变更的反向互斥（§八-6）：新数据库目录不得与已登记照片库
    // root 相同或互相包含（防 thumbs/ 等数据件落进照片库被扫描登记）。
    // 打开的是**新设置解析出的**数据库目录——photos_libraries 登记表
    // 随库就位（尚未建库时为空表，校验自然通过）。
    if settings.database_dir != previous.database_dir {
        let database_dir = settings.database_dir_path(&state.config_dir);
        let db =
            super::open_library_db(&database_dir).map_err(|e| format!("数据库位置不可用：{e}"))?;
        let libraries = db.photos_library_list().map_err(|e| e.to_string())?;
        for library in &libraries {
            crate::db::libraries::validate_photos_library_root(
                &[],
                &database_dir,
                std::path::Path::new(&library.root_path),
            )
            .map_err(|_| {
                format!(
                    "数据库位置与照片库「{}」的根目录相同或互相包含（{}）",
                    library.name, library.root_path
                )
            })?;
        }
    }
    SettingsManager::save(&settings, &state.config_dir).map_err(|err| err.to_string())?;
    // 缩略图缓存上限即时生效（M8-③）
    crate::thumbs::set_thumb_cache_cap_bytes(
        u64::from(settings.storage.thumb_cache_max_gb) * 1024 * 1024 * 1024,
    );
    // AI 索引参数投影（推理层即时读新值；use_gpu 关掉时新会话回落纯 CPU——
    // 存量会话重启后生效，v1 不做会话驱逐；quality_tier 切档后推理层惰性
    // 重建对应档位的会话/模型件）
    state.ai.set_ai_params(crate::ai::AiIndexParams {
        embed_input_size: settings.ai.embed_input_size,
        face_detect_threshold: settings.ai.face_detect_threshold,
        face_cluster_threshold: settings.ai.face_cluster_threshold,
        use_gpu: settings.ai.use_gpu,
        quality_tier: crate::ai::QualityTier::from_setting(&settings.ai.quality_tier)
            .unwrap_or_default(),
    });
    // 选片分析参数快照刷新（blur 软阈值 + eyes EAR 阈值 worker 侧即时读
    // 新值，0021；EAR 阈值变更经 selection 指纹重排 eyes 任务）
    crate::ai::selection::set_blur_soft_threshold(settings.ai.blur_soft_threshold);
    crate::ai::selection_regions::set_include_single(settings.ai.eyes_include_single);
    crate::ai::selection::set_eyes_ear_thresholds(
        settings.ai.eyes_ear_closed,
        settings.ai.eyes_ear_maybe,
    );
    // 参数指纹比对：变更通道后台自动重建（无变更为 no-op）。应用唯一
    // 数据库（§一）随设置解析；迟到任务比对内存快照防重复重建。
    let ai_snapshot = settings.ai.clone();
    let rebuild_db_dir = settings.database_dir_path(&state.config_dir);
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    let shared = state.inner().clone();
    state
        .supervisor
        .spawn("index", "params-fingerprint-check".into(), move |_| {
            {
                let current = shared.settings.lock().expect("settings mutex poisoned");
                if current.ai != ai_snapshot {
                    return; // 再次修改后的迟到任务不可重建。
                }
            }
            super::indexing::check_params_and_rebuild(&shared, &rebuild_db_dir, &ai_snapshot);
        });
    app.emit("settings://changed", &settings)
        .map_err(|err| err.to_string())?;
    Ok(())
}
