//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。
//! settings_set 附加两件事（2026-09-20）：① AI 索引参数投影到推理层
//! （embed 输入档位/人脸阈值即时生效）；② 参数指纹比对——变了就后台
//! 重建对应通道（改参数即自动重建，设置页手动按钮是兜底入口）。
//!
//! 2026-10-09 设置独立改造：落盘拆两层——onboarding_completed 写全局
//! 文件，七组偏好写**激活库 db_dir** 的库级文件（每库一份，无激活库 →
//! Err("尚未创建数据库")）；副作用投影抽成 [`apply_runtime_projection`]
//! 共享函数，database_create/switch/remove 换激活库后按新库重投影。
//!
//! 2026-10-09 多数据库修正：settings.json 的数据库注册表（databases /
//! activeDatabaseId）由 `database_*` 命令族独占维护——settings_set 对这两
//! 个字段**原样保留后端真值**（前端回显值不信任），杜绝设置保存路径绕开
//! database_create/switch/remove 的校验闸门。旧单库 databaseDir 键退役。

use tauri::{AppHandle, Emitter, State};

use super::{AppState, SharedState};
use crate::settings::{Settings, SettingsManager};

/// 读取当前设置（内存合成快照）。
#[tauri::command]
pub fn settings_get(state: State<SharedState>) -> Settings {
    fetch_settings_get(&state)
}

/// settings_get 核（测试直测用；无 IO，豁免后台线程铁律）。
pub fn fetch_settings_get(state: &AppState) -> Settings {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone()
}

/// 设置副作用运行时投影（settings_set / database_create/switch/remove
/// 换激活库后共用，设置独立改造抽出）：thumbs 缓存上限、AI 推理参数
/// （embed 档位/人脸阈值/GPU 开关/画质档位）、选片 blur/eyes 阈值全部
/// 按传入合成快照重投影——切库后绝不沿用上一个库的参数。
pub fn apply_runtime_projection(state: &AppState, settings: &Settings) {
    // 缩略图缓存上限即时生效（M8-③）
    crate::thumbs::set_thumb_cache_cap_bytes(
        u64::from(settings.storage.thumb_cache_max_gb) * 1024 * 1024 * 1024,
    );
    // AI 推理参数投影（推理层即时读新值；use_gpu 关掉时新会话回落纯 CPU——
    // 存量会话重启后生效，v1 不做会话驱逐；quality_tier 切档后推理层惰性
    // 重建对应档位的会话/模型件。模型存储/下载是应用级的，不经此处）
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
}

/// 库级偏好落盘共用（settings_set / watch_folder_add/remove）：把内存
/// 合成快照的七组偏好原子写进**激活库** db_dir 的库级文件；无激活库 →
/// Err("尚未创建数据库")。调用方先改好内存快照再调本函数。
pub(crate) fn save_active_database_settings(state: &AppState) -> Result<(), String> {
    let snapshot = fetch_settings_get(state);
    let db_dir = snapshot.active_database_dir(&state.config_dir)?;
    let (_, database) = snapshot.split();
    SettingsManager::save_database(&database, &db_dir).map_err(|e| e.to_string())
}

/// 保存设置核（settings_set；不广播——命令壳负责 emit，测试直测用）：
/// 校验 → 注册表真值还原 → 拆层落盘（onboarding → 全局文件，七组偏好 →
/// 激活库库级文件）→ 副作用投影 → 参数指纹比对后台重建 → 内存快照换新。
/// 收 `&SharedState`（指纹比对 spawn 需要 'static 的状态句柄）。
pub fn fetch_settings_set(state: &SharedState, mut settings: Settings) -> Result<(), String> {
    // 画质档位硬校验（2026-09-28 三档画质）：档位驱动模型件选择与指纹
    // 重建，脏值拒绝落盘（读取侧另有 load 兜底，双保险）。
    crate::settings::validate_ai_settings(&settings.ai)?;
    let previous = fetch_settings_get(state);
    // 数据库注册表真值保留（多数据库修正）：databases/activeDatabaseId 只经
    // database_* 命令族变更（带 db_dir 互斥/照片库 root 重叠/切换校验），
    // settings_set 回收前端的回显值并还原后端真值。
    settings.databases = previous.databases;
    settings.active_database_id = previous.active_database_id;
    // 拆层落盘（设置独立改造）：全局文件只收 onboarding + 注册表；七组
    // 偏好落激活库 db_dir 的库级文件——无激活库无处安放，明确报错。
    let db_dir = settings.active_database_dir(&state.config_dir)?;
    let (global, database) = settings.split();
    SettingsManager::save_global(&global, &state.config_dir).map_err(|err| err.to_string())?;
    SettingsManager::save_database(&database, &db_dir).map_err(|err| err.to_string())?;
    // 副作用投影（与切库共用；见 apply_runtime_projection）
    apply_runtime_projection(state, &settings);
    // 参数指纹比对：变更通道后台自动重建（无变更为 no-op）。目标 = 激活
    // 数据库（多数据库修正按 activeDatabaseId 解析）；迟到任务比对内存
    // 快照防重复重建。
    let ai_snapshot = settings.ai.clone();
    let rebuild_db_dir = db_dir;
    *state.settings.lock().expect("settings mutex poisoned") = settings;
    let shared = std::sync::Arc::clone(state);
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
    Ok(())
}

/// 保存设置到磁盘，更新内存快照，并广播 `settings://changed`。
#[tauri::command]
pub fn settings_set(
    app: AppHandle,
    state: State<SharedState>,
    settings: Settings,
) -> Result<(), String> {
    fetch_settings_set(state.inner(), settings)?;
    // 广播内存真值快照（fetch_settings_set 已还原注册表真值并写入）。
    let composed = fetch_settings_get(&state);
    app.emit("settings://changed", &composed)
        .map_err(|err| err.to_string())
}
