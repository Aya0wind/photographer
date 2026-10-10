//! 数据库命令族（2026-10-09 多数据库修正）。
//!
//! 层级：应用 → 数据库（可多个、可切换，为多用户协作预埋）→ 照片库
//!（文件夹登记，photo_library_* 命令族，全部作用于激活数据库内部）。
//! 每个数据库 = 独立 SQLite(library.db) + thumbs/ + 向量，落在各自的
//! db_dir（注册表存 settings.databases）；不同数据库的照片库天然隔离。
//!
//! 命令：database_list / database_create / database_switch /
//! database_remove（两级：仅摘登记 / 连数据目录删）。变更统一广播
//! `AppEvent::DatabasesChanged`——前端收尾后全量刷新库内数据（换库语义）。
//!
//! 铁律：磁盘 IO / 开库查询一律 `run_blocking` 后台线程；**绝不删除
//! 照片库文件夹**（数据目录删除前的照片库 root 重叠校验是硬闸，用户红线）。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};

use super::{run_blocking, AppState, SharedState};
use crate::events::AppEvent;
use crate::settings::{
    DatabaseEntry, DatabaseSettings, GlobalSettings, Settings, SettingsManager,
};

/// 数据库注册表快照 DTO（database_list 返回；databases + 激活 id 同帧
/// 返回，避免两次读取间被切换造成的前后不一致，camelCase 与前端
/// `DatabaseList` 契约对齐）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseListDto {
    pub databases: Vec<DatabaseEntry>,
    /// 激活数据库 id（null=尚未创建数据库）。
    pub active_id: Option<String>,
}

/// database_remove 结果（camelCase）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseRemoveResult {
    /// delete_data=true 时实际删除的数据目录条目（此处恒为 1；摘登记
    /// 模式为 0）。
    pub data_dirs_deleted: u32,
}

/// 默认数据库目录约定（database_create 的 db_dir 缺省值）：
/// `<app_config_dir>/databases/<id>`——同库模型 photo_library 的约定式
/// 布局，用户自选位置（Windows 数据盘）走显式 dbDir 覆盖。
fn default_db_dir(app_config_dir: &Path, id: &str) -> PathBuf {
    app_config_dir.join("databases").join(id)
}

/// 数据库名称归一：trim 后空串拒绝（错误文案直接面向用户）。
fn validate_name(raw: &str) -> Result<String, String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err("数据库名称不能为空".into());
    }
    Ok(name.to_string())
}

/// 持久化注册表变更的共用收尾：全局文件只落 GlobalSettings 形态（设置
/// 独立改造——不再写全量，db 级键回流全局文件会反复触发拆层迁移），
/// 内存合成快照整体换新。调用方持 settings 锁外调用（本函数自己拿锁）。
fn save_registry(state: &AppState, settings: Settings) -> Result<(), String> {
    let (global, _) = settings.split();
    SettingsManager::save_global(&global, &state.config_dir).map_err(|e| e.to_string())?;
    *state.settings.lock().expect("settings mutex poisoned") = settings;
    Ok(())
}

/// 换激活库后的设置收尾（create/switch/remove 共用，设置独立改造）：
/// 按传入全局真值 + **新激活库**的库级文件重算合成快照 → 落全局文件与
/// 内存 → [`apply_runtime_projection`] 按新库偏好重投影（防沿用上一个
/// 库的 AI/thumbs/选片参数）。无激活库 = 七组偏好全默认合成。
fn reload_active_preferences(state: &AppState, global: GlobalSettings) -> Result<(), String> {
    let database = match global.active_database_dir() {
        Some(db_dir) => SettingsManager::load_database(&db_dir).map_err(|e| e.to_string())?,
        None => DatabaseSettings::default(),
    };
    let composed = Settings::compose(&global, &database);
    save_registry(state, composed.clone())?;
    super::settings::apply_runtime_projection(state, &composed);
    Ok(())
}

/// 激活库变更后的设置重载广播（settings://changed，负载 = 合成快照）：
/// 偏好按库独立后，建库/切库/删库都让 settings_get 换一套值，前端据此
/// 重载（与 DatabasesChanged 的数据刷新互补）。在 fetch_* 已写入
/// state.settings 之后调命令壳层。
fn emit_settings_changed(app: &AppHandle, shared: &AppState) -> Result<(), String> {
    let composed = shared
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    app.emit("settings://changed", &composed)
        .map_err(|e| e.to_string())
}

/// 遍历注册表内**每个**数据库的 photos_libraries 收集照片库 root
///（跨库校验基准）。library.db 尚不存在 → 该库视为无登记（新建前/被
/// 手动清理后，不物化不报错）；文件在但读不开（目录被挪走/盘离线等）→
/// 返回 None（调用方按「无法核对」处理，宁可拒绝不可漏删）。
fn collect_all_photo_roots(
    state: &AppState,
) -> Result<Option<Vec<(String, String)>>, String> {
    let databases = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .databases
        .clone();
    let mut roots = Vec::new();
    for entry in &databases {
        let db_dir = Path::new(&entry.db_dir);
        if !db_dir.join("library.db").is_file() {
            continue; // 从未物化/已被清理：无照片库登记可核对
        }
        let db = match super::open_library_db(db_dir) {
            Ok(db) => db,
            // 读不开：无法核对重叠，返回 None 让调用方拒绝危险动作。
            Err(_) => return Ok(None),
        };
        for library in db.photos_library_list().map_err(|e| e.to_string())? {
            roots.push((library.name, library.root_path));
        }
    }
    Ok(Some(roots))
}

// ---------------------------------------------------------------------------
// database_list
// ---------------------------------------------------------------------------

/// 注册表快照核（database_list）：settings 内存真值投影（同帧带激活 id）。
pub fn fetch_database_list(state: &AppState) -> Result<DatabaseListDto, String> {
    let settings = state.settings.lock().expect("settings mutex poisoned");
    Ok(DatabaseListDto {
        databases: settings.databases.clone(),
        active_id: settings.active_database_id.clone(),
    })
}

/// 数据库注册表快照（database_list；纯内存读，豁免后台线程铁律）。
#[tauri::command]
pub fn database_list(state: State<'_, SharedState>) -> Result<DatabaseListDto, String> {
    fetch_database_list(&state)
}

// ---------------------------------------------------------------------------
// database_create
// ---------------------------------------------------------------------------

/// 新建数据库核（database_create）：
/// ① 名称/路径闸门——dbDir 缺省走约定路径（`<配置目录>/databases/<id>`），
///    显式给出则 normalize_database_path（绝对路径 + 盘根拒绝）；
/// ② 注册表内互斥（settings::validate_database_dir_overlap——db_dir 之间
///    不得相同或互相包含）；
/// ③ 跨库照片库重叠校验（IPC 层开库查）：新 db_dir 不得与**任何**已注册
///    数据库的照片库 root 相同或互相包含（防把照片库登记进数据目录、防
///    数据件落进照片库被扫描登记，§八-6 反向）；某库 library.db 读不开
///    时跳过该库（摘除场景同 purge 脚本语义，不阻塞新建）；
/// ④ 物化——open_library_db 建目录 + 单版本 schema（同时验证可写）；
/// ⑤ **一律激活新库**（设置独立改造，原「首个库创建即激活」推广为每次
///    创建即激活）：引导流程建第二库后，后续 AI 步骤的 settings_set 要
///    落进新库；偏好快照随之重算——新库 db_dir 无库级文件 = 全默认，
///    重新登记旧目录（摘登记后复用）则恢复当年那份偏好。
pub fn fetch_database_create(
    state: &AppState,
    name: &str,
    db_dir: Option<&str>,
) -> Result<DatabaseEntry, String> {
    let name = validate_name(name)?;
    let id = uuid::Uuid::new_v4().to_string();
    let db_dir = match db_dir {
        Some(raw) => crate::settings::normalize_database_path(raw)?,
        None => default_db_dir(&state.config_dir, &id)
            .to_string_lossy()
            .into_owned(),
    };
    let dir = PathBuf::from(&db_dir);
    {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        crate::settings::validate_database_dir_overlap(&settings.databases, &dir)?;
    }
    // 跨库照片库重叠校验：open 每个已注册库读 photos_libraries。
    if let Some(roots) = collect_all_photo_roots(state)? {
        for (library_name, root) in &roots {
            crate::db::libraries::validate_photos_library_root(&[], &dir, Path::new(root))
                .map_err(|_| {
                    format!(
                        "数据库位置与照片库「{library_name}」的根目录相同或互相包含（{root}）"
                    )
                })?;
        }
    }
    // 物化（建目录 + 建表 + 验证可写）；失败不落注册表。
    super::open_library_db(&dir).map_err(|e| format!("数据库位置不可用：{e}"))?;
    let entry = DatabaseEntry { id, name, db_dir };
    let settings = {
        let mut settings = state.settings.lock().expect("settings mutex poisoned");
        settings.databases.push(entry.clone());
        settings.active_database_id = Some(entry.id.clone());
        settings.clone()
    };
    // 合成快照重算 + 重投影：注册表/激活态用内存真值，偏好取新库的库级
    // 文件（新库 db_dir 无库级文件 = 全默认；重新登记旧目录（摘登记后
    // 复用）则恢复当年那份偏好）。
    let (global, _) = settings.split();
    reload_active_preferences(state, global)?;
    Ok(entry)
}

/// 新建数据库（database_create）：名称必填 + 位置可选（缺省约定路径）；
/// 一律激活新库（见核注释）。注册表变更 + 照片库语义（新库
/// photos_libraries 空）统一走 DatabasesChanged 广播；偏好快照随库换，
/// 另发 settings://changed 让前端重载设置。
#[tauri::command]
pub async fn database_create(
    app: AppHandle,
    state: State<'_, SharedState>,
    name: String,
    db_dir: Option<String>,
) -> Result<DatabaseEntry, String> {
    let shared = state.inner().clone();
    let entry = run_blocking(shared.clone(), move |state| {
        fetch_database_create(state, &name, db_dir.as_deref())
    })
    .await?;
    app.emit("app://event", &AppEvent::DatabasesChanged)
        .map_err(|e| e.to_string())?;
    emit_settings_changed(&app, &shared)?;
    Ok(entry)
}

// ---------------------------------------------------------------------------
// database_switch
// ---------------------------------------------------------------------------

/// 切换激活数据库核（database_switch）：校验注册表存在 → 开库验证可用
///（library.db 不存在或读不开 = 目录被挪走/盘未挂载，拒绝切换保持现库）
/// → 写 active_database_id + 按新库重算合成快照/重投影（设置独立改造：
/// 偏好按库独立，切库即整套换——绝不沿用上一个库的 AI/thumbs/选片参数）。
/// 导入在场拒绝（引擎会话锚定旧库连接，切换会让 UI 与落库分家）。
pub fn fetch_database_switch(state: &AppState, id: &str) -> Result<(), String> {
    super::ensure_no_import_running(state)?;
    let settings = state.settings.lock().expect("settings mutex poisoned").clone();
    let entry = settings
        .databases
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("数据库不存在：{id}"))?;
    let db_file = Path::new(&entry.db_dir).join("library.db");
    if !db_file.is_file() {
        return Err(format!(
            "数据库文件不存在（{}），未切换",
            db_file.display()
        ));
    }
    super::open_library_db(Path::new(&entry.db_dir))
        .map_err(|e| format!("数据库不可用，未切换：{e}"))?;
    if settings.active_database_id.as_deref() == Some(id) {
        return Ok(()); // 幂等：切到当前库 no-op（不发事件由命令层判断）
    }
    let (mut global, _) = settings.split();
    global.active_database_id = Some(id.to_string());
    reload_active_preferences(state, global)
}

/// 切换激活数据库（database_switch）。变更广播 DatabasesChanged——前端
/// 收到后全量刷新（照片库列表/画廊/相册等一切库内数据）；偏好随库整套
/// 换，另发 settings://changed 让设置 store 重载（设置独立改造）。
#[tauri::command]
pub async fn database_switch(
    app: AppHandle,
    state: State<'_, SharedState>,
    id: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    let changed = run_blocking(shared.clone(), move |state| {
        let before = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_database_id
            .clone();
        fetch_database_switch(state, &id)?;
        let after = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_database_id
            .clone();
        Ok(before != after)
    })
    .await?;
    if changed {
        app.emit("app://event", &AppEvent::DatabasesChanged)
            .map_err(|e| e.to_string())?;
        emit_settings_changed(&app, &shared)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// database_remove
// ---------------------------------------------------------------------------

/// 移除数据库核（database_remove）：两级——
/// - delete_data=false：仅摘注册表登记（数据目录原样留在磁盘，之后可
///   手动清理或将来重新登记）；
/// - delete_data=true：连数据目录整棵删除（library.db/thumbs/向量）。
///   **绝不删照片库文件夹**（用户红线）：删除前收集**全部**注册数据库的
///   照片库 root（含被删库自己的——数据目录里理论上不该有照片库 root，
///   有即说明 db_dir 配置与照片库重叠，直接拒绝）；任一 library.db 读
///   不开 → 无法核对重叠 → 拒绝删除（宁可保守）。
/// 删的是激活库时：切到剩余第一个（注册表序），无剩余则 None（应用回到
/// 「尚未创建数据库」态，门禁送回引导）。导入在场拒绝（同 switch）。
pub fn fetch_database_remove(
    state: &AppState,
    id: &str,
    delete_data: bool,
) -> Result<DatabaseRemoveResult, String> {
    super::ensure_no_import_running(state)?;
    let settings = state.settings.lock().expect("settings mutex poisoned").clone();
    let entry = settings
        .databases
        .iter()
        .find(|entry| entry.id == id)
        .ok_or_else(|| format!("数据库不存在：{id}"))?
        .clone();
    let mut data_dirs_deleted = 0u32;
    if delete_data {
        let dir = PathBuf::from(&entry.db_dir);
        // 收集全部库（含被删库自身）的照片库 root 做重叠核对。
        match collect_all_photo_roots(state)? {
            Some(roots) => {
                for (library_name, root) in &roots {
                    crate::db::libraries::validate_photos_library_root(
                        &[],
                        &dir,
                        Path::new(root),
                    )
                    .map_err(|_| {
                        format!(
                            "数据库目录与照片库「{library_name}」的根目录相同或互相包含（{root}），\
                             为保护照片文件拒绝删除；请先移除该照片库登记"
                        )
                    })?;
                }
            }
            None => {
                return Err(
                    "存在读不开的数据库（library.db 无法核对照片库根目录），为保护照片文件拒绝删除"
                        .into(),
                );
            }
        }
        std::fs::remove_dir_all(&dir).map_err(|e| format!("删除数据目录失败（{}）: {e}", entry.db_dir))?;
        data_dirs_deleted = 1;
    }
    let (mut global, _) = settings.split();
    global.databases.retain(|entry| entry.id != id);
    if global.active_database_id.as_deref() == Some(id) {
        global.active_database_id = global.first_database_id().map(|s| s.to_string());
    }
    // 悬空引用清理：system.autoOpenDatabaseId 是库级偏好（落新激活库的
    // 库级文件），指向被删库 → 就地清除并回写（单库时代在快照上清，拆层
    // 后落点跟着激活库走）。
    if let Some(db_dir) = global.active_database_dir() {
        let mut database = SettingsManager::load_database(&db_dir).map_err(|e| e.to_string())?;
        if database.system.auto_open_database_id.as_deref() == Some(id) {
            database.system.auto_open_database_id = None;
            SettingsManager::save_database(&database, &db_dir).map_err(|e| e.to_string())?;
        }
    }
    reload_active_preferences(state, global)?;
    Ok(DatabaseRemoveResult { data_dirs_deleted })
}

/// 移除数据库（database_remove）：两级确认由前端对话框完成（对齐
/// photo_library_remove 形态）；变更广播 DatabasesChanged；删激活库引发
/// 的回退切换让偏好快照随库换，另发 settings://changed（设置独立改造）。
#[tauri::command]
pub async fn database_remove(
    app: AppHandle,
    state: State<'_, SharedState>,
    id: String,
    delete_data: bool,
) -> Result<DatabaseRemoveResult, String> {
    let shared = state.inner().clone();
    let result = run_blocking(shared.clone(), move |state| {
        fetch_database_remove(state, &id, delete_data)
    })
    .await?;
    app.emit("app://event", &AppEvent::DatabasesChanged)
        .map_err(|e| e.to_string())?;
    emit_settings_changed(&app, &shared)?;
    Ok(result)
}
