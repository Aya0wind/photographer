//! 目录迁移（M3 数据层，spec §5.11：达芬奇式库管理）。
//!
//! - **dbDir 迁移** [`db_dir_migrate`]：整库目录自包含 → 两阶段：
//!   ①复制+逐文件校验（`new_dir/.db-migration.json` journal，中断可恢复，
//!   已校验文件跳过不重拷）②全部就绪后 `PRAGMA integrity_check` 复核 →
//!   旧目录写 `.migrated-bak` 标记 → 原子改 settings 注册表指向。任一文件
//!   复制/校验失败 → 回滚删除 new_dir（不留脏），settings 不动，旧库原样。
//! - **photoRoot 切换** [`photo_root_switch`]：mode=switch 仅改配置（物理
//!   文件零变化；旧照片 origin 转 external 只读语义）；mode=migrate 复用
//!   jobs/job_files journal 的批量移动（rename 优先，跨卷 copy+xxh 校验+
//!   删源）+ 资产路径逐文件更新，中断后重调同参恢复。
//!
//! 两种迁移进行中均拒绝新导入（[`crate::ipc`] 的 migrations 守卫）。
//! 命令为快验证 + supervisor 后台执行，进度经 migration* 事件回报。

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use xxhash_rust::xxh64::Xxh64;

use crate::db::Db;
use crate::events::{AppEvent, EventBus, Throttle};
use crate::ipc::{AppState, SharedState};
use crate::settings::{Library, Settings, SettingsManager};

/// 事件 kind 值：dbDir 迁移。
pub const KIND_DB_DIR: &str = "dbDir";
/// 事件 kind 值：photoRoot 切换/迁移。
pub const KIND_PHOTO_ROOT: &str = "photoRoot";
/// 旧 dbDir 完成迁移后的标记文件（内容 = 指向新目录的 JSON）。
pub const MARKER_FILE: &str = ".migrated-bak";
/// dbDir 迁移 journal 文件（落在新目录，随复制进度更新）。
const DB_JOURNAL_FILE: &str = ".db-migration.json";
/// journal 落盘周期（文件数）。
const JOURNAL_FLUSH_EVERY: usize = 32;
/// 进度事件节流。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// photoRoot 迁移任务在 jobs 表的 kind。
const PHOTO_ROOT_JOB_KIND: &str = "photo-root-migrate";
/// 流式复制块大小。
const COPY_CHUNK: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// 共用工具
// ---------------------------------------------------------------------------

/// 流式复制 + 校验：写块级 xxh64 哈希（写侧），写满后重读目标再哈希比对
/// （抓住截断/坏块）；大小不等直接失败。返回字节数。
fn copy_verified(src: &Path, dst: &Path) -> Result<u64, String> {
    let mut reader =
        fs::File::open(src).map_err(|e| format!("打开源文件失败 {}: {e}", src.display()))?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目录失败 {}: {e}", dst.display()))?;
    }
    let mut writer =
        fs::File::create(dst).map_err(|e| format!("创建目标失败 {}: {e}", dst.display()))?;
    let mut hasher = Xxh64::new(0);
    let mut buf = vec![0u8; COPY_CHUNK];
    let mut total = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("读取源失败 {}: {e}", src.display()))?;
        if n == 0 {
            break;
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| format!("写入目标失败 {}: {e}", dst.display()))?;
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    writer
        .flush()
        .map_err(|e| format!("刷新目标失败 {}: {e}", dst.display()))?;
    drop(writer);

    let expect = hasher.digest();
    let mut check =
        fs::File::open(dst).map_err(|e| format!("复读目标失败 {}: {e}", dst.display()))?;
    let mut rehash = Xxh64::new(0);
    let mut got = 0u64;
    loop {
        let n = check
            .read(&mut buf)
            .map_err(|e| format!("校验读取失败 {}: {e}", dst.display()))?;
        if n == 0 {
            break;
        }
        rehash.update(&buf[..n]);
        got += n as u64;
    }
    if got != total || rehash.digest() != expect {
        return Err(format!(
            "逐文件校验失败（大小 {got}/{total} 或 xxh 不符）: {}",
            dst.display()
        ));
    }
    Ok(total)
}

/// 单文件 move 语义（导入引擎同款）：同卷 rename；跨卷 copy+校验+删源。
/// dst 已存在视为冲突失败（不覆盖既有文件）。
fn move_file(src: &Path, dst: &Path) -> Result<(), String> {
    if dst.exists() {
        return Err(format!("目标已存在，拒绝覆盖: {}", dst.display()));
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
    }
    if fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    copy_verified(src, dst)?;
    fs::remove_file(src).map_err(|e| format!("移动删源失败 {}: {e}", src.display()))
}

/// 规范化用于同目录/嵌套判定：canonicalize + 剥 `\?\` verbatim 前缀
/// （canonical 旧库 vs 未创建的新目录形态不一致会让 starts_with 失明）；
/// 路径不存在时规范化其最近存在的祖先再拼回尾部组件（映射盘符场景下
/// 新目录常未创建，Y: 会被解析成 UNC，两侧同形态才可比）。
fn normalized(path: &Path) -> PathBuf {
    let strip = |p: &Path| PathBuf::from(crate::devices::folder::strip_verbatim(p));
    if let Ok(canon) = fs::canonicalize(path) {
        return strip(&canon);
    }
    let mut tail = Vec::new();
    let mut cur = path.to_path_buf();
    while let Some(name) = cur.file_name() {
        tail.push(name.to_os_string());
        match cur.parent() {
            Some(parent) => cur = parent.to_path_buf(),
            None => break,
        }
        if let Ok(canon) = fs::canonicalize(&cur) {
            let mut out = strip(&canon);
            for part in tail.iter().rev() {
                out.push(part);
            }
            return out;
        }
    }
    strip(path)
}

/// a 与 b 相同或互为祖先。
fn same_or_nested(a: &Path, b: &Path) -> bool {
    let a = normalized(a);
    let b = normalized(b);
    a == b || a.starts_with(&b) || b.starts_with(&a)
}

/// LIKE 前缀模式 + 转义（路径可含 %/_）。
fn like_prefix(prefix: &str) -> String {
    let mut out = String::with_capacity(prefix.len() + 2);
    for ch in prefix.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('%');
    out
}

/// 路径统一以分隔符结尾（前缀匹配避免 `Y:\照片2` 误中 `Y:\照片`）。
fn with_trailing_sep(root: &str) -> String {
    let trimmed = root.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        // 盘根（如 "X:\"）trim 后为空：保留原样
        root.to_string()
    } else {
        format!("{trimmed}\\")
    }
}

/// 迁移守卫：登记库 id（该库拒绝新导入/新迁移）。登记在命令同步段，
/// 除名在后台任务收尾（任务体所有退出路径都会经过收尾；任务 panic 属
/// 已上报的灾难路径，守卫留存至进程重启）。
fn guard_acquire(state: &AppState, library_id: &str) {
    state
        .migrations
        .lock()
        .expect("migrations mutex poisoned")
        .insert(library_id.to_string());
}

fn guard_release(state: &AppState, library_id: &str) {
    state
        .migrations
        .lock()
        .expect("migrations mutex poisoned")
        .remove(library_id);
}

/// 库 id 定位（无则 Err）。
fn find_library(state: &AppState, library_id: &str) -> Result<Library, String> {
    state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .libraries
        .iter()
        .find(|lib| lib.id == library_id)
        .cloned()
        .ok_or_else(|| format!("库不存在: {library_id}"))
}

/// settings 变更原子落盘（内存快照 + settings.json）。
fn save_settings(state: &AppState, mutate: impl FnOnce(&mut Settings)) -> Result<(), String> {
    let mut settings = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .clone();
    mutate(&mut settings);
    SettingsManager::save(&settings, &state.config_dir)
        .map_err(|e| format!("保存设置失败: {e}"))?;
    *state.settings.lock().expect("settings mutex poisoned") = settings;
    Ok(())
}

// ---------------------------------------------------------------------------
// dbDir 迁移
// ---------------------------------------------------------------------------

/// journal（落 new_dir/.db-migration.json）：已校验文件的 (rel, size, xxh)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DbMigrationJournal {
    schema: u32,
    old_dir: String,
    verified: Vec<VerifiedFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifiedFile {
    /// 相对 old_dir 的路径（/ 形态）。
    rel: String,
    size: u64,
    xxh: u64,
}

impl DbMigrationJournal {
    fn load(new_dir: &Path) -> Option<Self> {
        let raw = fs::read_to_string(new_dir.join(DB_JOURNAL_FILE)).ok()?;
        serde_json::from_str(&raw).ok()
    }

    /// 原子落盘（tmp + rename）。
    fn save(&self, new_dir: &Path) -> Result<(), String> {
        let path = new_dir.join(DB_JOURNAL_FILE);
        let tmp = new_dir.join(format!("{DB_JOURNAL_FILE}.tmp"));
        let json = serde_json::to_string(self).map_err(|e| e.to_string())?;
        fs::write(&tmp, json).map_err(|e| format!("写 journal 失败: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("落 journal 失败: {e}"))
    }
}

/// `.migrated-bak` 标记内容。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MigratedMarker {
    new_dir: String,
    finished_at: String,
}

/// dbDir 迁移入口：同步验证 → 后台执行（进度/结果经 migration* 事件）。
/// 重复调用同参 = 断点恢复（journal 已校验文件跳过）。
pub fn db_dir_migrate(state: &SharedState, library_id: &str, new_dir: &str) -> Result<(), String> {
    let library = find_library(state, library_id)?;
    let old_dir = PathBuf::from(&library.db_dir);
    let new_dir = PathBuf::from(new_dir);
    if !old_dir.is_dir() {
        return Err(format!("库目录不存在: {}", old_dir.display()));
    }
    if same_or_nested(&old_dir, &new_dir) {
        return Err("新目录与旧目录相同或互相嵌套，拒绝迁移".into());
    }
    // 新目录必须为空/不存在；唯一例外：存在匹配的 journal（断点恢复）
    let resumable =
        DbMigrationJournal::load(&new_dir).is_some_and(|j| Path::new(&j.old_dir) == old_dir);
    let new_empty = !new_dir.exists()
        || (fs::read_dir(&new_dir)
            .map(|mut d| d.next().is_none())
            .unwrap_or(false));
    if !new_empty && !resumable {
        return Err(format!(
            "新目录非空且无可恢复的迁移进度: {}",
            new_dir.display()
        ));
    }
    guard_acquire(state, library_id);

    let bus = state.bus.clone();
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    let state_clone = std::sync::Arc::clone(state);
    let library_id = library_id.to_string();
    supervisor.spawn("migrate", format!("db-dir-{library_id}"), move |_| {
        let outcome = run_db_dir_migration(&state_clone, &library_id, &old_dir, &new_dir, &bus);
        guard_release(&state_clone, &library_id);
        match outcome {
            Ok(()) => bus.publish(AppEvent::MigrationFinished {
                ok: true,
                failed: 0,
            }),
            Err(err) => {
                eprintln!("dbDir 迁移失败: {err}");
                bus.publish(AppEvent::MigrationFinished {
                    ok: false,
                    failed: 1,
                });
            }
        }
    });
    Ok(())
}

/// dbDir 迁移主体（后台线程）：复制校验（journal 断点）→ 完整性复核 →
/// 标记 + 改注册表。文件级失败 → 回滚删 new_dir 不留脏。
fn run_db_dir_migration(
    state: &SharedState,
    library_id: &str,
    old_dir: &Path,
    new_dir: &Path,
    bus: &EventBus,
) -> Result<(), String> {
    fs::create_dir_all(new_dir).map_err(|e| format!("创建新库目录失败: {e}"))?;

    // 活跃 SQLite 连接的 WAL 归并进主库文件（迁移窗口内拒绝导入，无并发写）
    checkpoint_wal(old_dir);

    // 待复制清单（walkdir 快照）
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    for entry in walkdir::WalkDir::new(old_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
    {
        let rel = entry
            .path()
            .strip_prefix(old_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .map_err(|e| e.to_string())?;
        files.push((rel, entry.path().to_path_buf()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut journal = DbMigrationJournal::load(new_dir)
        .filter(|j| Path::new(&j.old_dir) == old_dir)
        .unwrap_or(DbMigrationJournal {
            schema: 1,
            old_dir: old_dir.to_string_lossy().into_owned(),
            verified: Vec::new(),
        });
    let done: HashSet<String> = journal.verified.iter().map(|v| v.rel.clone()).collect();

    let total_bytes: u64 = files
        .iter()
        .map(|(_, p)| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
        .sum();
    bus.publish(AppEvent::MigrationStarted {
        kind: KIND_DB_DIR.into(),
        total_bytes,
    });

    let failure = (|| -> Result<(), String> {
        let mut done_bytes = journal.verified.iter().map(|v| v.size).sum::<u64>();
        let mut throttle = Throttle::new(PROGRESS_INTERVAL);
        let mut since_flush = 0usize;
        for (rel, src) in &files {
            let dst = new_dir.join(rel.replace('/', "\\"));
            let meta = match fs::metadata(src) {
                Ok(m) => m,
                Err(e) => return Err(format!("读取源信息失败 {rel}: {e}")),
            };
            // 断点恢复：已校验且目标在位（大小一致）→ 跳过
            if done.contains(rel) {
                if dst.exists() && fs::metadata(&dst).is_ok_and(|m| m.len() == meta.len()) {
                    continue;
                }
                // journal 说完成但目标缺失/不符 → 重做（从 verified 除名）
                journal.verified.retain(|v| v.rel != *rel);
            }
            let copied = copy_verified(src, &dst)?;
            if copied != meta.len() {
                return Err(format!("复制长度不符 {rel}: {copied}/{}", meta.len()));
            }
            journal.verified.push(VerifiedFile {
                rel: rel.clone(),
                size: copied,
                xxh: 0,
            });
            done_bytes += copied;
            since_flush += 1;
            if since_flush >= JOURNAL_FLUSH_EVERY {
                journal.save(new_dir)?;
                since_flush = 0;
            }
            if throttle.should_fire() {
                bus.publish(AppEvent::MigrationProgress {
                    done_bytes,
                    current: rel.clone(),
                });
            }
        }
        journal.save(new_dir)?;
        Ok(())
    })();

    if let Err(err) = failure {
        // 不留脏：回滚删除整个 new_dir（含 journal），settings/旧库原样
        let _ = fs::remove_dir_all(new_dir);
        return Err(err);
    }

    // 库文件完整性复核（无 library.db 的半成品目录跳过）
    if new_dir.join("library.db").is_file() {
        let db = Db::open(&new_dir.join("library.db")).map_err(|e| format!("打开新库失败: {e}"))?;
        let check: String =
            db.0.query_row("PRAGMA integrity_check", [], |r| r.get(0))
                .map_err(|e| format!("完整性检查失败: {e}"))?;
        if check != "ok" {
            let _ = fs::remove_dir_all(new_dir);
            return Err(format!("新库完整性检查未通过: {check}"));
        }
    }

    // 阶段②：旧目录标记 + 原子改注册表（标记先写：崩溃时注册表未动可重试）
    let marker = MigratedMarker {
        new_dir: new_dir.to_string_lossy().into_owned(),
        finished_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    };
    fs::write(
        old_dir.join(MARKER_FILE),
        serde_json::to_string_pretty(&marker).unwrap_or_default(),
    )
    .map_err(|e| format!("写迁移标记失败: {e}"))?;
    save_settings(state, |s| {
        if let Some(lib) = s.libraries.iter_mut().find(|l| l.id == library_id) {
            lib.db_dir = new_dir.to_string_lossy().into_owned();
        }
    })?;
    Ok(())
}

/// WAL 归并（TRUNCATE）：把 -wal 内容并回主库文件，文件级复制即自洽。
fn checkpoint_wal(db_dir: &Path) {
    let db_path = db_dir.join("library.db");
    if !db_path.is_file() {
        return;
    }
    let Ok(db) = Db::open(&db_path) else {
        return;
    };
    // wal_checkpoint(TRUNCATE) 返回 (busy, log, checkpointed)；失败静默
    //（复制后 integrity_check 兜底）。
    let _ = db.0.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, i64>(2)?,
        ))
    });
}

// ---------------------------------------------------------------------------
// photoRoot 切换 / 迁移
// ---------------------------------------------------------------------------

/// photoRoot 迁移任务的 plan_json 载荷（jobs 表）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PhotoRootPlan {
    library_id: String,
    old_root: String,
    new_root: String,
}

/// photoRoot 切换入口。mode：`switch` 仅改配置（旧照片转 external 只读
/// 语义，零物理变化）；`migrate` 批量移动 + 资产路径更新（journal 断点
/// 恢复，重调同参续跑）。
pub fn photo_root_switch(
    state: &SharedState,
    library_id: &str,
    new_root: &str,
    mode: &str,
) -> Result<(), String> {
    let library = find_library(state, library_id)?;
    let old_root = library.photo_root.clone();
    let new_root_path = PathBuf::from(new_root);
    match mode {
        "switch" => run_photo_root_switch(state, library_id, &old_root, new_root),
        "migrate" => {
            let old_root_path = PathBuf::from(&old_root);
            if same_or_nested(&old_root_path, &new_root_path) {
                return Err("新照片根与旧照片根相同或互相嵌套，拒绝迁移".into());
            }
            let db = crate::ipc::open_library_db(Path::new(&library.db_dir))?;
            // 断点恢复：存在未完成的同库迁移任务且目标一致 → 续跑
            if let Some((job_id, plan)) = unfinished_photo_root_job(&db, library_id)? {
                if plan.new_root != new_root {
                    return Err(format!(
                        "已有未完成的照片迁移任务（目标 {}），请先完成或续跑同目标",
                        plan.new_root
                    ));
                }
                guard_acquire(state, library_id);
                let bus = state.bus.clone();
                let supervisor = std::sync::Arc::clone(&state.supervisor);
                let state_clone = std::sync::Arc::clone(state);
                let library_id = library_id.to_string();
                supervisor.spawn(
                    "migrate",
                    format!("photo-root-resume-{library_id}"),
                    move |_| {
                        resume_photo_root_job(&state_clone, &library_id, job_id, &bus);
                        guard_release(&state_clone, &library_id);
                    },
                );
                return Ok(());
            }
            start_photo_root_migration(state, library_id, &old_root, new_root, &db)
        }
        _ => Err(format!("未知模式 {mode}（可选 switch|migrate）")),
    }
}

/// switch 模式：改配置 + 旧照片 origin→external（原子落盘，零 IO 移动）。
fn run_photo_root_switch(
    state: &SharedState,
    library_id: &str,
    old_root: &str,
    new_root: &str,
) -> Result<(), String> {
    let library = find_library(state, library_id)?;
    let db = crate::ipc::open_library_db(Path::new(&library.db_dir))?;
    let pattern = like_prefix(&with_trailing_sep(old_root));
    let flipped =
        db.0.execute(
            "UPDATE assets SET origin = 'external' WHERE origin = 'imported' \
             AND path LIKE ?1 ESCAPE '\\'",
            [&pattern],
        )
        .map_err(|e| e.to_string())? as u64;
    save_settings(state, |s| {
        if let Some(lib) = s.libraries.iter_mut().find(|l| l.id == library_id) {
            lib.photo_root = new_root.to_string();
        }
    })?;
    state.bus.publish(AppEvent::MigrationStarted {
        kind: KIND_PHOTO_ROOT.into(),
        total_bytes: 0,
    });
    state.bus.publish(AppEvent::MigrationFinished {
        ok: true,
        failed: 0,
    });
    let _ = db.append_log(
        "info",
        None,
        &format!(
            "photoRoot 仅切换：{old_root} → {new_root}（{flipped} 个旧照片资产转为 external）"
        ),
    );
    Ok(())
}

/// 找库的最新未完成 photoRoot 迁移任务（status running/paused/failed）。
fn unfinished_photo_root_job(
    db: &Db,
    library_id: &str,
) -> Result<Option<(i64, PhotoRootPlan)>, String> {
    let device_id = photo_root_device_id(library_id);
    let mut stmt =
        db.0.prepare(
            "SELECT id, plan_json FROM jobs WHERE kind = ?1 AND device_id = ?2 \
             AND status IN ('running', 'paused', 'failed') ORDER BY id DESC LIMIT 1",
        )
        .map_err(|e| e.to_string())?;
    let mut rows = stmt
        .query(rusqlite::params![PHOTO_ROOT_JOB_KIND, device_id])
        .map_err(|e| e.to_string())?;
    match rows.next().map_err(|e| e.to_string())? {
        Some(row) => {
            let id: i64 = row.get(0).map_err(|e| e.to_string())?;
            let plan_json: String = row.get(1).map_err(|e| e.to_string())?;
            let plan = serde_json::from_str(&plan_json).map_err(|e| format!("计划损坏: {e}"))?;
            Ok(Some((id, plan)))
        }
        None => Ok(None),
    }
}

fn photo_root_device_id(library_id: &str) -> String {
    format!("library:{library_id}")
}

/// 新建 photoRoot 迁移任务：资产清单（旧根下非 external）→ jobs/job_files
/// journal → 后台执行。
fn start_photo_root_migration(
    state: &SharedState,
    library_id: &str,
    old_root: &str,
    new_root: &str,
    db: &Db,
) -> Result<(), String> {
    let prefix = with_trailing_sep(old_root);
    let pattern = like_prefix(&prefix);
    let mut stmt =
        db.0.prepare(
            "SELECT path, size FROM assets WHERE origin != 'external' AND path LIKE ?1 \
             ESCAPE '\\' ORDER BY path",
        )
        .map_err(|e| e.to_string())?;
    let rows: Vec<(String, u64)> = stmt
        .query_map([&pattern], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    if rows.is_empty() {
        return Err("旧照片根下没有可迁移的库内资产".into());
    }

    let total_bytes: u64 = rows.iter().map(|(_, s)| s).sum();
    let plan = PhotoRootPlan {
        library_id: library_id.to_string(),
        old_root: old_root.to_string(),
        new_root: new_root.to_string(),
    };
    let job_id = db
        .create_job_with_plan(
            PHOTO_ROOT_JOB_KIND,
            &photo_root_device_id(library_id),
            "photoRoot 迁移",
            rows.len() as u64,
            total_bytes,
            &serde_json::to_string(&plan).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    for (path, size) in &rows {
        db.upsert_job_file(&crate::db::JobFileRow {
            job_id,
            src: path.clone(),
            dst: remap_path(new_root, &prefix, path),
            size: *size,
            state: crate::events::FileState::Pending,
            error: None,
            xxhash: None,
            dst2: String::new(),
        })
        .map_err(|e| e.to_string())?;
    }
    let _ = db.append_log("info", Some(job_id), "photoRoot 迁移任务创建");

    guard_acquire(state, library_id);
    let bus = state.bus.clone();
    let supervisor = std::sync::Arc::clone(&state.supervisor);
    let state_clone = std::sync::Arc::clone(state);
    let library_id = library_id.to_string();
    supervisor.spawn("migrate", format!("photo-root-{library_id}"), move |_| {
        resume_photo_root_job(&state_clone, &library_id, job_id, &bus);
        guard_release(&state_clone, &library_id);
    });
    Ok(())
}

/// 旧路径 → 新路径（new_root + 去掉旧根前缀后的余段）。
fn remap_path(new_root: &str, old_prefix: &str, path: &str) -> String {
    let rel = path
        .strip_prefix(old_prefix)
        .unwrap_or(path.trim_start_matches('\\'));
    let rel = rel.replace('/', "\\");
    let root = new_root.trim_end_matches('\\');
    format!("{root}\\{rel}")
}

/// photoRoot 迁移执行体（新任务/断点恢复共用）：逐行
/// pending/failed 重做——移动文件（rename/跨卷校验复制）→ 资产路径更新 →
/// journal verified；dst 已在位且源不在 → 对账补账（崩溃窗口）。
fn resume_photo_root_job(state: &SharedState, library_id: &str, job_id: i64, bus: &EventBus) {
    let library = match find_library(state, library_id) {
        Ok(l) => l,
        Err(err) => {
            eprintln!("photoRoot 迁移中断：{err}");
            bus.publish(AppEvent::MigrationFinished {
                ok: false,
                failed: 1,
            });
            return;
        }
    };
    let db = match crate::ipc::open_library_db(Path::new(&library.db_dir)) {
        Ok(d) => d,
        Err(err) => {
            eprintln!("photoRoot 迁移中断：{err}");
            bus.publish(AppEvent::MigrationFinished {
                ok: false,
                failed: 1,
            });
            return;
        }
    };
    let rows = match db.all_job_files(job_id) {
        Ok(r) => r,
        Err(err) => {
            eprintln!("photoRoot 迁移中断：{err}");
            bus.publish(AppEvent::MigrationFinished {
                ok: false,
                failed: 1,
            });
            return;
        }
    };
    let mut done_bytes = 0u64;
    let mut failed = 0u64;
    let mut total_bytes = 0u64;
    let mut todo: Vec<crate::db::JobFileRow> = Vec::new();
    for row in &rows {
        total_bytes += row.size;
        match row.state {
            crate::events::FileState::Verified => done_bytes += row.size,
            _ => todo.push(row.clone()),
        }
    }
    bus.publish(AppEvent::MigrationStarted {
        kind: KIND_PHOTO_ROOT.into(),
        total_bytes,
    });
    let mut throttle = Throttle::new(PROGRESS_INTERVAL);
    let mut settled = done_bytes;

    for row in todo {
        let src = PathBuf::from(&row.src);
        let dst = PathBuf::from(&row.dst);
        let outcome = if !src.exists() && dst.exists() {
            // 崩溃窗口对账：文件已移动、账未记完 → 补资产路径 + verified
            Ok(())
        } else if src.exists() && dst.exists() {
            Err(format!(
                "目标已存在且源未删（需人工裁决）: {}",
                dst.display()
            ))
        } else if !src.exists() {
            Err(format!("源文件缺失: {}", src.display()))
        } else {
            move_file(&src, &dst)
        };
        match outcome {
            Ok(()) => {
                let _ = db.0.execute(
                    "UPDATE assets SET path = ?2 WHERE path = ?1",
                    rusqlite::params![row.src, row.dst],
                );
                let _ = db.upsert_job_file(&crate::db::JobFileRow {
                    job_id,
                    src: row.src.clone(),
                    dst: row.dst.clone(),
                    size: row.size,
                    state: crate::events::FileState::Verified,
                    error: None,
                    xxhash: None,
                    dst2: String::new(),
                });
                settled += row.size;
                if throttle.should_fire() {
                    bus.publish(AppEvent::MigrationProgress {
                        done_bytes: settled,
                        current: row.src.clone(),
                    });
                }
            }
            Err(err) => {
                failed += 1;
                let _ = db.upsert_job_file(&crate::db::JobFileRow {
                    job_id,
                    src: row.src.clone(),
                    dst: row.dst.clone(),
                    size: row.size,
                    state: crate::events::FileState::Failed,
                    error: Some(err.clone()),
                    xxhash: None,
                    dst2: String::new(),
                });
                let _ = db.append_log("error", Some(job_id), &err);
            }
        }
    }

    let ok = failed == 0;
    if ok {
        // 全部落位：收尾任务 + 切换注册表指向新根（plan 丢失视为收尾失败，
        // 文件已安全移动，重试可对账补账）
        let _ = db.finish_job(job_id, "done", "{}");
        let new_root = db
            .job_plan_json(job_id)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<PhotoRootPlan>(&json).ok())
            .map(|plan| plan.new_root);
        let Some(new_root) = new_root else {
            eprintln!("photoRoot 迁移收尾失败：任务计划损坏，无法切换注册表");
            bus.publish(AppEvent::MigrationFinished {
                ok: false,
                failed: 1,
            });
            return;
        };
        if let Err(err) = save_settings(state, |s| {
            if let Some(lib) = s.libraries.iter_mut().find(|l| l.id == library_id) {
                lib.photo_root = new_root;
            }
        }) {
            eprintln!("photoRoot 迁移收尾失败（文件已移动）: {err}");
            bus.publish(AppEvent::MigrationFinished {
                ok: false,
                failed: 1,
            });
            return;
        }
        let _ = db.append_log("info", Some(job_id), "photoRoot 迁移完成");
    } else {
        let _ = db.finish_job(job_id, "failed", "{}");
        let _ = db.append_log(
            "warn",
            Some(job_id),
            &format!("photoRoot 迁移存在 {failed} 个失败文件（可重试恢复）"),
        );
    }
    bus.publish(AppEvent::MigrationFinished { ok, failed });
}
