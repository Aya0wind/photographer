//! M1 T7 导入引擎（spec §5.2 / §7）：单文件单遍读取流水线。
//!
//! 打开源流 → 循环读 8MB chunk → xxh64+sha256 同时 update → 写 `.part`
//! （集中暂存目录）→ 首 1MB 截存 head（EXIF + 魔数识别）→ 渲染目标路径 →
//! 完成后长度校验 → 原子 rename → journal verified → AssetRepo 入库。
//!
//! 线程模型：Volume 源 `min(4, plan.streams)` 个工作线程抢文件队列，MTP 源
//! 强制 1；工作线程只做源读取/哈希/写盘（不碰 SQLite），收集线程（调用
//! `run` 的线程）串行做 journal/查重/rename/事件——SQLite 单线程访问，
//! rename 串行化避免并发 `unique_path` 竞态。
//!
//! 查重分层：① skip_imported 时宽松键（size+filename+mtime±2s）预判→跳过；
//! ② 全量 (size, xxhash) 精确复核→跳过；③ 目标路径冲突→Rename 策略生成
//! `_1`，Skip/Ask 策略→skipped。
//!
//! 可靠性：取消为软取消（当前文件完成后停）；`DeviceError::Disconnected`
//! → publish `DeviceUnavailable` + 自动暂停（job 置 paused）并返回部分
//! stats；每文件状态实时落 journal；`.part` 残留在 resume 时清除重做。

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use xxhash_rust::xxh64::Xxh64;

use crate::db::{AssetRow, Db, JobFileRow};
use crate::devices::{classify, DeviceError, DeviceSource, FileEntry, SourceKind};
use crate::events::{AppEvent, AssetKind, EventBus, FileState, JobStats, Throttle};
use crate::import::templates::{
    render_dir, render_name, resolve_captured, unique_path, RenderCtx, TemplateError,
};
use crate::metadata::exif_lite::{self, MetaLite};
use crate::settings::DuplicatePolicy;

/// 单遍读取的块大小（spec §5.2：8MB 缓冲）。
const CHUNK: usize = 8 * 1024 * 1024;
/// 头部截存上限：EXIF-lite + 魔数识别只需文件头。
const HEAD_MAX: usize = 1024 * 1024;
/// 进度事件最小间隔（spec：≥100ms）。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// 暂停时工作线程的轮询间隔。
const PAUSE_POLL: Duration = Duration::from_millis(10);
/// 宽松查重键的 mtime 容差。
const MTIME_TOLERANCE: chrono::Duration = chrono::Duration::seconds(2);
/// `.part` 集中暂存目录名（target_root 下，点前缀）。
const PART_DIR: &str = ".smartphoto-part";
/// {相机}/{镜头} 上下文缺失时的降级默认段。
const FALLBACK_CAMERA: &str = "未知相机";
const FALLBACK_LENS: &str = "未知镜头";

/// 导入计划（IPC 契约，camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub source_id: String,
    pub target_root: PathBuf,
    pub dir_template: String,
    pub name_template: String,
    pub duplicate: DuplicatePolicy,
    pub skip_imported: bool,
    pub streams: u32,
}

/// 引擎错误（begin 阶段：设备枚举或建任务失败）。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("设备错误: {0}")]
    Device(#[from] DeviceError),
}

/// 引擎控制柄：与 `run()` 并行持有（IPC 命令 / 测试用）。
/// pause=当前文件完成后停；cancel=软取消；done=run 收尾置位。
#[derive(Clone, Debug)]
pub struct EngineControls {
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
}

impl EngineControls {
    pub fn pause(&self) {
        self.paused.store(true, Ordering::SeqCst);
    }

    pub fn resume(&self) {
        self.paused.store(false, Ordering::SeqCst);
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }
}

/// 会话计数（JobStats 的累加器）。里程碑按“已结算”字节（done+skipped+failed）
/// 占比计算——跳过/失败的文件同样终结了处理。
#[derive(Debug, Default, Clone, Copy)]
struct Counters {
    total_files: u64,
    total_bytes: u64,
    done_files: u64,
    done_bytes: u64,
    skipped: u64,
    skipped_bytes: u64,
    failed: u64,
    failed_bytes: u64,
}

impl Counters {
    fn settled_bytes(self) -> u64 {
        self.done_bytes + self.skipped_bytes + self.failed_bytes
    }

    fn into_stats(self, elapsed: Duration) -> JobStats {
        let secs = elapsed.as_secs_f64();
        JobStats {
            total_files: self.total_files,
            done_files: self.done_files,
            skipped_duplicates: self.skipped,
            failed_files: self.failed,
            total_bytes: self.total_bytes,
            done_bytes: self.done_bytes,
            elapsed_ms: elapsed.as_millis() as u64,
            bytes_per_sec: if secs > 0.0 {
                self.done_bytes as f64 / secs
            } else {
                0.0
            },
        }
    }
}

/// begin/resume 产出的执行计划。
struct Prepared {
    job_id: i64,
    /// resume：journal 中待重做的 src（run 时对源 list 匹配）。
    resume_srcs: Option<Vec<String>>,
    /// 新任务：宽松预判后的待处理条目（resume 为空）。
    fresh: Vec<FileEntry>,
    /// 会话统计基线（新任务含宽松命中；resume 含既有 verified/skipped）。
    base: Counters,
}

/// 导入引擎。`new` + `begin` 建新任务；`resume` 从 journal 重建。
pub struct Engine {
    db: Db,
    bus: EventBus,
    source: Arc<dyn DeviceSource>,
    plan: ImportPlan,
    controls: EngineControls,
    prepared: Option<Prepared>,
}

impl Engine {
    pub fn new(db: Db, bus: EventBus, source: Box<dyn DeviceSource>, plan: ImportPlan) -> Self {
        Self::with_controls(db, bus, source, plan, None)
    }

    /// 从 journal 重建：verified/skipped 计入统计基线跳过，pending/failed 重做。
    pub fn resume(
        db: Db,
        bus: EventBus,
        source: Box<dyn DeviceSource>,
        plan: ImportPlan,
        job_id: i64,
    ) -> rusqlite::Result<Self> {
        let rows = db.all_job_files(job_id)?;
        let mut base = Counters::default();
        let mut resume_srcs = Vec::new();
        for row in &rows {
            base.total_files += 1;
            base.total_bytes += row.size;
            match row.state {
                FileState::Verified => {
                    base.done_files += 1;
                    base.done_bytes += row.size;
                }
                FileState::Skipped => {
                    base.skipped += 1;
                    base.skipped_bytes += row.size;
                }
                FileState::Pending | FileState::Failed | FileState::Copying => {
                    resume_srcs.push(row.src.clone());
                }
            }
        }
        Ok(Self::with_controls(
            db,
            bus,
            source,
            plan,
            Some(Prepared {
                job_id,
                resume_srcs: Some(resume_srcs),
                fresh: Vec::new(),
                base,
            }),
        ))
    }

    fn with_controls(
        db: Db,
        bus: EventBus,
        source: Box<dyn DeviceSource>,
        plan: ImportPlan,
        prepared: Option<Prepared>,
    ) -> Self {
        Self {
            db,
            bus,
            source: source.into(),
            plan,
            controls: EngineControls {
                paused: Arc::new(AtomicBool::new(false)),
                cancelled: Arc::new(AtomicBool::new(false)),
                done: Arc::new(AtomicBool::new(false)),
            },
            prepared,
        }
    }

    /// 控制柄（pause/cancel 与 run 并行使用）。
    pub fn controls(&self) -> EngineControls {
        self.controls.clone()
    }

    /// 建新任务：枚举源 → 宽松查重预判 → journal 全量 pending/skipped →
    /// 发 ImportSessionStarted。run() 未 begin 时内部自动调用。
    pub fn begin(&mut self) -> Result<i64, EngineError> {
        if let Some(prepared) = self.prepared.as_ref() {
            return Ok(prepared.job_id);
        }
        let entries = self.source.list()?;
        let mut base = Counters::default();
        for e in &entries {
            base.total_files += 1;
            base.total_bytes += e.size;
        }

        let plan_json = serde_json::to_string(&self.plan)
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(e.into()))?;
        let job_id = self.db.create_job_with_plan(
            "import",
            &self.source.id(),
            &self.source.name(),
            base.total_files,
            base.total_bytes,
            &plan_json,
        )?;

        // 宽松预判（§查重①）+ journal 批量落 pending/skipped
        let tx = self.db.0.unchecked_transaction()?;
        let mut fresh = Vec::new();
        for entry in entries {
            let hit = self.plan.skip_imported && loose_hit(&self.db, &entry).unwrap_or(false);
            let state = if hit {
                FileState::Skipped
            } else {
                FileState::Pending
            };
            if !hit {
                fresh.push(entry.clone());
            }
            tx.execute(
                "INSERT INTO job_files (job_id, src, dst, size, state) VALUES (?1, ?2, '', ?3, ?4)",
                rusqlite::params![job_id, entry.id, entry.size as i64, state],
            )?;
        }
        tx.commit()?;
        base.skipped = base.total_files - fresh.len() as u64;
        base.skipped_bytes = 0; // 宽松命中字节在 run 结算（journal 行没有单独统计）

        let _ = self.db.append_log("info", Some(job_id), "导入会话开始");
        self.bus.publish(AppEvent::ImportSessionStarted {
            job_id,
            total_files: base.total_files,
            total_bytes: base.total_bytes,
        });
        self.prepared = Some(Prepared {
            job_id,
            resume_srcs: None,
            fresh,
            base,
        });
        Ok(job_id)
    }

    /// 阻塞执行至完成/取消/设备失联。调用方决定线程（IPC 层 spawn）。
    pub fn run(mut self) -> JobStats {
        if self.prepared.is_none() {
            if let Err(err) = self.begin() {
                self.bus.publish(AppEvent::AppError {
                    level: "error".into(),
                    message: format!("导入启动失败: {err}"),
                    recoverable: true,
                });
                self.controls.done.store(true, Ordering::SeqCst);
                return Counters::default().into_stats(Duration::ZERO);
            }
        }
        let prepared = self.prepared.take().expect("begin ensured prepared");
        let started = Instant::now();
        let job_id = prepared.job_id;

        // resume：清除 .part 残留（上次中断的半成品，一律重做）
        self.sweep_part_files();

        // 待处理队列：resume 需重列源匹配 journal src（list 失败→按设备失联暂停）
        let queue = self.resolve_queue(&prepared);
        let mut counters = prepared.base;
        let mut device_lost = false;
        if resume_has_pending(&prepared) && queue.is_none() {
            self.bus.publish(AppEvent::DeviceUnavailable {
                id: self.source.id(),
            });
            let stats = counters.into_stats(started.elapsed());
            let _ = self.db.finish_job(
                job_id,
                "paused",
                &serde_json::to_string(&stats).unwrap_or_default(),
            );
            let _ = self
                .db
                .append_log("warn", Some(job_id), "恢复导入时设备不可用，保持暂停");
            self.controls.done.store(true, Ordering::SeqCst);
            return stats;
        }
        let queue = queue.unwrap_or_default();

        let worker_count = worker_count(self.source.kind(), self.plan.streams);
        let (work_tx, work_rx) = mpsc::channel::<FileEntry>();
        for entry in queue {
            let _ = work_tx.send(entry);
        }
        drop(work_tx);
        let work_rx = Arc::new(Mutex::new(work_rx));
        let (result_tx, result_rx) = mpsc::channel::<FileOutcome>();
        let part_seq = Arc::new(AtomicU64::new(0));

        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let source = Arc::clone(&self.source);
            let work_rx = Arc::clone(&work_rx);
            let result_tx = result_tx.clone();
            let controls = self.controls.clone();
            let part_dir = self.plan.target_root.join(PART_DIR);
            let part_seq = Arc::clone(&part_seq);
            let dir_template = self.plan.dir_template.clone();
            let name_template = self.plan.name_template.clone();
            let target_root = self.plan.target_root.clone();
            workers.push(std::thread::spawn(move || {
                loop {
                    if controls.is_cancelled() {
                        break;
                    }
                    while controls.is_paused() && !controls.is_cancelled() {
                        std::thread::sleep(PAUSE_POLL);
                    }
                    if controls.is_cancelled() {
                        break;
                    }
                    let entry = { work_rx.lock().expect("queue mutex").try_recv() };
                    let entry = match entry {
                        Ok(entry) => entry,
                        Err(_) => break, // 队列已空/关闭
                    };
                    let seq = part_seq.fetch_add(1, Ordering::Relaxed);
                    let outcome = copy_one(
                        &*source,
                        &part_dir,
                        seq,
                        &entry,
                        &target_root,
                        &dir_template,
                        &name_template,
                    );
                    if result_tx.send(outcome).is_err() {
                        break; // 收集端已退出
                    }
                }
            }));
        }
        drop(result_tx);

        let mut progress = Throttle::new(PROGRESS_INTERVAL);
        let mut next_milestone: u32 = 25;
        let mut last_completed_src = String::new();

        for outcome in result_rx {
            match outcome {
                FileOutcome::Copied(copied) => {
                    let entry = &copied.entry;
                    match self.finish_copy(job_id, &copied) {
                        Ok((FileState::Verified, dst)) => {
                            counters.done_files += 1;
                            counters.done_bytes += entry.size;
                            last_completed_src = entry.id.clone();
                            self.bus.publish(AppEvent::ImportFileCompleted {
                                job_id,
                                src: entry.id.clone(),
                                dst,
                                state: FileState::Verified,
                            });
                        }
                        Ok((FileState::Skipped, dst)) => {
                            counters.skipped += 1;
                            counters.skipped_bytes += entry.size;
                            last_completed_src = entry.id.clone();
                            self.bus.publish(AppEvent::ImportFileCompleted {
                                job_id,
                                src: entry.id.clone(),
                                dst,
                                state: FileState::Skipped,
                            });
                        }
                        Ok(_) => unreachable!("finish_copy 只返回 verified/skipped"),
                        Err(error) => {
                            counters.failed += 1;
                            counters.failed_bytes += entry.size;
                            let _ = self.db.upsert_job_file(&JobFileRow {
                                job_id,
                                src: entry.id.clone(),
                                dst: String::new(),
                                size: entry.size,
                                state: FileState::Failed,
                                error: Some(error.clone()),
                                xxhash: Some(copied.xxh),
                                sha256: Some(copied.sha),
                            });
                            let _ = self.db.append_log("error", Some(job_id), &error);
                            self.bus.publish(AppEvent::ImportFileCompleted {
                                job_id,
                                src: entry.id.clone(),
                                dst: String::new(),
                                state: FileState::Failed,
                            });
                        }
                    }
                }
                FileOutcome::Failed { entry, error } => {
                    counters.failed += 1;
                    counters.failed_bytes += entry.size;
                    let _ = self.db.upsert_job_file(&JobFileRow {
                        job_id,
                        src: entry.id,
                        dst: String::new(),
                        size: entry.size,
                        state: FileState::Failed,
                        error: Some(error.clone()),
                        xxhash: None,
                        sha256: None,
                    });
                    let _ = self.db.append_log("error", Some(job_id), &error);
                }
                FileOutcome::Disconnected { entry, error } => {
                    // 拔线：journal 回 pending（resume 续传），停工 + 设备不可用事件
                    let _ = self.db.upsert_job_file(&JobFileRow {
                        job_id,
                        src: entry.id,
                        dst: String::new(),
                        size: entry.size,
                        state: FileState::Pending,
                        error: Some(error),
                        xxhash: None,
                        sha256: None,
                    });
                    self.controls.cancelled.store(true, Ordering::SeqCst);
                    device_lost = true;
                    self.bus.publish(AppEvent::DeviceUnavailable {
                        id: self.source.id(),
                    });
                    let _ = self.db.append_log(
                        "warn",
                        Some(job_id),
                        "设备失联，导入已自动暂停（可恢复续传）",
                    );
                }
            }

            // 节流进度 + 里程碑（按已结算字节占比：done+skipped+failed）
            if progress.should_fire() {
                let secs = started.elapsed().as_secs_f64();
                self.bus.publish(AppEvent::ImportFileProgress {
                    job_id,
                    done_files: counters.done_files,
                    done_bytes: counters.done_bytes,
                    current_file: last_completed_src.clone(),
                    bytes_per_sec: if secs > 0.0 {
                        counters.done_bytes as f64 / secs
                    } else {
                        0.0
                    },
                });
            }
            if let Some(percent) = (counters.settled_bytes() * 100)
                .checked_div(counters.total_bytes)
                .map(|p| p as u32)
            {
                while next_milestone <= 100 && percent >= next_milestone {
                    self.bus.publish(AppEvent::ImportMilestoneReached {
                        job_id,
                        percent: next_milestone,
                    });
                    next_milestone += 25;
                }
            }
        }

        for worker in workers {
            let _ = worker.join();
        }

        // 收尾：清空暂存目录 + 终态 + SessionFinished
        let _ = fs::remove_dir_all(self.plan.target_root.join(PART_DIR));
        let status = if device_lost {
            "paused"
        } else if self.controls.is_cancelled() {
            "cancelled"
        } else {
            "done"
        };
        let stats = counters.into_stats(started.elapsed());
        let _ = self.db.finish_job(
            job_id,
            status,
            &serde_json::to_string(&stats).unwrap_or_default(),
        );
        let _ = self.db.append_log(
            "info",
            Some(job_id),
            &format!(
                "导入会话结束：{status}（完成 {}，跳过 {}，失败 {}）",
                stats.done_files, stats.skipped_duplicates, stats.failed_files
            ),
        );
        self.bus.publish(AppEvent::ImportSessionFinished {
            job_id,
            stats: stats.clone(),
        });
        self.controls.done.store(true, Ordering::SeqCst);
        stats
    }

    /// resume：journal src → 源条目匹配；新任务直接用 begin 的队列。
    /// 返回 None 表示源 list 失败（设备不可用）。
    fn resolve_queue(&self, prepared: &Prepared) -> Option<Vec<FileEntry>> {
        match &prepared.resume_srcs {
            None => Some(prepared.fresh.clone()),
            Some(srcs) => {
                let entries = self.source.list().ok()?;
                let by_id: HashMap<String, FileEntry> =
                    entries.into_iter().map(|e| (e.id.clone(), e)).collect();
                Some(
                    srcs.iter()
                        .filter_map(|src| by_id.get(src).cloned())
                        .collect(),
                )
            }
        }
    }

    /// 清除 target_root 暂存目录的所有 `.part` 残留（重做语义）。
    fn sweep_part_files(&self) {
        let _ = fs::remove_dir_all(self.plan.target_root.join(PART_DIR));
    }

    /// §查重②③：精确复核 + 路径冲突处理 + 原子 rename + 入库 + journal。
    /// 返回 (终态, 最终目标路径)。
    fn finish_copy(&self, job_id: i64, copied: &CopiedFile) -> Result<(FileState, String), String> {
        let entry = &copied.entry;

        // ② 全量 (size, xxhash) 精确复核
        if self.plan.skip_imported
            && self
                .db
                .find_asset_by_size_xxh(entry.size, copied.xxh)
                .map_err(|e| e.to_string())?
                .is_some()
        {
            let _ = fs::remove_file(&copied.part);
            self.upsert(job_id, entry, "", FileState::Skipped, copied);
            return Ok((FileState::Skipped, String::new()));
        }

        // ③ 目标路径冲突
        let mut final_dst = copied.dst.clone();
        if final_dst.exists() {
            match self.plan.duplicate {
                DuplicatePolicy::Rename => {
                    let parent = final_dst.parent().unwrap_or(Path::new("")).to_path_buf();
                    let name = final_dst
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    final_dst = unique_path(&parent, &name);
                }
                DuplicatePolicy::Skip | DuplicatePolicy::Ask => {
                    let _ = fs::remove_file(&copied.part);
                    self.upsert(job_id, entry, "", FileState::Skipped, copied);
                    return Ok((FileState::Skipped, String::new()));
                }
            }
        }

        // 原子落位
        if let Some(parent) = final_dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
        }
        fs::rename(&copied.part, &final_dst).map_err(|e| format!("落位失败: {e}"))?;

        // 入库（assets 同路径覆盖）
        let filename = final_dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let _ = self.db.insert_asset(&AssetRow {
            path: final_dst.to_string_lossy().into_owned(),
            filename,
            size: entry.size,
            mtime: rfc3339(entry.mtime),
            xxhash: copied.xxh,
            sha256: copied.sha,
            kind: copied.kind,
            captured_at: copied.meta.captured_at.map(rfc3339),
            camera: copied.meta.camera.clone(),
            source: "imported".into(),
            created_at: rfc3339(Utc::now()),
        });
        let dst = final_dst.to_string_lossy().into_owned();
        self.upsert(job_id, entry, &dst, FileState::Verified, copied);
        Ok((FileState::Verified, dst))
    }

    fn upsert(
        &self,
        job_id: i64,
        entry: &FileEntry,
        dst: &str,
        state: FileState,
        copied: &CopiedFile,
    ) {
        let _ = self.db.upsert_job_file(&JobFileRow {
            job_id,
            src: entry.id.clone(),
            dst: dst.to_string(),
            size: entry.size,
            state,
            error: None,
            xxhash: Some(copied.xxh),
            sha256: Some(copied.sha),
        });
    }
}

/// 工作线程产出。
enum FileOutcome {
    Copied(Box<CopiedFile>),
    Failed {
        entry: FileEntry,
        error: String,
    },
    /// 设备失联（DeviceError::Disconnected / 传输中断）：触发自动暂停。
    Disconnected {
        entry: FileEntry,
        error: String,
    },
}

struct CopiedFile {
    entry: FileEntry,
    kind: AssetKind,
    meta: MetaLite,
    xxh: u64,
    sha: [u8; 32],
    /// 暂存 .part 路径（已写满、长度已校验）。
    part: PathBuf,
    /// 渲染出的最终路径（收集端做冲突处理后 rename）。
    dst: PathBuf,
}

/// 单文件单遍复制：流式读 → 哈希/写盘/head 截存 → 长度校验。
/// dst = target_root + 渲染结果。
#[allow(clippy::too_many_arguments)]
fn copy_one(
    source: &dyn DeviceSource,
    part_dir: &Path,
    seq: u64,
    entry: &FileEntry,
    target_root: &Path,
    dir_template: &str,
    name_template: &str,
) -> FileOutcome {
    let fail = |error: String| FileOutcome::Failed {
        entry: entry.clone(),
        error,
    };
    let disconnect = |error: String| FileOutcome::Disconnected {
        entry: entry.clone(),
        error,
    };
    let io_err = |e: std::io::Error| {
        if e.kind() == std::io::ErrorKind::ConnectionAborted {
            disconnect(format!("传输中断: {e}"))
        } else {
            fail(format!("读取失败: {e}"))
        }
    };

    let mut reader = match source.stream(&entry.id) {
        Ok(reader) => reader,
        Err(DeviceError::Disconnected) => return disconnect("设备连接中断".into()),
        Err(err) => return fail(err.to_string()),
    };

    // 首段：截存 head（≤1MB）——EXIF + 魔数识别；同时进哈希与 .part
    let mut head: Vec<u8> = Vec::with_capacity(entry.size.min(HEAD_MAX as u64) as usize);
    let mut first = vec![0u8; HEAD_MAX];
    while head.len() < HEAD_MAX {
        match reader.read(&mut first[..HEAD_MAX - head.len()]) {
            Ok(0) => break,
            Ok(n) => head.extend_from_slice(&first[..n]),
            Err(e) => return io_err(e),
        }
    }

    let kind = classify(&entry.rel_path, &head);
    if kind == AssetKind::Other {
        return fail("类型识别失败（扩展名与内容不符，疑似伪装文件）".into());
    }
    let meta = exif_lite::parse(&head);

    // 渲染目标路径（{相机}/{镜头} 缺失降级默认段）
    let (stem, ext) = split_stem_ext(&entry.rel_path);
    let ctx = RenderCtx {
        captured_at: resolve_captured(meta.captured_at, entry.mtime),
        camera: meta.camera.clone(),
        lens: None,
        original_stem: stem,
        ext,
    };
    let dst = match render_dst(&ctx, dir_template, name_template) {
        Ok(rel) => target_root.join(rel),
        Err(error) => return fail(error),
    };

    // 写 .part（集中暂存目录，避免同名目标并发冲突）
    if let Err(e) = fs::create_dir_all(part_dir) {
        return fail(format!("创建暂存目录失败: {e}"));
    }
    let part = part_dir.join(format!("{seq}.part"));
    let mut out = match File::create(&part) {
        Ok(out) => out,
        Err(e) => return fail(format!("创建临时文件失败: {e}")),
    };
    let mut xxh = Xxh64::new(0);
    let mut sha = Sha256::new();
    if let Err(error) = write_chunk(&head, &mut out, &mut xxh, &mut sha, &part) {
        return fail(error);
    }

    // 剩余流：8MB 块单遍读
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = write_chunk(&chunk[..n], &mut out, &mut xxh, &mut sha, &part) {
                    return fail(error);
                }
            }
            Err(e) => return io_err(e),
        }
    }
    if let Err(e) = out.flush() {
        let _ = fs::remove_file(&part);
        return fail(format!("刷盘失败: {e}"));
    }
    drop(out); // Windows：rename 前必须关闭句柄

    // 长度校验（spec §7：任何时刻不留半文件）
    let len = match fs::metadata(&part) {
        Ok(meta) => meta.len(),
        Err(e) => {
            let _ = fs::remove_file(&part);
            return fail(format!("临时文件丢失: {e}"));
        }
    };
    if len != entry.size {
        let _ = fs::remove_file(&part);
        return fail(format!(
            "长度校验失败: 期望 {} 字节，实得 {len}",
            entry.size
        ));
    }

    FileOutcome::Copied(Box::new(CopiedFile {
        entry: entry.clone(),
        kind,
        meta,
        xxh: xxh.digest(),
        sha: sha.finalize().into(),
        part,
        dst,
    }))
}

/// 写盘 + 双哈希单遍更新；失败时清掉半成品 .part。
fn write_chunk(
    buf: &[u8],
    out: &mut File,
    xxh: &mut Xxh64,
    sha: &mut Sha256,
    part: &Path,
) -> Result<(), String> {
    if let Err(e) = out.write_all(buf) {
        let _ = fs::remove_file(part);
        return Err(format!("写盘失败: {e}"));
    }
    xxh.update(buf);
    sha.update(buf);
    Ok(())
}

/// 渲染目录+文件名 → 完整目标路径；`{相机}`/`{镜头}` 缺失时用默认段重试。
fn render_dst(ctx: &RenderCtx, dir_template: &str, name_template: &str) -> Result<PathBuf, String> {
    let render = |ctx: &RenderCtx| -> Result<String, TemplateError> {
        Ok(format!(
            "{}/{}",
            render_dir(dir_template, ctx)?,
            render_name(name_template, ctx)?
        ))
    };
    match render(ctx) {
        Ok(path) => Ok(PathBuf::from(path)),
        Err(TemplateError::MissingContext(_)) => {
            // 无 EXIF 的文件（截图/转码/损坏头）：默认段降级重试
            let fallback = RenderCtx {
                captured_at: ctx.captured_at,
                camera: Some(FALLBACK_CAMERA.to_string()),
                lens: Some(FALLBACK_LENS.to_string()),
                original_stem: ctx.original_stem.clone(),
                ext: ctx.ext.clone(),
            };
            render(&fallback)
                .map(PathBuf::from)
                .map_err(|e| e.to_string())
        }
        Err(TemplateError::UnknownToken(token)) => {
            Err(format!("命名模板含未知令牌「{token}」，请检查导入设置"))
        }
    }
}

fn split_stem_ext(rel_path: &str) -> (String, String) {
    let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem.to_string(), ext.to_string()),
        _ => (name.to_string(), String::new()),
    }
}

fn worker_count(kind: SourceKind, streams: u32) -> usize {
    match kind {
        SourceKind::Mtp => 1,
        SourceKind::Volume => 4.min(streams.max(1) as usize),
    }
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// 宽松查重键命中判定（size + filename + mtime±2s）。
fn loose_hit(db: &Db, entry: &FileEntry) -> rusqlite::Result<bool> {
    let filename = entry.rel_path.rsplit('/').next().unwrap_or(&entry.rel_path);
    let from = rfc3339(entry.mtime - MTIME_TOLERANCE);
    let to = rfc3339(entry.mtime + MTIME_TOLERANCE);
    Ok(db
        .find_asset_loose(entry.size, filename, &from, &to)?
        .is_some())
}

fn resume_has_pending(prepared: &Prepared) -> bool {
    prepared
        .resume_srcs
        .as_ref()
        .is_some_and(|srcs| !srcs.is_empty())
}
