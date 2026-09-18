//! M1 T7 导入引擎（spec §5.2 / §7）：单文件单遍读取流水线。
//!
//! 打开源流 → 循环读 8MB chunk → xxh64+sha256 同时 update → 写 `.part`
//! （集中暂存目录）→ 首 1MB 截存 head（EXIF + 魔数识别）→ 渲染目标路径 →
//! 完成后长度校验 → 原子 rename → journal verified → assets 入库。
//!
//! 模块拆分：`pipeline`（工作线程侧的单遍读写/哈希/双目的地双写）、
//! `dedup`（宽松/精确查重键）、`fsutil`（源根推导/空目录清理/时间格式化）；
//! 本文件是编排面：Engine 状态机（begin/run/finish）、计划契约（ImportPlan）、
//! 控制柄与统计。
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

mod dedup;
mod fsutil;
mod pipeline;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::db::{AssetRow, Db, JobFileRow};
use crate::devices::{DeviceError, DeviceSource, FileEntry, SourceKind};
use crate::events::{AppEvent, EventBus, FileState, JobStats, Throttle};
use crate::import::templates::unique_path;
use crate::settings::DuplicatePolicy;

use dedup::{exact_hit, loose_hit};
use fsutil::{cleanup_empty_dirs, rfc3339, source_root_of};
use pipeline::{copy_one, CopiedFile, FileOutcome};

/// 进度事件最小间隔（spec：≥100ms）。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// 暂停时工作线程的轮询间隔。
const PAUSE_POLL: Duration = Duration::from_millis(10);
/// `.part` 集中暂存目录名（各目标根下，点前缀）。
const PART_DIR: &str = ".smartphoto-part";

/// 导入模式（M2“保留原文件”开关，spec §5.11）：
/// copy=复制（默认，源不动）；move=移动（校验入册后删源，删源失败仅告警）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    #[default]
    Copy,
    Move,
}

/// F2 双目的地导入的第二目的地：单遍读取同时写第二份（独立目录模板，
/// 同文件名模板）；第二路同样走 `.part` 暂存 + 长度校验 + journal 记录
/// （job_files.dst2）。与 move 模式互斥（begin 时拒绝）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecondTarget {
    pub target_root: PathBuf,
    pub dir_template: String,
}

/// 导入计划（IPC 契约，camelCase）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub source_id: String,
    pub target_root: PathBuf,
    pub dir_template: String,
    pub name_template: String,
    pub duplicate_policy: DuplicatePolicy,
    pub skip_imported: bool,
    pub streams: u32,
    /// 缺省 copy：journal 中的历史计划（M1 无此字段）仍可反序列化恢复。
    #[serde(default)]
    pub mode: ImportMode,
    /// F2 第二目的地；缺省 None（M1/M2 历史计划兼容）。
    #[serde(default)]
    pub second_target: Option<SecondTarget>,
    /// 向导勾选的文件（rel_path 列表）；None=全部导入。Some 时 begin 阶段
    /// 严格过滤源清单，只保留勾选文件；与源无交集 → InvalidPlan。
    /// 随 plan_json 落 journal，resume/retry 自然兼容。
    #[serde(default)]
    pub include: Option<Vec<String>>,
}

/// 引擎错误（begin 阶段：设备枚举或建任务失败）。
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("设备错误: {0}")]
    Device(#[from] DeviceError),
    #[error("无效的导入计划: {0}")]
    InvalidPlan(String),
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
    moved: u64,
    source_delete_failed: u64,
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
            moved: self.moved,
            source_delete_failed: self.source_delete_failed,
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

    /// 建新任务：计划校验 → 自我嵌套守卫 → 枚举源 → 宽松查重预判 →
    /// journal 全量 pending/skipped → 发 ImportSessionStarted。
    /// run() 未 begin 时内部自动调用。
    pub fn begin(&mut self) -> Result<i64, EngineError> {
        if let Some(prepared) = self.prepared.as_ref() {
            return Ok(prepared.job_id);
        }
        if self.plan.mode == ImportMode::Move && self.plan.second_target.is_some() {
            return Err(EngineError::InvalidPlan(
                "move 模式与双目的地（secondTarget）不能同时使用：移动入册后源已删除，\
                 无从写第二目的地"
                    .into(),
            ));
        }
        self.check_nesting()?;
        let entries = self.filter_included(self.source.list()?)?;
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
            let second = self
                .plan
                .second_target
                .as_ref()
                .map(|s| (s.target_root.clone(), s.dir_template.clone()));
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
                        second
                            .as_ref()
                            .map(|(root, tpl)| (root.as_path(), tpl.as_str())),
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
                            last_completed_src = entry.rel_path.clone();
                            // move：journal verified + assets 入库之后删源
                            //（删源失败≠导入失败：warn 日志 + 独立计数）
                            if self.plan.mode == ImportMode::Move {
                                match self.source.delete(&entry.id) {
                                    Ok(()) => counters.moved += 1,
                                    Err(err) => {
                                        counters.source_delete_failed += 1;
                                        let _ = self.db.append_log(
                                            "warn",
                                            Some(job_id),
                                            &format!(
                                                "移动后删源失败（文件已安全导入）: {}: {err}",
                                                entry.rel_path
                                            ),
                                        );
                                    }
                                }
                            }
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
                            last_completed_src = entry.rel_path.clone();
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
                                dst2: String::new(),
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
                        dst2: String::new(),
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
                        dst2: String::new(),
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

        // move：尽力清理空的源中间子目录（保留源根；MTP 无文件系统语义跳过）
        if self.plan.mode == ImportMode::Move && counters.moved > 0 {
            if let Some(root) = source_root_of(self.source.as_ref()) {
                let removed = cleanup_empty_dirs(&root);
                if removed > 0 {
                    let _ = self.db.append_log(
                        "info",
                        Some(job_id),
                        &format!("移动完成：已清理 {removed} 个空的源子目录"),
                    );
                }
            }
        }

        // 收尾：清空暂存目录（主 + 第二目的地）+ 终态 + SessionFinished
        let _ = fs::remove_dir_all(self.plan.target_root.join(PART_DIR));
        if let Some(second) = &self.plan.second_target {
            let _ = fs::remove_dir_all(second.target_root.join(PART_DIR));
        }
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
                "导入会话结束：{status}（完成 {}：复制 {}，移动 {}；跳过 {}；失败 {}；删源失败 {}）",
                stats.done_files,
                stats.done_files - stats.moved,
                stats.moved,
                stats.skipped_duplicates,
                stats.failed_files,
                stats.source_delete_failed
            ),
        );
        self.bus.publish(AppEvent::ImportSessionFinished {
            job_id,
            stats: stats.clone(),
        });
        self.controls.done.store(true, Ordering::SeqCst);
        stats
    }

    /// 向导勾选过滤（plan.include）：Some 时只保留 rel_path 在勾选集合内的
    /// 条目；无交集 → InvalidPlan（提示"所选文件均不在源中"，拒绝建任务）。
    fn filter_included(&self, entries: Vec<FileEntry>) -> Result<Vec<FileEntry>, EngineError> {
        let Some(include) = &self.plan.include else {
            return Ok(entries); // None = 全部
        };
        let selected: HashSet<&str> = include.iter().map(String::as_str).collect();
        let filtered: Vec<FileEntry> = entries
            .into_iter()
            .filter(|e| selected.contains(e.rel_path.as_str()))
            .collect();
        if filtered.is_empty() {
            return Err(EngineError::InvalidPlan(
                "所选文件均不在源中，无法导入（勾选列表与设备清单无交集）".into(),
            ));
        }
        Ok(filtered)
    }

    /// 自我嵌套守卫（spec §5.11）：文件系统源的根与任一目标根（主/第二
    /// 目的地）的 canonical 路径互为祖先（或相同）→ 拒绝。否则导入会把自己的
    /// 产出再枚举进来。目标目录可能尚未创建——先建再比较；任一侧
    /// canonicalize 失败则跳过（folder 源枚举层还有排除子树防御）。
    fn check_nesting(&self) -> Result<(), EngineError> {
        let Some(source_root) = source_root_of(self.source.as_ref()) else {
            return Ok(()); // MTP 无文件系统语义
        };
        let mut roots = vec![self.plan.target_root.clone()];
        if let Some(second) = &self.plan.second_target {
            roots.push(second.target_root.clone());
        }
        let _ = fs::create_dir_all(&self.plan.target_root);
        if let Some(second) = &self.plan.second_target {
            let _ = fs::create_dir_all(&second.target_root);
        }
        let Ok(source_canon) = fs::canonicalize(&source_root) else {
            return Ok(());
        };
        for root in roots {
            let Ok(target_canon) = fs::canonicalize(&root) else {
                continue;
            };
            if source_canon == target_canon
                || target_canon.starts_with(&source_canon)
                || source_canon.starts_with(&target_canon)
            {
                return Err(EngineError::InvalidPlan(format!(
                    "源目录与目标目录互相嵌套（源 {}，目标 {}），已拒绝导入",
                    crate::devices::folder::strip_verbatim(&source_canon),
                    crate::devices::folder::strip_verbatim(&target_canon),
                )));
            }
        }
        Ok(())
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

    /// 清除各目标根暂存目录的所有 `.part` 残留（重做语义）。
    fn sweep_part_files(&self) {
        let _ = fs::remove_dir_all(self.plan.target_root.join(PART_DIR));
        if let Some(second) = &self.plan.second_target {
            let _ = fs::remove_dir_all(second.target_root.join(PART_DIR));
        }
    }

    /// §查重②③：精确复核 + 路径冲突处理 + 原子 rename + 入库 + journal。
    /// 双目的地：两路都落位才算成功（任一失败按单文件失败，主路已落位的
    /// 撤回删除，不留半套拷贝）；journal 双记录（dst + dst2）。
    /// 返回 (终态, 最终目标路径)。
    fn finish_copy(&self, job_id: i64, copied: &CopiedFile) -> Result<(FileState, String), String> {
        let entry = &copied.entry;

        // ② 全量 (size, xxhash) 精确复核
        if self.plan.skip_imported
            && exact_hit(&self.db, entry.size, copied.xxh).map_err(|e| e.to_string())?
        {
            copied.discard_parts();
            self.upsert(job_id, entry, "", FileState::Skipped, copied);
            return Ok((FileState::Skipped, String::new()));
        }

        // ③ 目标路径冲突（主/第二目的地各自独立处理）
        let mut final_dst = copied.dst.clone();
        if final_dst.exists() {
            match self.plan.duplicate_policy {
                DuplicatePolicy::Rename => {
                    let parent = final_dst.parent().unwrap_or(Path::new("")).to_path_buf();
                    let name = final_dst
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    final_dst = unique_path(&parent, &name);
                }
                DuplicatePolicy::Skip | DuplicatePolicy::Ask => {
                    copied.discard_parts();
                    self.upsert(job_id, entry, "", FileState::Skipped, copied);
                    return Ok((FileState::Skipped, String::new()));
                }
            }
        }
        let mut final_dst2 = copied.dst2.clone();
        if let Some(dst2) = final_dst2.clone().filter(|p| p.exists()) {
            match self.plan.duplicate_policy {
                DuplicatePolicy::Rename => {
                    let parent = dst2.parent().unwrap_or(Path::new("")).to_path_buf();
                    let name = dst2
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    final_dst2 = Some(unique_path(&parent, &name));
                }
                DuplicatePolicy::Skip | DuplicatePolicy::Ask => {
                    copied.discard_parts();
                    self.upsert(job_id, entry, "", FileState::Skipped, copied);
                    return Ok((FileState::Skipped, String::new()));
                }
            }
        }

        // 原子落位：主路先行；第二目的地任一步失败则回滚主路（要么双落位要么全无）
        if let Some(parent) = final_dst.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
        }
        fs::rename(&copied.part, &final_dst).map_err(|e| format!("落位失败: {e}"))?;
        if let (Some(final_dst2), Some(part2)) = (&final_dst2, &copied.part2) {
            let second = (|| -> Result<(), String> {
                if let Some(parent) = final_dst2.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|e| format!("创建第二目的地目录失败: {e}"))?;
                }
                fs::rename(part2, final_dst2).map_err(|e| format!("第二目的地落位失败: {e}"))
            })();
            if let Err(e) = second {
                // 回滚主路刚落位的文件（仅删本文件刚 rename 的产物，不动既有文件）
                let _ = fs::remove_file(&final_dst);
                let _ = fs::remove_file(part2);
                return Err(e);
            }
        }

        // 入库（assets 同路径覆盖；第二目的地是备份拷贝，不入 assets）
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
            origin: "imported".into(),
        });
        let dst = final_dst.to_string_lossy().into_owned();
        let dst2 = final_dst2
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.upsert_dst2(job_id, entry, &dst, &dst2, FileState::Verified, copied);
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
        self.upsert_dst2(job_id, entry, dst, "", state, copied)
    }

    fn upsert_dst2(
        &self,
        job_id: i64,
        entry: &FileEntry,
        dst: &str,
        dst2: &str,
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
            dst2: dst2.to_string(),
        });
    }
}

fn worker_count(kind: SourceKind, streams: u32) -> usize {
    match kind {
        SourceKind::Mtp => 1,
        SourceKind::Volume | SourceKind::Folder => 4.min(streams.max(1) as usize),
    }
}

fn resume_has_pending(prepared: &Prepared) -> bool {
    prepared
        .resume_srcs
        .as_ref()
        .is_some_and(|srcs| !srcs.is_empty())
}
