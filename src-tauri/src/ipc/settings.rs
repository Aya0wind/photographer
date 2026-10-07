//! settings 命令：`settings_get` / `settings_set`。
//!
//! 两者均为同步（用户规定 2026-09-18：设置需等待生效确认，保持同步语义；
//! 写 settings.json 为本地小文件原子写，不值得异步）。
//! settings_set 附加两件事（2026-09-20）：① AI 索引参数投影到推理层
//! （embed 输入档位/人脸阈值即时生效）；② 参数指纹比对——变了就后台
//! 重建对应通道（改参数即自动重建，设置页手动按钮是兜底入口）。

use tauri::{AppHandle, Emitter, State};

use super::{run_blocking, SharedState};
use crate::settings::{Settings, SettingsManager};

/// 删除库的物理结果（camelCase）。
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDeleteResult {
    pub db_deleted: bool,
    pub photo_root_deleted: bool,
    pub managed_files_deleted: u64,
}

/// 删除库核：db_dir 必删（数据库/缩略图/向量/标记），photo_root 可选连删。
/// 三道安全闸：①目录必须形如库数据目录（存在 library.db）；②不得为当前
/// 活跃库（前端删除前先清 activeLibraryId，这里双保险）；③照片目录拒绝
/// 盘根/与 db_dir 相同。前端的「输入库名确认」对话框是第一道用户侧闸。
pub fn fetch_library_delete(
    state: &super::AppState,
    db_dir: &str,
    photo_root: Option<&str>,
) -> Result<LibraryDeleteResult, String> {
    let db_path = std::path::PathBuf::from(db_dir);
    if db_path
        .parent()
        .is_none_or(|parent| parent.as_os_str().is_empty())
    {
        return Err("数据库目录不能是磁盘或网络共享的根目录".into());
    }
    if !db_path.join("library.db").is_file() {
        return Err(format!(
            "目录不像库数据目录（未找到 library.db），拒绝删除：{db_dir}"
        ));
    }
    if db_path.is_symlink() {
        return Err("数据库目录是链接，拒绝删除其目标".into());
    }
    for entry in std::fs::read_dir(&db_path).map_err(|e| format!("读取数据库目录失败：{e}"))?
    {
        let entry = entry.map_err(|e| format!("读取数据库目录失败：{e}"))?;
        let name = entry.file_name().to_string_lossy().to_string();
        let known = matches!(
            name.as_str(),
            "thumbs"
                | "geo"
                | "vectors.usearch"
                | "library.db"
                | "library.db-wal"
                | "library.db-shm"
                | ".db-migration.json"
                | ".db-migration.json.tmp"
                | ".migrated-bak"
                | "phash-gen-1.marker"
                | "hash-gen-1.marker"
                | "selection-gen-1.marker"
                | "exif-gen-5.marker"
                | "index-params.marker"
        ) || (name.starts_with("thumbs-raw-gen-") && name.ends_with(".marker"));
        if !known || entry.file_type().map_err(|e| e.to_string())?.is_symlink() {
            return Err(format!(
                "数据库目录中还有其他文件或链接，拒绝整体删除：{name}"
            ));
        }
    }
    // 校验输入路径，再核对注册表；任何错误都发生在实际删除前。
    if let Some(root) = photo_root {
        let root_path = std::path::PathBuf::from(root);
        let root_parent_empty = root_path
            .parent()
            .map(|p| p.as_os_str().is_empty())
            .unwrap_or(true);
        if root_parent_empty {
            return Err(format!("拒绝删除盘根/裸目录：{}", root_path.display()));
        }
        if root_path.starts_with(&db_path) || db_path.starts_with(&root_path) {
            return Err("照片目录与库数据目录相同或互相包含，拒绝删除".into());
        }
        if let (Ok(real_db), Ok(real_root)) = (db_path.canonicalize(), root_path.canonicalize()) {
            let real_db = real_db.to_string_lossy().replace('/', "\\").to_lowercase();
            let real_root = real_root
                .to_string_lossy()
                .replace('/', "\\")
                .to_lowercase();
            if real_db == real_root
                || real_db.starts_with(&(real_root.clone() + "\\"))
                || real_root.starts_with(&(real_db + "\\"))
            {
                return Err("照片目录与库数据目录实际位置重叠，拒绝删除".into());
            }
        }
    }
    {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        if let Some(active) = settings.active_library() {
            if active.db_dir.eq_ignore_ascii_case(db_dir) {
                return Err("该库为当前活跃库，请先切换/清除激活再删除".into());
            }
        }
        let Some(registered) = settings
            .libraries
            .iter()
            .find(|lib| lib.db_dir.eq_ignore_ascii_case(db_dir))
        else {
            return Err("该数据库目录未登记为库，拒绝删除".into());
        };
        if photo_root.is_some_and(|root| !registered.photo_root.eq_ignore_ascii_case(root)) {
            return Err("照片目录与已登记的库不一致，拒绝删除".into());
        }
        if let Some(root) = photo_root {
            if settings
                .libraries
                .iter()
                .any(|lib| lib.id != registered.id && lib.photo_root.eq_ignore_ascii_case(root))
            {
                return Err("照片目录仍被其他库使用，拒绝删除".into());
            }
        }
    }
    let mut result = LibraryDeleteResult {
        db_deleted: false,
        photo_root_deleted: false,
        managed_files_deleted: 0,
    };
    // The database is the ownership ledger. Never recursively delete the photo root:
    // it may contain user files or originals indexed in place (origin=external).
    if let Some(root) = photo_root {
        let root_path = std::path::PathBuf::from(root);
        let conn = rusqlite::Connection::open_with_flags(
            db_path.join("library.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|e| format!("读取库照片清单失败，未删除任何内容：{e}"))?;
        let mut stmt = conn
            .prepare("SELECT path FROM assets WHERE origin = 'imported'")
            .map_err(|e| format!("读取库照片清单失败，未删除任何内容：{e}"))?;
        let paths = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("读取库照片清单失败，未删除任何内容：{e}"))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| format!("读取库照片清单失败，未删除任何内容：{e}"))?;
        let root_canonical = std::fs::canonicalize(&root_path).ok();
        let mut empty_dirs = Vec::new();
        for path in paths {
            let file = std::path::PathBuf::from(path);
            // Canonical paths also exclude a junction/symlink that escapes photoRoot.
            let inside = match (&root_canonical, std::fs::canonicalize(&file).ok()) {
                (Some(base), Some(actual)) => {
                    #[cfg(windows)]
                    {
                        let base = base.to_string_lossy().replace('/', "\\").to_lowercase();
                        let actual = actual.to_string_lossy().replace('/', "\\").to_lowercase();
                        actual.starts_with(&(base + "\\"))
                    }
                    #[cfg(not(windows))]
                    {
                        actual.starts_with(base) && actual != *base
                    }
                }
                _ => false,
            };
            if !inside {
                continue;
            }
            if file.exists() {
                std::fs::remove_file(&file)
                    .map_err(|e| format!("删除库内照片失败（{}）：{e}", file.display()))?;
                result.managed_files_deleted += 1;
            }
            let mut parent = file.parent();
            while let Some(dir) = parent {
                if dir == root_path {
                    break;
                }
                if !dir.starts_with(&root_path) {
                    break;
                }
                empty_dirs.push(dir.to_path_buf());
                parent = dir.parent();
            }
        }
        empty_dirs.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        empty_dirs.dedup();
        for dir in empty_dirs {
            let _ = std::fs::remove_dir(dir);
        }
        result.photo_root_deleted = std::fs::remove_dir(&root_path).is_ok();
    }
    std::fs::remove_dir_all(&db_path)
        .map_err(|e| format!("删除库数据目录失败（{}）：{e}", db_path.display()))?;
    result.db_deleted = true;
    Ok(result)
}

/// 删除库（物理）：库数据目录必删；勾选连照片目录一起删。注册表移除由
/// 前端在调用前后清（先清 activeLibraryId 再删，再从 libraries 摘除）。
#[tauri::command]
pub async fn library_delete(
    state: State<'_, SharedState>,
    db_dir: String,
    photo_root: Option<String>,
) -> Result<LibraryDeleteResult, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_library_delete(state, &db_dir, photo_root.as_deref())
    })
    .await
}

/// 库重定位结果（camelCase）：affected=旧根前缀重写数；unaffected=不在
/// 旧根下（外部根/历史遗留）保持原样的数量；rootExists=新根当前是否在盘。
#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRelocateDto {
    pub affected: u64,
    pub unaffected: u64,
    pub root_exists: bool,
}

/// 库照片存储目录整体重定位（用户定案 2026-09-28「整体重定位」语义）：
/// 前提是用户已在文件管理器把整个照片树搬到新根，应用负责改配置 + 重写
/// 库内路径前缀（大小写/斜杠不敏感）+ 重排缩略图任务。apply=false 只预检
/// 返回计数。新根不必当前在盘（可先改后挂载；缺失走 missing 终态提示）。
pub fn fetch_library_relocate(
    state: &super::AppState,
    library_id: &str,
    new_photo_root: &str,
    apply: bool,
) -> Result<LibraryRelocateDto, String> {
    let normalized = crate::settings::normalize_library_path(new_photo_root)?;
    let root_exists = std::path::Path::new(&normalized).is_dir();
    let (db_dir, old_root) = {
        let settings = state.settings.lock().expect("settings mutex poisoned");
        let lib = settings
            .libraries
            .iter()
            .find(|l| l.id == library_id)
            .ok_or_else(|| format!("库不存在：{library_id}"))?;
        (lib.db_dir.clone(), lib.photo_root.clone())
    };
    let same_root = normalized.eq_ignore_ascii_case(&old_root);
    let db = super::open_library_db(std::path::Path::new(&db_dir))?;
    let (affected, unaffected) = if apply && !same_root {
        db.rewrite_asset_roots(&old_root, &normalized)?
    } else {
        db.inspect_asset_roots(&old_root)?
    };
    if apply && !same_root {
        // DB 已提交在先；配置写失败时用户对同一目标重试即自愈
        // （路径已重写 → affected=0 → 只补配置落盘）。
        let snapshot = {
            let mut settings = state.settings.lock().expect("settings mutex poisoned");
            if let Some(lib) = settings.libraries.iter_mut().find(|l| l.id == library_id) {
                lib.photo_root = normalized.clone();
            }
            settings.clone()
        };
        SettingsManager::save(&snapshot, &state.config_dir).map_err(|e| e.to_string())?;
        crate::index::kick(std::path::PathBuf::from(&db_dir), &state.supervisor);
    }
    Ok(LibraryRelocateDto {
        affected,
        unaffected,
        root_exists,
    })
}

/// 重定位库照片存储目录（library_relocate）：DB 批量重写 + settings 落盘
/// + 事件 → 后台线程执行（铁律：批量 UPDATE 不上主线程）。
#[tauri::command]
pub async fn library_relocate(
    app: AppHandle,
    state: State<'_, SharedState>,
    library_id: String,
    new_photo_root: String,
    apply: bool,
) -> Result<LibraryRelocateDto, String> {
    let shared = state.inner().clone();
    let dto = run_blocking(shared, move |state| {
        fetch_library_relocate(&state, &library_id, &new_photo_root, apply)
    })
    .await?;
    if apply {
        // settings 已在后台落盘并更新内存快照；事件通知前端刷新
        let snapshot = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .clone();
        let _ = app.emit("settings://changed", &snapshot);
    }
    Ok(dto)
}

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
    crate::settings::normalize_library_quality_tiers(&mut settings, &previous)?;
    // 建库路径统一规范化（2026-09-28 边界修复）：onboarding 提交的
    // photoRoot/dbDir 拒绝盘符相对路径（如 `I:xxx` 按进程 CWD 解析），
    // 绝对路径归一到 canonical/反斜杠形态后持久化并随 settings://changed
    // 回传前端；任一非法拒绝整次写入。
    crate::settings::normalize_library_paths(&mut settings)?;
    crate::settings::validate_library_storage_paths(&settings, &previous)?;
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
    crate::ai::selection::set_eyes_ear_thresholds(
        settings.ai.eyes_ear_closed,
        settings.ai.eyes_ear_maybe,
    );
    // 参数指纹比对：变更通道后台自动重建（无库/无变更为 no-op）
    let ai_snapshot = settings.ai.clone();
    *state.settings.lock().expect("settings mutex poisoned") = settings.clone();
    if let Some(library) = settings.active_library() {
        let library_id = library.id.clone();
        let db_dir = std::path::PathBuf::from(&library.db_dir);
        let shared = state.inner().clone();
        state
            .supervisor
            .spawn("index", "params-fingerprint-check".into(), move |_| {
                {
                    let current = shared.settings.lock().expect("settings mutex poisoned");
                    if current.active_library_id.as_deref() != Some(&library_id)
                        || current.ai != ai_snapshot
                    {
                        return; // 切库/再次修改后的迟到任务不可重建旧库。
                    }
                }
                super::indexing::check_params_and_rebuild(&shared, &db_dir, &ai_snapshot);
            });
    }
    app.emit("settings://changed", &settings)
        .map_err(|err| err.to_string())?;
    Ok(())
}
