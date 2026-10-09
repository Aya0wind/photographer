//! 相册/子组「导出为文件夹」命令（M6，Photo Hub → LR 互操作；§六定案）。
//!
//! 同卷硬链接（秒级零拷贝）/ 跨卷拷贝 + **全量 XMP 边车**（星级/颜色/
//! 关键字，库内 DB 真值投影）→ LR 引用导入。导出物为快照，双向修改互不
//! 影响：本体硬链接 inode 语义天然保证；**边车永远独立落盘（绝不硬链接）**
//! ——LR 侧写回不波及库内。
//!
//! 任务模型（对齐单文件导出 export_job 的「重启后可查终态」）：
//! - 启动即建 `album_export_job` queued 行（total 预填成员数快照），IPC 层
//!   经 TaskSupervisor 后台线程执行 [`run_album_export_job`]——命令立刻
//!   返回任务 DTO，进度/收尾走 EventBus（`app://event` 既有任务事件通道：
//!   AlbumExportProgress / AlbumExportFinished，进度发布节流 ≥100ms）。
//! - 单活跃任务：已有一单在跑时拒绝新任务（对话框单例 + status 单数契约）。
//! - 取消 = 软信号（当前文件写完即停），终态 cancelled，已完成计数保留。
//! - 进程重启后遗留的 queued/running 行在下一次 run/status 对照内存活跃
//!   集合收尸为 error（无续传语义）。
//!
//! 容错（计划 §五）：源 missing 的成员（DB 缺失标记或文件不在盘）跳过
//! 不计入 done——前端以 total-done 差值出「跳过」总结；单成员拷贝/边车
//! 失败不中断任务，收尾 error 汇总失败数与样本。
//!
//! 目标校验：绝对路径 + 目标存在时必须是文件夹；目标落在任一照片库 root
//! 内**不禁止**，DTO 携带提示文案（「该文件夹会被扫描忽略，建议选择照片
//! 库外」——库内目标必然同卷硬链接，行为已被 §三 两级去重闭合）。

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::State;

use crate::db::{AlbumExportJobRow, AlbumExportMember, Db};
use crate::events::{AppEvent, EventBus};
use crate::import::templates::unique_path;
use crate::ipc::{run_blocking, AppState, SharedState};

/// 进度事件与进度落库的共用节流窗（与 libraryScanProgress 同节奏）。
const PROGRESS_THROTTLE: Duration = Duration::from_millis(100);

/// 相册/子组导出为文件夹的后台任务 DTO（camelCase，与前端 `AlbumExportTask`
/// 契约对齐；album_export_run 返回 / album_export_status 查询）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumExportTaskDto {
    pub id: i64,
    pub album_id: i64,
    /// 子分组名（None=整个相册）。
    pub subgroup: Option<String>,
    pub output_dir: String,
    /// queued | running | done | cancelled | error。
    pub status: String,
    /// 相册内待导出资产数。
    pub total: u64,
    /// 已导出数（源 missing 跳过不计入）。
    pub done: u64,
    /// 其中硬链接落盘数（其余为拷贝）。
    pub linked: u64,
    pub error: Option<String>,
    /// 目标校验提示（目标落在任一照片库内时不禁止、携带提示文案；前端
    /// 对话框展示。可选字段，旧契约消费方缺省兼容）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

impl AlbumExportTaskDto {
    /// 行投影 + 提示文案组装（warning 由调用方按库清单现算——不入库，
    /// 库登记变动后旧任务提示自动跟随新真值）。
    fn from_row(row: &AlbumExportJobRow, warning: Option<String>) -> Self {
        Self {
            id: row.id,
            album_id: row.album_id,
            subgroup: row.subgroup.clone(),
            output_dir: row.output_dir.clone(),
            status: row.status.clone(),
            total: row.total,
            done: row.done,
            linked: row.linked,
            error: row.error.clone(),
            warning,
        }
    }
}

// ---------------------------------------------------------------------------
// 单活跃任务运行时（取消信号 + 孤儿收尸对照）
// ---------------------------------------------------------------------------

/// 进程级活跃任务槽：单活跃模型（DTO/对话框均单数契约）。
struct ActiveJob {
    job_id: i64,
    cancel: Arc<AtomicBool>,
}

fn active_job() -> &'static Mutex<Option<ActiveJob>> {
    static ACTIVE: OnceLock<Mutex<Option<ActiveJob>>> = OnceLock::new();
    ACTIVE.get_or_init(|| Mutex::new(None))
}

/// 活跃槽守卫（Drop 摘除——worker 正常/出错/panic 三态都收回；同
/// edit::export::LiveGuard 手法，防 panic 泄漏槽位后永久拒单）。
struct SlotGuard(i64);

impl SlotGuard {
    fn register(job_id: i64, cancel: Arc<AtomicBool>) -> Self {
        let mut slot = active_job()
            .lock()
            .expect("album export active job mutex poisoned");
        *slot = Some(ActiveJob { job_id, cancel });
        Self(job_id)
    }
}

impl Drop for SlotGuard {
    fn drop(&mut self) {
        clear_active_job(self.0);
    }
}

/// 摘除活跃槽（仅当仍是该任务；守卫 Drop 与 fetch 侧异常兜底共用）。
fn clear_active_job(job_id: i64) {
    let mut slot = active_job()
        .lock()
        .expect("album export active job mutex poisoned");
    if slot.as_ref().is_some_and(|job| job.job_id == job_id) {
        *slot = None;
    }
}

/// 进程重启孤儿收尸：queued/running 且不在活跃槽 → error（幂等）。
fn reap_orphan_jobs(db: &Db) {
    let live = active_job()
        .lock()
        .expect("album export active job mutex poisoned")
        .as_ref()
        .map(|job| vec![job.job_id])
        .unwrap_or_default();
    let Ok(stale) = db.album_export_job_stale_ids(&live) else {
        return;
    };
    for id in stale {
        let _ = db.album_export_job_finish(id, "error", 0, 0, Some("进程重启中断"));
    }
}

/// 启动前的活跃任务检查（单活跃闸）：返回在跑任务 id。
fn running_job_id() -> Option<i64> {
    active_job()
        .lock()
        .expect("album export active job mutex poisoned")
        .as_ref()
        .map(|job| job.job_id)
}

/// 当前活跃任务 id（None=空闲）。lib 内无消费点——集成测试等待槽位
/// 释放用（tests/album_export_test.rs），同 edit::export::wait_terminal
/// 的 `#[allow(dead_code)]` 先例。
#[allow(dead_code)]
pub fn active_job_id() -> Option<i64> {
    running_job_id()
}

/// 目标落在任一照片库 root 内（或等于 root）→ 提示文案（§六：不禁止）。
/// 判定与前端 exportTarget.isPathInside 同口径（前缀包含、大小写/分隔符
/// 不敏感），后端持有库登记真值。
fn library_containment_warning(db: &Db, output_dir: &str) -> Option<String> {
    let libraries = db.photos_library_list().ok()?;
    for library in libraries {
        if crate::db::strip_root_prefix(output_dir, &library.root_path).is_some() {
            return Some(format!(
                "目标文件夹位于照片库「{}」内：该文件夹会被扫描忽略（内容与库内重复），建议选择照片库外",
                library.name
            ));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 任务执行核
// ---------------------------------------------------------------------------

/// 单任务执行请求（IPC 层组装；worker 线程消费）。
pub struct AlbumExportJobRequest {
    pub job_id: i64,
    pub output_dir: String,
    /// 成员快照（启动时读取——total 与前端回显一致；任务中相册变动不波及）。
    pub members: Vec<AlbumExportMember>,
    /// 软取消信号（album_export_cancel 置位；文件边界响应）。
    pub cancel: Arc<AtomicBool>,
}

/// 执行一个导出任务至终态（running → done|cancelled|error + 事件）。
/// 阻塞；调用方决定线程（IPC 层 supervisor / 集成测试直调）。
pub fn run_album_export_job(db: Db, bus: &EventBus, request: AlbumExportJobRequest) {
    let job_id = request.job_id;
    let output_dir = Path::new(&request.output_dir).to_path_buf();
    let cancel = Arc::clone(&request.cancel);
    // 活跃槽守卫（fetch 已预登记同款旗标；这里以 worker 视角重设，保证
    // 槽内 job_id 与真跑任务一致）——Drop 摘除覆盖正常/出错/panic 三态。
    let _slot = SlotGuard::register(job_id, Arc::clone(&cancel));

    let mut done: u64 = 0;
    let mut linked: u64 = 0;
    let mut failures: Vec<String> = Vec::new();
    let mut last_progress = Instant::now();

    let _ = db.album_export_job_set_status(job_id, "running");
    // 目标目录就位（启动即建；失败 = 任务级 error——一个文件都导不出）。
    if let Err(error) = std::fs::create_dir_all(&output_dir) {
        let message = format!("创建输出目录失败 {}: {error}", output_dir.display());
        let _ = db.album_export_job_finish(job_id, "error", 0, 0, Some(&message));
        bus.publish(AppEvent::AlbumExportFinished {
            task_id: job_id,
            ok: false,
            exported: 0,
            linked: 0,
            error: Some(message),
        });
        return;
    }

    let total = request.members.len() as u64;
    for member in &request.members {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        match export_member(&db, member, &output_dir) {
            Ok(MemberOutcome::Linked) => {
                linked += 1;
                done += 1;
            }
            Ok(MemberOutcome::Copied) => {
                done += 1;
            }
            Ok(MemberOutcome::SkippedMissing) => {
                // §五：缺失成员跳过（DB 标记或文件不在盘），总结由
                // total-done 差值呈现——不失败、不计数。
            }
            Err(error) => failures.push(error),
        }
        // 进度节流（事件 + 落库同窗）：硬链接批次极快，逐成员发布会风暴。
        if last_progress.elapsed() >= PROGRESS_THROTTLE {
            last_progress = Instant::now();
            let _ = db.album_export_job_progress(job_id, done, linked);
            bus.publish(AppEvent::AlbumExportProgress {
                task_id: job_id,
                done,
                total,
            });
        }
    }

    let cancelled = cancel.load(Ordering::SeqCst);
    let (status, ok, error) = if cancelled {
        ("cancelled", false, None)
    } else if failures.is_empty() {
        ("done", true, None)
    } else {
        let sample: Vec<&str> = failures.iter().take(3).map(String::as_str).collect();
        (
            "done",
            false,
            Some(format!(
                "{} 个成员导出失败：{}",
                failures.len(),
                sample.join("；")
            )),
        )
    };
    let _ = db.album_export_job_finish(job_id, status, done, linked, error.as_deref());
    bus.publish(AppEvent::AlbumExportFinished {
        task_id: job_id,
        ok,
        exported: done,
        linked,
        error,
    });
}

/// 单成员导出结果。
enum MemberOutcome {
    Linked,
    Copied,
    SkippedMissing,
}

/// 单成员导出核：missing 跳过；同卷硬链接（失败回退拷贝）；边车独立落盘。
fn export_member(
    db: &Db,
    member: &AlbumExportMember,
    output_dir: &Path,
) -> Result<MemberOutcome, String> {
    let src = Path::new(&member.path);
    if member.missing || !src.is_file() {
        return Ok(MemberOutcome::SkippedMissing);
    }
    let dst = unique_path(output_dir, &member.filename);
    // ① 本体：同卷硬链接（秒级零拷贝）；卷识别不可用/硬链接失败回退拷贝
    //   （FAT/exFAT 无硬链接语义，拷贝是唯一正确行为）。
    let mut linked = false;
    if crate::platform::same_filesystem(src, &dst).unwrap_or(false)
        && std::fs::hard_link(src, &dst).is_ok()
    {
        linked = true;
    } else {
        std::fs::copy(src, &dst).map_err(|e| format!("拷贝 {} 失败: {e}", member.filename))?;
    }
    // ② 全量边车：基底 = 源旁边车（保留 LR 开发设置等未知字段），覆写
    //   星级/颜色/关键字为库内真值；无基底新建。独立文件（绝不硬链接）。
    let base = std::fs::read_to_string(crate::metadata::xmp::sidecar_path(src)).ok();
    let rating = crate::metadata::xmp::projected_rating(member.rating, member.rejected);
    let label = member
        .color_label
        .as_deref()
        .and_then(crate::metadata::xmp::label_to_xmp);
    let keywords = asset_keywords(db, member.asset_id);
    crate::metadata::xmp::write_export_sidecar(&dst, base.as_deref(), rating, label, &keywords)
        .map_err(|e| format!("边车写入 {} 失败: {e}", member.filename))?;
    Ok(if linked {
        MemberOutcome::Linked
    } else {
        MemberOutcome::Copied
    })
}

/// 成员关键字（asset_metadata.value JSON 的 keywords 数组；无记录/损坏
/// 返回空）。归一与编辑保存路径同规则：trim、去空、排序去重。
fn asset_keywords(db: &Db, asset_id: i64) -> Vec<String> {
    #[derive(Deserialize)]
    struct KeywordsOnly {
        #[serde(default)]
        keywords: Vec<String>,
    }
    let value: Option<String> = db
        .0
        .query_row(
            "SELECT value FROM asset_metadata WHERE asset_id = ?1",
            [asset_id],
            |row| row.get(0),
        )
        .ok();
    let mut keywords = value
        .and_then(|text| serde_json::from_str::<KeywordsOnly>(&text).ok())
        .map(|parsed| parsed.keywords)
        .unwrap_or_default()
        .into_iter()
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .collect::<Vec<_>>();
    keywords.sort();
    keywords.dedup();
    keywords
}

// ---------------------------------------------------------------------------
// IPC 命令核 + Tauri 壳
// ---------------------------------------------------------------------------

/// 启动核：收尸 → 校验 → 成员快照 → 建 queued 任务 → supervisor 后台执行。
pub fn fetch_album_export_run(
    state: &AppState,
    album_id: i64,
    subgroup: Option<&str>,
    output_dir: &str,
) -> Result<AlbumExportTaskDto, String> {
    let db = crate::ipc::app_database_db(state)?;
    reap_orphan_jobs(&db);
    if !db.album_exists(album_id).map_err(|e| e.to_string())? {
        return Err(format!("相册 {album_id} 不存在"));
    }
    if let Some(running) = running_job_id() {
        return Err(format!("已有相册导出任务（#{running}）在执行，请等待完成或先取消"));
    }
    let subgroup = subgroup
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    // 目标闸门：绝对路径；已存在的目标必须是文件夹（写入失败留给任务级
    // error 兜底）。不查库内（§六：库内目标不禁止，提示走 DTO.warning）。
    let output_dir = output_dir.trim();
    if output_dir.is_empty() {
        return Err("输出目录不能为空".into());
    }
    let output_path = Path::new(output_dir);
    if !output_path.is_absolute() {
        return Err(format!("输出目录必须是绝对路径：{output_dir}"));
    }
    if output_path.exists() && !output_path.is_dir() {
        return Err(format!("输出目标不是文件夹：{output_dir}"));
    }
    let warning = library_containment_warning(&db, output_dir);

    let members = db
        .album_export_members(album_id, subgroup.as_deref())
        .map_err(|e| e.to_string())?;
    let total = members.len() as u64;
    let job_id = db
        .album_export_job_create(album_id, subgroup.as_deref(), output_dir, total)
        .map_err(|e| e.to_string())?;

    // 预登记取消旗（worker 启动前 cancel 命令也不丢信号）。
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = active_job()
            .lock()
            .expect("album export active job mutex poisoned");
        *slot = Some(ActiveJob {
            job_id,
            cancel: Arc::clone(&cancel),
        });
    }
    // worker 库连接预开（打开失败 = 任务同步 error 收尾 + 槽回摘——不留
    // 「行 queued + 槽占用」的泄漏窗口）。
    let db_dir = crate::ipc::app_database_dir(state)?;
    let worker_db = match crate::ipc::open_library_db(&db_dir) {
        Ok(worker_db) => worker_db,
        Err(error) => {
            let message = format!("无法打开数据库: {error}");
            let _ = db.album_export_job_finish(job_id, "error", 0, 0, Some(&message));
            clear_active_job(job_id);
            return Err(message);
        }
    };
    let request = AlbumExportJobRequest {
        job_id,
        output_dir: output_dir.to_string(),
        members,
        cancel,
    };
    let bus = state.bus.clone();
    state
        .supervisor
        .spawn("album-export", format!("task-{job_id}"), move |_| {
            run_album_export_job(worker_db, &bus, request);
        });
    let row = db
        .album_export_job_latest()
        .map_err(|e| e.to_string())?
        .ok_or("导出任务建档后无法读回")?;
    Ok(AlbumExportTaskDto::from_row(&row, warning))
}

/// 状态查询核：最近一次任务行 + 提示重算（库登记变动后提示跟随新真值）。
pub fn fetch_album_export_status(state: &AppState) -> Result<Option<AlbumExportTaskDto>, String> {
    let db = crate::ipc::app_database_db(state)?;
    reap_orphan_jobs(&db);
    let row = db.album_export_job_latest().map_err(|e| e.to_string())?;
    Ok(row.map(|row| {
        let warning = library_containment_warning(&db, &row.output_dir);
        AlbumExportTaskDto::from_row(&row, warning)
    }))
}

/// 取消核：软信号（文件边界响应）；无在跑任务时空操作（幂等）。
pub fn fetch_album_export_cancel() {
    if let Some(job) = active_job()
        .lock()
        .expect("album export active job mutex poisoned")
        .as_ref()
    {
        job.cancel.store(true, Ordering::SeqCst);
    }
}

/// 启动相册/子组导出为文件夹（album_export_run）：建库内任务后台执行，
/// 立刻返回任务 DTO。
#[tauri::command]
pub async fn album_export_run(
    state: State<'_, SharedState>,
    album_id: i64,
    subgroup: Option<String>,
    output_dir: String,
) -> Result<AlbumExportTaskDto, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_album_export_run(state, album_id, subgroup.as_deref(), &output_dir)
    })
    .await
}

/// 当前/最近一次导出任务状态（album_export_status；无任务 null）。
#[tauri::command]
pub async fn album_export_status(
    state: State<'_, SharedState>,
) -> Result<Option<AlbumExportTaskDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, fetch_album_export_status).await
}

/// 取消在跑的导出任务（album_export_cancel；软信号，当前文件写完即停）。
#[tauri::command]
pub async fn album_export_cancel() -> Result<(), String> {
    fetch_album_export_cancel();
    Ok(())
}
