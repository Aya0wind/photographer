//! M1 T7 导入引擎（spec §5.2 / §7）：单文件单遍读取流水线。
//!
//! 打开源流 → 循环读 8MB chunk → xxh64 update → 写 `.part`
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
use fsutil::{cleanup_empty_dirs, ensure_directory, rfc3339, source_root_of};
use pipeline::{copy_one, CopiedFile, FileOutcome};

/// 进度事件最小间隔（spec：≥100ms）。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// 实时速率窗口（用户定案 2026-09-29）：按最近 1 秒的 done_bytes 增量计算，
/// 不拿总时间除总字节——跳过/失败阶段的无流量时间一进分母，后续速度就被
/// 稀释（实测：全量重导已存在照片后，真实拷贝段速度显示偏低）。
const RATE_WINDOW: Duration = Duration::from_secs(1);
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

/// F2 双目的地导入的第二目的地：单遍读取同时写第二份（同布局公式、
/// 同文件名模板，仅根不同——`{secondRoot}/{创建YYYY}/{创建MM}/{dir_name}/
/// [{子组}/]`，2026-09-28 定案；dir_template 随布局写死一并退役；子组段
/// 随 plan.album_subgroup 追加，0022）；第二路同样走
/// `.part` 暂存 + 长度校验 + journal 记录（job_files.dst2）。与 move 模式
/// 互斥（begin 时拒绝）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecondTarget {
    pub target_root: PathBuf,
}

/// 导入计划（IPC 契约，camelCase）。布局固定不可配置（用户定案
/// 2026-09-28）：目录段没有计划字段，运行时由相册统一公式
/// [`crate::db::Db::album_item_home_rel`] 派生（`{创建YYYY}/{创建MM}/
/// {dir_name}[/{子组}]`，相册内平铺——唯一例外 = 子组段，0022 物理化随
/// `album_subgroup` 追加）；历史 journal plan_json 里的 `dirTemplate` 键
/// 反序列化时自动忽略（resume 剩余文件按当前公式重渲染落位）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub source_id: String,
    pub target_root: PathBuf,
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
    /// 导入挂相册（M9 0015，可选）：Some(album_id) 时每个资产入册与
    /// album_item 引用同事务落库（INSERT OR IGNORE 幂等）；随 plan_json
    /// 落 journal，resume 重放不重不漏。None = 行为与历史版本完全一致。
    #[serde(default)]
    pub album_id: Option<i64>,
    /// 相册内子分组（0019，可选）：入册 album_item 带 subgroup（NULL = 散在
    /// 相册根）。serde default 缺省 = None（历史 journal 兼容）。0022 物理化：
    /// Some 时目标目录在相册主目录后追加净化子组段（存储布局唯一不平铺
    /// 例外），落位公式 [`crate::db::Db::album_item_home_rel`]。
    #[serde(default)]
    pub album_subgroup: Option<String>,
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

/// 会话计数（JobStats 的累加器）。进度按“已结算”字节（done+skipped+failed）
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

    /// `active`：done_bytes 实际有增长的时间累计（跳过/失败/等待不计入）。
    /// elapsed 仍是总耗时（时长展示）；速度只按活跃时间折算，同
    /// [`RATE_WINDOW`] 的动机——无流量阶段不得稀释速度。
    fn into_stats(self, elapsed: Duration, active: Duration) -> JobStats {
        let active_secs = active.as_secs_f64();
        JobStats {
            total_files: self.total_files,
            done_files: self.done_files,
            skipped_duplicates: self.skipped,
            failed_files: self.failed,
            total_bytes: self.total_bytes,
            done_bytes: self.done_bytes,
            elapsed_ms: elapsed.as_millis() as u64,
            bytes_per_sec: if active_secs > 0.0 {
                self.done_bytes as f64 / active_secs
            } else {
                0.0
            },
            moved: self.moved,
            source_delete_failed: self.source_delete_failed,
        }
    }
}

/// 滑动窗口传输速率（进度事件用）：保留窗口内的 (时刻, done_bytes) 样本，
/// 速率=首尾字节增量/首尾时间增量。字节平坦（跳过/等待/卡死）自然归零，
/// 久远的高吞吐样本随窗口滑出衰减——总耗时除法做不到这两点。
#[derive(Debug)]
struct TransferRate {
    window: Duration,
    samples: std::collections::VecDeque<(std::time::Instant, u64)>,
}

impl TransferRate {
    fn new(window: Duration) -> Self {
        Self {
            window,
            samples: std::collections::VecDeque::new(),
        }
    }

    /// 记录一个样本（每个文件结果到达时调用一次），返回当前窗口速率。
    fn update(&mut self, now: std::time::Instant, done_bytes: u64) -> f64 {
        let cutoff = now.checked_sub(self.window);
        while let Some(&(t, _)) = self.samples.front() {
            match cutoff {
                Some(c) if t < c => {
                    self.samples.pop_front();
                }
                // 平台不支持 checked_sub（理论上仅极老系统）：退化为不清窗。
                _ => break,
            }
        }
        self.samples.push_back((now, done_bytes));
        match (self.samples.front(), self.samples.back()) {
            (Some(&(t0, b0)), Some(&(t1, b1))) if t1 > t0 && b1 > b0 => {
                (b1 - b0) as f64 / (t1 - t0).as_secs_f64()
            }
            _ => 0.0,
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
    on_asset_imported: Option<Box<dyn FnMut() + Send>>,
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
            on_asset_imported: None,
        }
    }

    /// 控制柄（pause/cancel 与 run 并行使用）。
    pub fn controls(&self) -> EngineControls {
        self.controls.clone()
    }

    /// 仅在文件落位并成功入库后通知后台索引；复制中的文件不可索引。
    pub fn set_on_asset_imported(&mut self, callback: impl FnMut() + Send + 'static) {
        self.on_asset_imported = Some(Box::new(callback));
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

        // 相册物理目录化（0018；布局改版用户定案 2026-09-28）：带 album_id
        // 的导入落 `photoRoot/{创建YYYY}/{创建MM}/{dir_name}/[{子组}/]`——
        // 布局**固定不可配置**，统一公式 [`crate::db::Db::album_item_home_rel`]
        // （外层两段 = 相册 created_at 的字面量（UTC 口径），相册级常量、
        // 相册内**平铺**——唯一例外 = 子组段（0022 物理化），plan 带子组时
        // 追加净化子组段；拍摄日分组在应用 UI（groupAssetsByDate）完成，
        // 不落存储层）。第二目的地同公式、仅根不同。此处只做相册存在性
        // 校验（早失败）；模板在 run() 派生，不落 plan_json（相册改名后
        // resume 自然跟新）。album_id 必填（None = 防御性报错，IPC 层已有
        // 同款校验）。
        match self.plan.album_id {
            Some(album_id) => {
                self.db
                    .album_home_rel(album_id)
                    .map_err(EngineError::Db)?
                    .ok_or_else(|| {
                        EngineError::InvalidPlan(format!("导入相册 {album_id} 不存在"))
                    })?;
            }
            None => {
                return Err(EngineError::InvalidPlan("必须选择相册".into()));
            }
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
                return Counters::default().into_stats(Duration::ZERO, Duration::ZERO);
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
            // 循环未起步，无活跃时间可言
            let stats = counters.into_stats(started.elapsed(), Duration::ZERO);
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

        // 布局公式派生（固定不可配置，2026-09-28 定案）：目录段 =
        // album_item_home_rel（`{创建YYYY}/{创建MM}/{dir_name}[/{子组}]`——
        // 相册内平铺，**唯一例外 = 子组段**（0022 物理化）：plan 带子组时
        // 追加净化子组段，None = 平铺现状）。
        // begin 已对新任务校验相册存在；resume 历史任务在此派生——相册
        // 被删/无 album_id 的史前计划无法落位，任务判 error 收尾。
        let dir_template = match self.plan.album_id.and_then(|aid| {
            self.db
                .album_item_home_rel(aid, self.plan.album_subgroup.as_deref())
                .ok()
                .flatten()
        }) {
            Some(template) => template,
            None => {
                let message = match self.plan.album_id {
                    Some(aid) => format!("导入相册 {aid} 不存在（恢复前已被删除）"),
                    None => "历史任务缺少相册（布局改版前暂停），无法恢复".to_string(),
                };
                self.bus.publish(AppEvent::AppError {
                    level: "error".into(),
                    message: message.clone(),
                    recoverable: true,
                });
                let _ = self.db.append_log("error", Some(job_id), &message);
                let _ = self.db.finish_job(
                    job_id,
                    "error",
                    &serde_json::to_string(&Counters::default().into_stats(Duration::ZERO, Duration::ZERO))
                        .unwrap_or_default(),
                );
                self.controls.done.store(true, Ordering::SeqCst);
                return Counters::default().into_stats(Duration::ZERO, Duration::ZERO);
            }
        };

        let worker_count = worker_count(self.source.kind(), self.plan.streams);
        let (work_tx, work_rx) = mpsc::channel::<FileEntry>();
        for entry in queue {
            let _ = work_tx.send(entry);
        }
        drop(work_tx);
        let work_rx = Arc::new(Mutex::new(work_rx));
        let (result_tx, result_rx) = mpsc::channel::<FileOutcome>();
        let part_seq = Arc::new(AtomicU64::new(0));

        // 暂存目录只在启动工作线程前创建一次。网络共享盘可能在并发 mkdir
        // 时返回 183，且目录元数据尚未传播；不能让每个文件重复参与创建。
        let part_dir = self.plan.target_root.join(PART_DIR);
        let staging_error = ensure_directory(&part_dir)
            .map_err(|e| format!("创建暂存目录失败（{}）: {e}", part_dir.display()))
            .and_then(|()| {
                if let Some(second) = &self.plan.second_target {
                    let dir = second.target_root.join(PART_DIR);
                    ensure_directory(&dir).map_err(|e| {
                        format!("创建第二目的地暂存目录失败（{}）: {e}", dir.display())
                    })?;
                }
                Ok(())
            })
            .err();

        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            let source = Arc::clone(&self.source);
            let work_rx = Arc::clone(&work_rx);
            let result_tx = result_tx.clone();
            let controls = self.controls.clone();
            let part_dir = part_dir.clone();
            let staging_error = staging_error.clone();
            let part_seq = Arc::clone(&part_seq);
            let dir_template = dir_template.clone();
            let name_template = self.plan.name_template.clone();
            let target_root = self.plan.target_root.clone();
            let second = self
                .plan
                .second_target
                .as_ref()
                .map(|s| s.target_root.clone());
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
                    let outcome = if let Some(error) = &staging_error {
                        FileOutcome::Failed {
                            entry,
                            error: error.clone(),
                        }
                    } else {
                        copy_one(
                            &*source,
                            &part_dir,
                            seq,
                            self.plan.mode == ImportMode::Move,
                            &entry,
                            &target_root,
                            &dir_template,
                            &name_template,
                            second.as_deref(),
                        )
                    };
                    if result_tx.send(outcome).is_err() {
                        break; // 收集端已退出
                    }
                }
            }));
        }
        drop(result_tx);

        let mut progress = Throttle::new(PROGRESS_INTERVAL);
        let mut last_completed_src = String::new();
        // 速率/活跃时间统计见 RATE_WINDOW 与 Counters::into_stats 注释：
        // 每个文件结果到达即记一个样本；done_bytes 有增长才算活跃时间。
        let mut rate = TransferRate::new(RATE_WINDOW);
        let mut last_tick = Instant::now();
        let mut last_done_bytes = counters.done_bytes;
        let mut active = Duration::ZERO;

        for outcome in result_rx {
            let tick_now = Instant::now();
            if counters.done_bytes > last_done_bytes {
                active += tick_now.saturating_duration_since(last_tick);
            }
            last_tick = tick_now;
            last_done_bytes = counters.done_bytes;
            let window_rate = rate.update(tick_now, counters.done_bytes);
            match outcome {
                FileOutcome::Copied(copied) => {
                    let entry = &copied.entry;
                    match self.finish_copy(job_id, &copied) {
                        Ok((FileState::Verified, dst)) => {
                            if let Some(callback) = self.on_asset_imported.as_mut() {
                                callback();
                            }
                            counters.done_files += 1;
                            counters.done_bytes += entry.size;
                            last_completed_src = entry.rel_path.clone();
                            // move：journal verified + assets 入库之后删源
                            //（删源失败≠导入失败：warn 日志 + 独立计数）。
                            // rename 快道：源已是 .part 并随主路落位——无源
                            // 可删，直接计移动成功。
                            if self.plan.mode == ImportMode::Move {
                                if copied.fast_moved {
                                    counters.moved += 1;
                                } else {
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

            // 节流进度（按已结算字节占比：done+skipped+failed）
            if progress.should_fire() {
                self.bus.publish(AppEvent::ImportFileProgress {
                    job_id,
                    done_files: counters.done_files,
                    done_bytes: counters.done_bytes,
                    settled_bytes: counters.settled_bytes(),
                    current_file: last_completed_src.clone(),
                    // 实时速度=最近 1 秒窗口增量（见 RATE_WINDOW）
                    bytes_per_sec: window_rate,
                });
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
        let stats = counters.into_stats(started.elapsed(), active);
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
        let _ = ensure_directory(&self.plan.target_root);
        if let Some(second) = &self.plan.second_target {
            let _ = ensure_directory(&second.target_root);
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
    /// Skip 落盘前的清理：常规路径删 .part 拷贝即可；rename 快道的 .part
    /// 是源文件本体——尽力 rename 回源，回不去（源目录消失等）保留 .part
    /// 并告警。跳过（duplicate=skip）绝不吞掉源数据。
    fn skip_discard(&self, job_id: i64, copied: &CopiedFile) {
        if !copied.fast_moved {
            copied.discard_parts();
            return;
        }
        let restored = self
            .source
            .local_path(&copied.entry.id)
            .and_then(|src| fs::rename(&copied.part, &src).ok());
        if restored.is_none() {
            let _ = self.db.append_log(
                "warn",
                Some(job_id),
                &format!(
                    "快道跳过撤回失败，源数据保留在暂存: {}",
                    copied.part.display()
                ),
            );
        }
    }

    /// 双目的地：两路都落位才算成功（任一失败按单文件失败，主路已落位的
    /// 撤回删除，不留半套拷贝）；journal 双记录（dst + dst2）。
    /// 返回 (终态, 最终目标路径)。
    fn finish_copy(&self, job_id: i64, copied: &CopiedFile) -> Result<(FileState, String), String> {
        let entry = &copied.entry;

        // ② 全量 (size, xxhash) 精确复核（xxh=0 哨兵 = rename 快道未哈希，
        // 精确层跳过——hash 通道补算后此层对后续导入自动就位）
        if self.plan.skip_imported
            && copied.xxh != 0
            && exact_hit(&self.db, entry.size, copied.xxh).map_err(|e| e.to_string())?
        {
            self.skip_discard(job_id, copied);
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
                    self.skip_discard(job_id, copied);
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
                    self.skip_discard(job_id, copied);
                    self.upsert(job_id, entry, "", FileState::Skipped, copied);
                    return Ok((FileState::Skipped, String::new()));
                }
            }
        }

        // 原子落位：主路先行；第二目的地任一步失败则回滚主路（要么双落位要么全无）
        if let Some(parent) = final_dst.parent() {
            ensure_directory(parent).map_err(|e| format!("创建目标目录失败: {e}"))?;
        }
        fs::rename(&copied.part, &final_dst).map_err(|e| format!("落位失败: {e}"))?;
        if let (Some(final_dst2), Some(part2)) = (&final_dst2, &copied.part2) {
            let second = (|| -> Result<(), String> {
                if let Some(parent) = final_dst2.parent() {
                    ensure_directory(parent).map_err(|e| format!("创建第二目的地目录失败: {e}"))?;
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

        // 入库（assets 同路径覆盖；第二目的地是备份拷贝，不入 assets）；
        // album_id 通道：资产与相册引用同事务（相册被并发删除时跳过挂载）
        let filename = final_dst
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // 入库失败不回滚文件（已落盘的物理事实优先——下次导入靠同路径
        // 覆盖 upsert 收编），但 stderr 留痕可查：文件在、库没有 = 线索。
        if let Err(err) = self.db.insert_asset_with_album(
            // album_subgroup（0019）：入册引用带子分组命名层
            &AssetRow {
                path: final_dst.to_string_lossy().into_owned(),
                filename,
                size: entry.size,
                mtime: rfc3339(entry.mtime),
                xxhash: copied.xxh,
                kind: copied.kind,
                captured_at: copied.meta.captured_at.map(rfc3339),
                camera: copied.meta.camera.clone(),
                source: "imported".into(),
                created_at: rfc3339(Utc::now()),
                origin: "imported".into(),
                width: copied.meta.width,
                height: copied.meta.height,
                iso: copied.meta.iso,
                f_number: copied.meta.f_number.clone(),
                exposure_time: copied.meta.exposure_time.clone(),
                focal_length: copied.meta.focal_length.clone(),
                lens: copied.meta.lens.clone(),
                pair_asset_id: None,
                thumb_state: 0,
                orientation: copied.meta.deep.orientation,
                flash: copied.meta.deep.flash.clone(),
                metering_mode: copied.meta.deep.metering_mode.clone(),
                white_balance: copied.meta.deep.white_balance.clone(),
                exposure_program: copied.meta.deep.exposure_program.clone(),
                software: copied.meta.deep.software.clone(),
                artist: copied.meta.deep.artist.clone(),
                gps_lat: copied.meta.deep.gps_lat,
                gps_lon: copied.meta.deep.gps_lon,
                rating: 0,
                flagged: 0,
                color_label: None,
                rejected: 0,
            },
            self.plan.album_id,
            self.plan.album_subgroup.as_deref(),
        ) {
            eprintln!(
                "入库失败（文件已落盘不回滚）: {}: {err}",
                final_dst.display()
            );
        }
        let dst = final_dst.to_string_lossy().into_owned();
        let dst2 = final_dst2
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.upsert_dst2(job_id, entry, &dst, &dst2, FileState::Verified, copied);
        // rename 快道：xxh=0 哨兵 → 登记 hash 后台补算任务（幂等）
        if copied.fast_moved {
            let id = self.db.asset_id_by_path(&dst).ok().flatten();
            if let Some(id) = id {
                let _ = self.db.create_hash_task_for(id);
            }
        }
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

#[cfg(test)]
mod rate_tests {
    use super::TransferRate;
    use std::time::{Duration, Instant};

    /// 跳过阶段（字节平坦）不计入速率分母：60s 无流量后 1s 内拷 100MB，
    /// 速率应是 ~100MB/s 而不是 100MB/61s（总耗时法的稀释 bug）。
    #[test]
    fn skip_phase_does_not_dilute_later_speed() {
        let mut rate = TransferRate::new(Duration::from_secs(1));
        let t0 = Instant::now();
        // 60 秒跳过阶段：样本时间推进、字节不动
        let r = rate.update(t0 + Duration::from_secs(60), 0);
        assert_eq!(r, 0.0);
        // 随后 1 秒拷完 100MB（两个样本划出增量）
        rate.update(t0 + Duration::from_secs(60) + Duration::from_millis(900), 90_000_000);
        let r = rate.update(t0 + Duration::from_secs(61), 100_000_000);
        // 窗口内增量 100MB-0? 不——90MB 样本已把基线抬高，窗口≈100ms/10MB
        // 断言量级即可（≥10MB/s；总耗时法此时只有 ~1.6MB/s）
        assert!(r > 10_000_000.0, "diluted rate: {r}");
    }

    /// 字节平坦 → 0；单样本 → 0。
    #[test]
    fn flat_or_single_sample_yields_zero() {
        let mut rate = TransferRate::new(Duration::from_secs(1));
        let t0 = Instant::now();
        assert_eq!(rate.update(t0, 0), 0.0);
        assert_eq!(rate.update(t0 + Duration::from_millis(500), 0), 0.0);
        let mut solo = TransferRate::new(Duration::from_secs(1));
        assert_eq!(solo.update(t0, 1_000), 0.0);
    }

    /// 窗口外的旧样本滑出：10s 前的高吞吐不参与当前速率。
    #[test]
    fn stale_samples_fall_out_of_window() {
        let mut rate = TransferRate::new(Duration::from_secs(1));
        let t0 = Instant::now();
        rate.update(t0, 0);
        rate.update(t0 + Duration::from_millis(500), 1_000_000_000);
        // 10s 后再记样本：窗口里只剩新样本 → 0（旧吞吐已滑出）
        assert_eq!(rate.update(t0 + Duration::from_secs(10), 1_000_000_000), 0.0);
    }
}
