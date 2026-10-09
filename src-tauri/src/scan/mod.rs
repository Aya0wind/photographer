//! 照片库增量扫描（2026-10-09 计划 §三「手动往库文件夹放文件」+ §八
//! 1/2/3/5/7 边界定案）。
//!
//! 单轮 [`scan_library_once`]：以库 root 递归枚举——
//! - **目录 mtime 剪枝**：dir mtime 只反映直接子项增删/改名，每目录独立
//!   判定（缓存 mtime 相同 → 跳过该目录直接文件处理；子目录仍递归 stat，
//!   深层变化不漏）；
//! - **不跟随符号链接/junction**（§八-7）：目录与文件两侧的链接条目全部
//!   跳过（Windows 上 junction 与 symlink 的 file_type().is_symlink() 同真）；
//! - **文件稳定度冷却窗**（§八-2）：mtime 距今小于冷却窗、两次探测间
//!   （读前 stat / 读毕 stat）大小或 mtime 变化、写入者持锁打不开 → 本轮
//!   跳过（大文件夹拷贝期间逐步拾取，最终收敛）；
//! - **两级识别**（§三）：file-id（卷序列号+文件 id）命中 = 硬链接零哈希
//!   直跳；否则算哈希，同库同内容去重 skip——**仅当存在在线同哈希资产**
//!   （§八-1），命中 missing 资产 = 移动/改名 → 重绑路径；
//! - **missing 哈希重绑**（§八-1）：重绑路径 + 清 missing + 边车补写新
//!   位置 + 缩略图按内容复用（缓存键换路径、mtime 段保留）；
//! - **恢复校验**（§八-1）：missing 路径重新在盘 → 校验哈希；不一致 =
//!   同名不同内容 → 重算索引、保留逻辑元数据并提示；
//! - **晚到边车补读**（§八-3）：已登记资产旁出现同名边车且 mtime 晚于
//!   登记时间 → 触发一次边车读入（与 exif 通道/rating_watch 读入方向
//!   同路径，DB 已有值不覆盖）；
//! - **missing 延迟确认**（§八-7）：连续两轮缺席才标 missing。
//!
//! **批量登记模式** [`scan_library_batch`]（M4b「从文件夹建立」§三）：
//! 同一条扫描管线（剪枝/冷却/两级识别/重绑全部照旧，登记幂等），叠加——
//! - **预点数**：先递归点一遍候选文件数作进度分母（与 walk 同一忽略
//!   规则/扩展名口径，不读内容）；
//! - **软取消**：[`BatchSignals::cancel_requested`] 轮询（每文件边界一次），
//!   true → 当前文件处理完即停；
//! - **导入让路**：[`BatchSignals::yield_now`]（import_running 旗）置位 →
//!   停止本轮，任务回队 pending，导入结束后续跑（续跑从已登记路径短路，
//!   自然增量）；
//! - **进度出口**：[`BatchProgressSink`]（ipc 层实现进度入库 +
//!   LibraryScanProgress 事件走既有节流）。
//!
//! **登记单管道**（§八-5）：所有登记段（查重判定 + 写库原子段）经
//! `register_gate` 互斥——增量扫描发现与导入落盘产出串行处理，防双写
//! 竞争与重复算哈希/缩略图（闸由 ipc 层注入；导入引擎 finish_copy/
//! finish_reference 的登记段持同一把锁）。
//!
//! 本模块不依赖 ipc/AppState（核心可独立集成测试）；轮询 worker、批量
//! 任务账本（library_scan_jobs）与让路编排在 `ipc::photo_library`。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use xxhash_rust::xxh64::Xxh64;

use crate::db::libraries::PhotosLibraryRow;
use crate::db::{AssetRow, Db};
use crate::devices::{classify, is_media_ext};
use crate::events::{AppEvent, AssetKind, EventBus, Throttle};
use crate::metadata::exif_lite::{self, MetaLite};
use crate::platform::file_registration_id;

/// 文件稳定度冷却窗（§八-2「约 10s」）：mtime 距今小于该值 → 本轮跳过。
pub const STABILITY_COOLDOWN: Duration = Duration::from_secs(10);

/// 扫描进度事件最小间隔（与导入进度同节奏）。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// 单轮扫描选项（测试注入时钟/冷却窗；生产走 Default）。
#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// 文件稳定度冷却窗（§八-2）。
    pub cooldown: Duration,
    /// 当前时间（冷却判定基准；测试注入，缺省真实时钟）。
    pub now: Option<SystemTime>,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            cooldown: STABILITY_COOLDOWN,
            now: None,
        }
    }
}

/// 单轮扫描报告（LibraryScanFinished 事件与 photo_library_scan_status
/// 的数据底座；camelCase 与前端契约对齐）。
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryScanReport {
    /// 扫描后库根是否在盘（false = 整库离线，本轮无文件处理）。
    pub online: bool,
    /// 新登记资产数（手动放文件自动入册）。
    pub registered: u64,
    /// 同库去重/硬链接 file-id 直跳跳过数。
    pub skipped: u64,
    /// 冷却窗/不稳定跳过数（下轮重试；事件口径并入 skipped）。
    pub cooled: u64,
    /// missing 哈希重绑数（移动/改名）。
    pub rebound: u64,
    /// missing 恢复数（同哈希回到原位）。
    pub restored: u64,
    /// 同名不同内容重算索引数（恢复时哈希不一致）。
    pub recomputed: u64,
    /// 本轮新标 missing 数（连续两轮缺席确认）。
    pub missing_marked: u64,
    /// 晚到边车补读次数。
    pub sidecar_read: u64,
    /// xmp_dirty 补写冲刷数（§五：离线/缺失期间的改动在库在线后补写
    /// 边车、清标志——与晚到边车读入方向对称的写回方向闭环）。
    pub xmp_flushed: u64,
}

impl LibraryScanReport {
    /// LibraryScanFinished 事件的 skipped 口径 = 去重跳过 + 冷却跳过。
    pub fn skipped_event_total(&self) -> u64 {
        self.skipped + self.cooled
    }

    /// 本轮是否实际改变了什么（增量收尾事件门槛：零动作轮不惊动 UI——
    /// 60s 轮询下空轮发 Finished 会让存储页每分钟弹一次「登记 0 张」）。
    pub fn has_changes(&self) -> bool {
        self.registered > 0
            || self.skipped > 0
            || self.rebound > 0
            || self.restored > 0
            || self.recomputed > 0
    }
}

/// 批量登记运行信号（M4b「从文件夹建立」；None 字段 = 无该控制，增量
/// 模式全 None 即退化为 [`scan_library_once`] 行为）。
#[derive(Default)]
pub struct BatchSignals<'a> {
    /// 软取消轮询（每文件边界一次；true → 当前文件处理完即停）。ipc 层
    /// 注入任务行状态查询（cancelling/行消失 = 停）。
    pub cancel_requested: Option<&'a dyn Fn() -> bool>,
    /// 导入让路旗（import_running）：置位 → 停止本轮批量，任务回队 pending。
    pub yield_now: Option<&'a std::sync::atomic::AtomicBool>,
}

/// 批量登记进度出口（进度入库；ipc 层实现写 library_scan_jobs 行，测试
/// 以桩收集）。on_total 在预点数完成时一次；on_progress 与事件同节流
///（≥100ms）推进。
pub trait BatchProgressSink {
    /// 预点数完成（进度分母）。
    fn on_total(&mut self, total: u64);
    /// 扫描推进（计数为本轮增量，累计基线由实现持有）。
    fn on_progress(&mut self, report: &LibraryScanReport);
}

/// 批量登记提前停止原因（None = 自然跑完）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchOutcome {
    /// 自然完成（任务 done）。
    Completed,
    /// 软取消（当前文件处理完停；任务行删除，未登记文件由增量扫描拾取）。
    Cancelled,
    /// 导入让路（任务回队 pending，导入结束后续跑）。
    Yielded,
    /// 库根离线（任务保持 pending，根回来续跑；库行翻 offline 已在核内）。
    Offline,
}

/// 批量登记结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchRun {
    pub report: LibraryScanReport,
    pub outcome: BatchOutcome,
}

/// 已登记资产的扫描快照（单轮内内存索引）。
#[derive(Debug, Clone)]
struct AssetBrief {
    id: i64,
    size: u64,
    /// 登记时的源 mtime（RFC3339；文件级 (size, mtime) 零哈希比对基准）。
    mtime: String,
    xxhash: u64,
    missing: bool,
    xmp_dirty: bool,
    rating: i64,
    color_label: Option<String>,
    rejected: bool,
    /// 登记时间（RFC3339；晚到边车判定基准）。
    created_at: String,
    volume_serial: Option<i64>,
}

/// 单轮库扫描（增量模式：手动放文件自动登记走这条线）。库 root 不在盘 →
/// 翻 status=offline 返回 online=false 报告（文件处理零成本）；在盘且原
/// offline → 翻回 online 正常扫。单文件失败记日志继续（不炸整轮）；报告
/// 计数见 [`LibraryScanReport`]。
///
/// 收尾事件口径：**零动作轮不发 LibraryScanFinished**——轮询周期 60s 下
/// 空轮也发会让存储页每分钟弹一次「登记 0 张」通知；只有本轮实际改变了
/// 什么（登记/跳过/重绑/恢复/重算 > 0）才发（§三「登记后通知」）。
pub fn scan_library_once(
    db: &Db,
    db_dir: &Path,
    library: &PhotosLibraryRow,
    options: &ScanOptions,
    gate: Option<&Arc<Mutex<()>>>,
    bus: Option<&EventBus>,
) -> Result<LibraryScanReport, String> {
    let (report, online, _) = run_scan(
        db,
        db_dir,
        library,
        options,
        gate,
        bus,
        BatchSignals::default(),
        None,
        None,
    )?;
    if online && report.has_changes() {
        publish_finished(bus, library, &report, registered_cross_dup(db, &library.id));
    }
    Ok(report)
}

/// 批量登记模式（M4b「从文件夹建立」§三）：同一条扫描管线 + 预点数进度
/// 分母 + 软取消 + 导入让路 + 进度出口。根离线 → [`BatchOutcome::Offline`]
///（调用方保持任务 pending，根回来续跑）；取消/让路 → 提前停止（已处理
/// 文件全部落库，未处理文件下轮拾取——续跑按已登记路径短路，自然增量）。
/// 收尾 Completed/Cancelled 都发 LibraryScanFinished（取消也是收尾，UI
/// 进度卡需要关闭信号；让路不发——导入结束后续跑时再发）。
#[allow(clippy::too_many_arguments)]
pub fn scan_library_batch<'a>(
    db: &'a Db,
    db_dir: &'a Path,
    library: &'a PhotosLibraryRow,
    options: &'a ScanOptions,
    gate: Option<&'a Arc<Mutex<()>>>,
    bus: Option<&'a EventBus>,
    signals: BatchSignals<'a>,
    mut sink: Option<&'a mut dyn BatchProgressSink>,
) -> Result<BatchRun, String> {
    // 预点数（进度分母估算）：与 walk 同一候选口径，先于扫描一遍目录树
    //（只枚举不读内容；点数与实际处理差异 = 冷却窗/魔数不符等，属估算口径）
    let mut total_hint = None;
    if Path::new(&library.root_path).is_dir() {
        let total = count_candidates(Path::new(&library.root_path));
        if let Some(sink) = sink.as_deref_mut() {
            sink.on_total(total);
        }
        total_hint = Some(total);
    }
    let (report, online, stopped) = run_scan(
        db, db_dir, library, options, gate, bus, signals, sink, total_hint,
    )?;
    let outcome = if !online {
        BatchOutcome::Offline
    } else {
        stopped.unwrap_or(BatchOutcome::Completed)
    };
    if matches!(outcome, BatchOutcome::Completed | BatchOutcome::Cancelled) {
        publish_finished(bus, library, &report, registered_cross_dup(db, &library.id));
    }
    Ok(BatchRun { report, outcome })
}

/// 共享扫描核（增量/批量两模式共用；批量模式经 signals/sink 注入控制）。
/// 返回 (报告, 库是否在线, 提前停止原因)。统一生命周期 'a：Scanner 的
/// 进度出口 `&mut dyn` 不变（invariant），各引用参数须同龄。
#[allow(clippy::too_many_arguments)]
fn run_scan<'a>(
    db: &'a Db,
    db_dir: &'a Path,
    library: &'a PhotosLibraryRow,
    options: &'a ScanOptions,
    gate: Option<&'a Arc<Mutex<()>>>,
    bus: Option<&'a EventBus>,
    signals: BatchSignals<'a>,
    sink: Option<&'a mut dyn BatchProgressSink>,
    total_hint: Option<u64>,
) -> Result<(LibraryScanReport, bool, Option<BatchOutcome>), String> {
    let root = PathBuf::from(&library.root_path);
    // 库上线状态翻转（§五 整库粒度；root 在盘性即判定）
    let online = root.is_dir();
    if !online {
        if library.status == "online" {
            db.photos_library_set_status(&library.id, "offline")
                .map_err(|e| e.to_string())?;
            if let Some(bus) = bus {
                bus.publish(AppEvent::PhotoLibrariesChanged);
            }
        }
        return Ok((LibraryScanReport::default(), false, None));
    }
    if library.status == "offline" {
        db.photos_library_set_status(&library.id, "online")
            .map_err(|e| e.to_string())?;
        if let Some(bus) = bus {
            bus.publish(AppEvent::PhotoLibrariesChanged);
        }
        // 整库回线（AfterFrame changed-media 借鉴）：库内仍标 missing 的资产
        // 即将由本轮 walk 恢复/重绑；离线期间前端缩略图管线曾按在盘性把
        // 瓦片判成 missing 终态（缓存不再重查）——提前按集合发 presence
        // 事件使这些瓦片失效重查，恢复结果随后由精准事件二次对账（幂等）。
        let missing_ids: Vec<i64> = db
            .0
            .prepare("SELECT id FROM assets WHERE library_id = ?1 AND missing = 1")
            .and_then(|mut stmt| {
                let rows = stmt.query_map([library.id.as_str()], |r| r.get::<_, i64>(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        if !missing_ids.is_empty() {
            if let Some(bus) = bus {
                bus.publish(AppEvent::AssetsPresenceChanged {
                    library_id: library.id.clone(),
                    asset_ids: missing_ids,
                });
            }
        }
    }

    let mut scanner = Scanner::new(
        db, db_dir, library, options, gate, bus, signals, sink, total_hint,
    );
    scanner.walk(&root);
    let stopped = scanner.stopped.take();
    let mut presence = std::mem::take(&mut scanner.presence_restored);
    let mut report = scanner.report;
    report.online = true;
    // presence 精准事件（轮末合并发布，去重）：本轮回在线的资产集合——前端
    // 画廊瓦片级刷新缺失角标，不做全量重拉。批量让路/取消也发（已发生的
    // 恢复必须通知到 UI）。
    presence.sort_unstable();
    presence.dedup();
    if !presence.is_empty() {
        if let Some(bus) = bus {
            bus.publish(AppEvent::AssetsPresenceChanged {
                library_id: library.id.clone(),
                asset_ids: presence,
            });
        }
    }
    // xmp_dirty 闭环（§五）：库在线 → 对脏资产补写边车、清标志（与
    // rating_watch 读入方向对称）。离线期间无从写起（本轮已在上面提前
    // 返回）；写失败保持脏，下轮重试自愈。missing 资产不冲刷——补写归
    // 恢复/重绑路径（§八-1，含新位置投影）；两轮延迟确认窗口内（已消失
    // 但尚未标 missing）的资产按在盘性再核一次，不在盘同样留给重绑。
    if let Ok(dirty) = db.assets_xmp_dirty_in_library(&library.id) {
        for (asset_id, path, rating, label, rejected) in dirty {
            let path_ref = Path::new(&path);
            if !path_ref.is_file() {
                continue;
            }
            let projected = crate::metadata::xmp::projected_rating(rating, rejected);
            let mut ok = crate::metadata::xmp::sync_rating_to_sidecar(path_ref, projected).is_ok();
            if let Some(label) = label.as_deref() {
                // DB 小写 token → XMP 标准色名（与写方向 IPC 同一投影）
                ok &= crate::metadata::xmp::sync_label_to_sidecar(
                    path_ref,
                    crate::metadata::xmp::label_to_xmp(label),
                )
                .is_ok();
            }
            if ok && db.clear_asset_xmp_dirty(asset_id).is_ok() {
                report.xmp_flushed += 1;
            }
        }
    }

    // 库统计缓存重算（登记/重绑/missing 均改变口径可见集合）
    let _ = db.photos_library_refresh_stats(&library.id);
    Ok((report, true, stopped))
}

/// 收尾事件发布（增量/批量共用；cross_dup=跨库重复计数，§四 收尾总结，
/// None=未登记新内容不计算/零值不携带）。
fn publish_finished(
    bus: Option<&EventBus>,
    library: &PhotosLibraryRow,
    report: &LibraryScanReport,
    cross_dup: Option<u64>,
) {
    if let Some(bus) = bus {
        bus.publish(AppEvent::LibraryScanFinished {
            library_id: library.id.clone(),
            registered: report.registered + report.rebound,
            skipped: report.skipped_event_total(),
            cross_library_duplicates: cross_dup,
        });
    }
}

/// 跨库重复计数（§四）：仅在发收尾事件的轮次核对（零动作轮计数不可能
/// 变化）；零值不携带（前端 optional 字段防御读取，None 不显示提示行）。
fn registered_cross_dup(db: &Db, library_id: &str) -> Option<u64> {
    db.count_cross_library_duplicates(library_id)
        .ok()
        .filter(|n| *n > 0)
}

/// 预点数（批量进度分母）：与 [`Scanner::walk`] 同一候选口径——可识别
/// 图片扩展名、跳过隐藏/系统目录与符号链接/junction；不读文件内容
///（不判冷却/魔数——分母是估算口径，见 LibraryScanStatusDto.total 注释）。
fn count_candidates(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue; // 与 walk 同语义：暂不可读的目录本轮放弃
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue; // 不跟随符号链接/junction（§八-7）
            }
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if is_ignored_dir_name(&name) || crate::platform::is_hidden_or_system(&entry) {
                    continue;
                }
                stack.push(entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            if entry
                .path()
                .extension()
                .map(|e| is_media_ext(&e.to_string_lossy()))
                .unwrap_or(false)
            {
                total += 1;
            }
        }
    }
    total
}

/// 单轮扫描的行进状态（walk 递归的上下文）。
struct Scanner<'a> {
    db: &'a Db,
    db_dir: &'a Path,
    library: &'a PhotosLibraryRow,
    cooldown: Duration,
    now: SystemTime,
    gate: Option<&'a Arc<Mutex<()>>>,
    bus: Option<&'a EventBus>,
    /// 本库在库资产（in_trash=0；精确 path 键——大小写异名走新文件路径由
    /// 同库哈希去重收敛，§八-7）。
    by_path: HashMap<String, AssetBrief>,
    /// 父目录（小写键）→ 该目录下资产 path 列表（缺席判定；大小写不敏感
    /// 卷的同文件异名不得误标 missing）。
    by_dir: HashMap<String, Vec<String>>,
    /// 已在缺席账上的资产 id（两轮确认的第二轮判定）。
    absent_ids: HashSet<i64>,
    /// missing 资产登记指纹索引：(size, mtime RFC3339) → asset_id
    ///（AfterFrame changed-media 借鉴：文件移动/改名不改 size/mtime，
    /// 新路径枚举到此零哈希直接重绑，省整文件读取）。
    by_missing_fingerprint: HashMap<(u64, String), i64>,
    /// 本轮判回在线的资产 id（presence 精准事件收集；轮末去重发布）。
    presence_restored: Vec<i64>,
    report: LibraryScanReport,
    progress: Throttle,
    /// 批量模式控制（M4b；增量模式全 None）。
    signals: BatchSignals<'a>,
    /// 批量模式进度分母（预点数；None = 增量模式，事件口径沿用运行值）。
    total_hint: Option<u64>,
    /// 批量模式进度出口（进度入库；节流与事件同拍）。
    progress_sink: Option<&'a mut dyn BatchProgressSink>,
    /// 批量提前停止原因（None = 未停止；由 [`Scanner::should_stop`] 置位）。
    stopped: Option<BatchOutcome>,
}

/// 目录忽略规则（与设备源枚举一致：点前缀 + 系统目录黑名单）。
const IGNORED_DIRS: &[&str] = &["System Volume Information", "$RECYCLE.BIN"];

fn is_ignored_dir_name(name: &str) -> bool {
    name.starts_with('.') || IGNORED_DIRS.contains(&name)
}

/// SystemTime → RFC3339（毫秒，UTC；与资产行 mtime/created_at 同形态）。
fn rfc3339(t: SystemTime) -> String {
    rfc3339_dt(chrono::DateTime::<chrono::Utc>::from(t))
}

/// DateTime → RFC3339（毫秒，UTC；EXIF 拍摄时间落库形态）。
fn rfc3339_dt(t: chrono::DateTime<chrono::Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

impl<'a> Scanner<'a> {
    fn new(
        db: &'a Db,
        db_dir: &'a Path,
        library: &'a PhotosLibraryRow,
        options: &'a ScanOptions,
        gate: Option<&'a Arc<Mutex<()>>>,
        bus: Option<&'a EventBus>,
        signals: BatchSignals<'a>,
        progress_sink: Option<&'a mut dyn BatchProgressSink>,
        total_hint: Option<u64>,
    ) -> Self {
        // 库内资产快照（排除回收站——回收站资产的文件语义不归扫描管）
        let mut by_path = HashMap::new();
        let mut by_dir: HashMap<String, Vec<String>> = HashMap::new();
        let mut by_missing_fingerprint = HashMap::new();
        if let Ok(mut stmt) = db.0.prepare(
            "SELECT id, path, size, mtime, xxhash, missing, xmp_dirty, rating, color_label, \
             rejected, created_at, volume_serial FROM assets \
             WHERE library_id = ?1 AND in_trash = 0",
        ) {
            let rows = stmt.query_map([library.id.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)? as u64,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)? as u64,
                    row.get::<_, i64>(5)? != 0,
                    row.get::<_, i64>(6)? != 0,
                    row.get::<_, i64>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)? != 0,
                    row.get::<_, String>(10)?,
                    row.get::<_, Option<i64>>(11)?,
                ))
            });
            if let Ok(rows) = rows {
                for row in rows.flatten() {
                    let (
                        id,
                        path,
                        size,
                        mtime,
                        xxhash,
                        missing,
                        xmp_dirty,
                        rating,
                        color_label,
                        rejected,
                        created_at,
                        volume_serial,
                    ) = row;
                    if let Some(parent) = Path::new(&path).parent() {
                        by_dir
                            .entry(parent.to_string_lossy().to_ascii_lowercase())
                            .or_default()
                            .push(path.clone());
                    }
                    if missing {
                        by_missing_fingerprint.insert((size, mtime.clone()), id);
                    }
                    by_path.insert(
                        path,
                        AssetBrief {
                            id,
                            size,
                            mtime,
                            xxhash,
                            missing,
                            xmp_dirty,
                            rating,
                            color_label,
                            rejected,
                            created_at,
                            volume_serial,
                        },
                    );
                }
            }
        }
        let absent_ids =
            db.0.prepare("SELECT asset_id FROM library_scan_absent WHERE library_id = ?1")
                .and_then(|mut stmt| {
                    let rows = stmt.query_map([library.id.as_str()], |row| row.get::<_, i64>(0))?;
                    rows.collect::<rusqlite::Result<Vec<_>>>()
                })
                .map(|ids| ids.into_iter().collect())
                .unwrap_or_default();
        Self {
            db,
            db_dir,
            library,
            cooldown: options.cooldown,
            now: options.now.unwrap_or_else(SystemTime::now),
            gate,
            bus,
            by_path,
            by_dir,
            absent_ids,
            by_missing_fingerprint,
            presence_restored: Vec::new(),
            report: LibraryScanReport::default(),
            progress: Throttle::new(PROGRESS_INTERVAL),
            signals,
            total_hint,
            progress_sink,
            stopped: None,
        }
    }

    /// 批量模式停止判定（幂等：首次触发记下原因，后续直接短路）。检查顺序
    /// = 取消优先于让路（用户显式动作先响）。
    fn should_stop(&mut self) -> bool {
        if self.stopped.is_some() {
            return true;
        }
        if let Some(cancel) = self.signals.cancel_requested {
            if cancel() {
                self.stopped = Some(BatchOutcome::Cancelled);
                return true;
            }
        }
        if let Some(flag) = self.signals.yield_now {
            if flag.load(std::sync::atomic::Ordering::SeqCst) {
                self.stopped = Some(BatchOutcome::Yielded);
                return true;
            }
        }
        false
    }

    /// 登记单管道锁段执行器（§八-5：查重判定+写库原子段互斥）。
    fn registered<T>(&self, work: impl FnOnce() -> T) -> T {
        match self.gate {
            Some(gate) => {
                let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                work()
            }
            None => work(),
        }
    }

    fn publish_progress(&mut self) {
        if !self.progress.should_fire() {
            return;
        }
        // 进度入库（批量模式；节流与事件同拍 ≥100ms）
        if let Some(sink) = self.progress_sink.as_mut() {
            sink.on_progress(&self.report);
        }
        if let Some(bus) = self.bus {
            // 批量模式分母 = 预点数；增量模式无分母，口径沿用运行值
            let total = self.total_hint.unwrap_or(
                self.report.registered + self.report.rebound + self.report.skipped_event_total(),
            );
            bus.publish(AppEvent::LibraryScanProgress {
                library_id: self.library.id.clone(),
                registered: self.report.registered + self.report.rebound,
                total,
            });
        }
    }

    /// 递归枚举：目录 mtime 未变 → 剪枝（跳过直接文件处理与缺席判定，
    /// 子目录仍递归——子目录自身 mtime 独立判定）。
    fn walk(&mut self, dir: &Path) {
        if self.should_stop() {
            return; // 批量取消/让路：整棵递归就地收束（父层循环同判定）
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return; // 目录暂时不可读（权限/网络抖动）：本轮放弃，下轮再来
        };
        let Ok(meta) = fs::metadata(dir) else {
            return;
        };
        let Ok(dir_mtime) = meta.modified() else {
            return;
        };
        let dir_key = dir.to_string_lossy().into_owned();
        let mtime_str = rfc3339(dir_mtime);
        let pruned = self
            .db
            .library_scan_dir_mtime(&self.library.id, &dir_key)
            .ok()
            .flatten()
            .is_some_and(|cached| cached == mtime_str);

        if pruned {
            // 晚到边车检查独立于目录剪枝（§八-3）：LR **原地覆盖写**边车不改
            // 目录 mtime——剪枝目录的已登记资产仍需 stat 边车 mtime 判定
            //（常态每资产一次 stat；mtime 未变零成本跳过）。
            let dir_lower = dir_key.to_ascii_lowercase();
            if let Some(asset_paths) = self.by_dir.get(&dir_lower).cloned() {
                for asset_path in asset_paths {
                    if let Some(brief) = self.by_path.get(&asset_path).cloned() {
                        if !brief.missing {
                            self.check_late_sidecar(&brief, Path::new(&asset_path));
                        }
                    }
                }
            }
            for entry in entries.flatten() {
                let Ok(file_type) = entry.file_type() else {
                    continue;
                };
                // 不跟随符号链接/junction（§八-7）：链接目录整体跳过
                if file_type.is_symlink() || !file_type.is_dir() {
                    continue;
                }
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if is_ignored_dir_name(&name) || crate::platform::is_hidden_or_system(&entry) {
                    continue;
                }
                self.walk(&entry.path());
            }
            return;
        }

        let mut files: Vec<(PathBuf, fs::Metadata)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue; // 链接文件同样不跟随不登记（§八-7）
            }
            if file_type.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if is_ignored_dir_name(&name) || crate::platform::is_hidden_or_system(&entry) {
                    continue;
                }
                self.walk(&entry.path());
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            // 只认可识别图片扩展名（§三；.xmp 等附属文件按晚到边车规则处理）
            let is_image = entry
                .path()
                .extension()
                .map(|e| is_media_ext(&e.to_string_lossy()))
                .unwrap_or(false);
            if !is_image {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                files.push((entry.path(), meta));
            }
        }

        // 缺席判定（本目录直接资产；剪枝目录不做——mtime 未变即无增删）
        let dir_lower = dir_key.to_ascii_lowercase();
        let asset_paths = self.by_dir.get(&dir_lower).cloned().unwrap_or_default();
        let present: HashSet<String> = files
            .iter()
            .map(|(path, _)| path.to_string_lossy().to_ascii_lowercase())
            .collect();
        let mut noted_absent = 0u64;
        for asset_path in &asset_paths {
            let Some(brief) = self.by_path.get(asset_path) else {
                continue;
            };
            if present.contains(&asset_path.to_ascii_lowercase()) {
                if self.absent_ids.contains(&brief.id) {
                    let _ = self.db.library_scan_clear_absent(brief.id);
                    // 两轮延迟确认落定（未确认为失踪）：缺席期间该资产可能被
                    // 前端缩略图管线按在盘性判成 missing 终态（缓存不再重查），
                    // 收入 presence 事件让受影响瓦片失效重查。
                    self.presence_restored.push(brief.id);
                }
                continue;
            }
            if brief.missing {
                // 已 missing：缺席账结清即可（不重复标）
                if self.absent_ids.contains(&brief.id) {
                    let _ = self.db.library_scan_clear_absent(brief.id);
                }
                continue;
            }
            // 两轮确认（§八-7）：首轮记账，次轮仍在账 → 标 missing
            let first_round = self
                .db
                .library_scan_note_absent(brief.id, &self.library.id)
                .unwrap_or(true);
            if first_round {
                noted_absent += 1;
                continue;
            }
            let _ = self.db.library_scan_clear_absent(brief.id);
            let _ = self
                .db
                .0
                .execute("UPDATE assets SET missing = 1 WHERE id = ?1", [brief.id]);
            self.report.missing_marked += 1;
        }

        let cooled_before = self.report.cooled;
        for (path, meta) in files {
            if self.should_stop() {
                break; // 批量取消/让路：当前目录余下文件留给下轮
            }
            self.process_file(&path, &meta);
            self.publish_progress();
        }
        // 收敛性（剪枝缓存写入条件）：冷却中的文件（下轮可过窗）、首轮
        // 缺席记账（§八-7 需要次轮确认）与批量提前停止（余下文件未处理）
        // 都是「未收敛」信号——该目录本轮不落 mtime 缓存，下轮强制重枚举；
        // 否则剪枝会把它们永远饿死。
        if noted_absent > 0 || self.report.cooled > cooled_before || self.stopped.is_some() {
            let _ = self.db.library_scan_dir_forget(&self.library.id, &dir_key);
        } else {
            let _ = self
                .db
                .library_scan_dir_set_mtime(&self.library.id, &dir_key, &mtime_str);
        }
    }

    /// 单文件处理：已登记（恢复/晚到边车/指纹补录）或候选新文件（两级识别）。
    fn process_file(&mut self, path: &Path, meta: &fs::Metadata) {
        let path_str = path.to_string_lossy().into_owned();
        if let Some(brief) = self.by_path.get(&path_str).cloned() {
            if brief.missing {
                self.recover_missing_asset(&brief, path, meta);
            } else {
                self.check_late_sidecar(&brief, path);
                // 文件级零哈希比对（AfterFrame changed-media 借鉴）：
                // (size, mtime) 与登记指纹一致 → 文件未变，本轮零动作直过
                //（不进任何计数——不算新文件也不算消失）。边车检查保留在闸前：
                // §八-3 LR 原地覆盖写边车不动图片指纹，不能被此闸饿死；
                // 被跳过的只有登记指纹补录（exFAT 无 file-id 资产避免每轮
                // 重试 UPDATE）。
                if brief.size == meta.len()
                    && meta.modified().map(rfc3339).unwrap_or_default() == brief.mtime
                {
                    return;
                }
                self.backfill_registration_id(&brief, path);
            }
            return;
        }
        // 候选新文件：冷却窗（§八-2）
        if self.in_cooldown(meta) {
            self.report.cooled += 1;
            return;
        }
        // 文件级零哈希快速闸（两级识别之前的便宜检查）：(size, mtime) 命中
        // missing 资产登记指纹 → 移动/改名（两者均不随改名变化）零整读直接
        // 重绑；未命中/并发已被处理才继续 file-id/哈希识别。
        let fingerprint = (
            meta.len(),
            meta.modified().ok().map(rfc3339).unwrap_or_default(),
        );
        if let Some(&asset_id) = self.by_missing_fingerprint.get(&fingerprint) {
            self.by_missing_fingerprint.remove(&fingerprint); // 同轮不重复尝试
            let reg_id = file_registration_id(path).ok();
            if self.rebind_asset(asset_id, path, meta, None, reg_id) {
                return;
            }
            // 锁内重查发现资产已非 missing（并发恢复）→ 本文件走两级识别
        }
        // 两级识别第一级：file-id（同卷同 id = 硬链接，零哈希直跳）
        if let Ok(reg) = file_registration_id(path) {
            let hit = self
                .db
                .find_asset_brief_by_volume_file_id(reg.volume_serial as i64, &reg.file_id);
            if let Ok(Some((id, _, missing))) = hit {
                if missing {
                    // 移动/改名的同一物理文件（内容不变，size/哈希沿用旧值）
                    self.rebind_asset(id, path, meta, None, None);
                } else {
                    self.report.skipped += 1;
                }
                return;
            }
        }
        // 第二级：哈希（同库范围；读毕稳定度复核）
        let Some((kind, file_meta, xxh, size)) = read_stable(path, meta) else {
            self.report.cooled += 1;
            return;
        };
        if kind == AssetKind::Other {
            return; // 扩展名可识别但魔数不符（伪装文件）：不登记不计跳过
        }
        let reg_id = file_registration_id(path).ok();
        self.register_or_skip(path, meta, kind, &file_meta, xxh, size, reg_id);
    }

    /// mtime 距今小于冷却窗（§八-2）。文件 mtime 在未来（时钟漂移）按
    /// 「刚写完」处理——同样冷却。
    fn in_cooldown(&self, meta: &fs::Metadata) -> bool {
        let Ok(mtime) = meta.modified() else {
            return true; // 拿不到 mtime = 无法判稳定：保守跳过
        };
        match self.now.duration_since(mtime) {
            Ok(age) => age < self.cooldown,
            Err(_) => true,
        }
    }

    /// 候选新文件的登记段（锁内：查重判定 + 写库原子，§八-5/§八-1）。
    #[allow(clippy::too_many_arguments)]
    fn register_or_skip(
        &mut self,
        path: &Path,
        meta: &fs::Metadata,
        kind: AssetKind,
        file_meta: &MetaLite,
        xxh: u64,
        size: u64,
        reg_id: Option<crate::platform::FileRegistrationId>,
    ) {
        let library_id = self.library.id.clone();
        let db = self.db;
        let path_str = path.to_string_lossy().into_owned();
        let filename = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mtime = meta
            .modified()
            .ok()
            .map(rfc3339)
            .unwrap_or_else(|| crate::db::now_rfc3339());
        let volume_serial = reg_id.as_ref().map(|r| r.volume_serial as i64);
        let file_id = reg_id.as_ref().map(|r| r.file_id.clone());
        // 锁内：查重判定 + 新资产登记同段原子（§八-5——只锁查重不锁 insert
        // 仍有双写窗口：后到者 REPLACE 覆盖前行）
        enum Verdict {
            Registered,
            Skipped,
            Missing(i64),
        }
        let verdict = self.registered(move || {
            match db.find_asset_brief_by_size_xxh(&library_id, size, xxh) {
                Ok(Some((id, _, true))) => Verdict::Missing(id),
                Ok(Some(_)) => Verdict::Skipped, // 在线同哈希：skip（§八-1）
                Err(_) | Ok(None) => {
                    let row = AssetRow {
                        path: path_str,
                        filename,
                        size,
                        mtime,
                        xxhash: xxh,
                        kind,
                        captured_at: file_meta.captured_at.map(rfc3339_dt),
                        camera: file_meta.camera.clone(),
                        source: "scan".into(),
                        created_at: crate::db::now_rfc3339(),
                        origin: "imported".into(),
                        width: file_meta.width,
                        height: file_meta.height,
                        iso: file_meta.iso,
                        f_number: file_meta.f_number.clone(),
                        exposure_time: file_meta.exposure_time.clone(),
                        focal_length: file_meta.focal_length.clone(),
                        lens: file_meta.lens.clone(),
                        pair_asset_id: None,
                        thumb_state: 0,
                        orientation: file_meta.deep.orientation,
                        flash: file_meta.deep.flash.clone(),
                        metering_mode: file_meta.deep.metering_mode.clone(),
                        white_balance: file_meta.deep.white_balance.clone(),
                        exposure_program: file_meta.deep.exposure_program.clone(),
                        software: file_meta.deep.software.clone(),
                        artist: file_meta.deep.artist.clone(),
                        gps_lat: file_meta.deep.gps_lat,
                        gps_lon: file_meta.deep.gps_lon,
                        rating: 0,
                        flagged: 0,
                        color_label: None,
                        rejected: 0,
                        library_id: Some(library_id),
                        missing: 0,
                        xmp_dirty: 0,
                        volume_serial,
                        file_id,
                    };
                    if db.insert_asset_with_album(&row, None, None).is_ok() {
                        Verdict::Registered
                    } else {
                        Verdict::Skipped
                    }
                }
            }
        });
        match verdict {
            Verdict::Registered => self.report.registered += 1,
            Verdict::Skipped => self.report.skipped += 1,
            Verdict::Missing(id) => {
                self.rebind_asset(id, path, meta, Some((xxh, size)), reg_id);
            }
        }
    }

    /// missing 资产重绑（§八-1）：更新路径 + 清 missing + 记录新位置登记
    /// 指纹 + 缩略图按内容复用 + 边车补写新位置（xmp_dirty/有逻辑元数据时）。
    /// 返回是否重绑成功（并发已处理/写库失败为 false——文件级指纹闸的
    /// 调用方据此回退两级识别）。
    fn rebind_asset(
        &mut self,
        asset_id: i64,
        new_path: &Path,
        meta: &fs::Metadata,
        fingerprint: Option<(u64, u64)>,
        reg_id: Option<crate::platform::FileRegistrationId>,
    ) -> bool {
        // 锁内重查（防并发轮次/导入同时重绑）
        let db = self.db;
        let row = self.registered(move || {
            db.0.query_row(
                "SELECT path, missing, xmp_dirty, rating, color_label, rejected \
                     FROM assets WHERE id = ?1",
                [asset_id],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)? != 0,
                        r.get::<_, i64>(2)? != 0,
                        r.get::<_, i64>(3)?,
                        r.get::<_, Option<String>>(4)?,
                        r.get::<_, i64>(5)? != 0,
                    ))
                },
            )
            .ok()
            .filter(|(_, missing, ..)| *missing)
        });
        let Some((old_path, _was_missing, xmp_dirty, rating, color_label, rejected)) = row else {
            // 已被并发处理（重绑/删除/恢复）：本轮不再计
            return false;
        };
        let path_str = new_path.to_string_lossy().into_owned();
        let filename = new_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mtime = meta.modified().map(rfc3339).unwrap_or_default();
        let reg = reg_id.or_else(|| file_registration_id(new_path).ok());
        let (size_sql, xxh_sql) = match fingerprint {
            // file-id 路径无哈希：内容未变，沿用登记值（size 不动）
            None => (String::new(), String::new()),
            Some((xxh, size)) => (format!(", size = {size}"), format!(", xxhash = {xxh}")),
        };
        let db = self.db;
        let updated = self.registered(move || {
            db.0.execute(
                &format!(
                    "UPDATE assets SET path = ?2, filename = ?3, mtime = ?4{size_sql}{xxh_sql}, \
                         missing = 0, volume_serial = ?5, file_id = ?6 WHERE id = ?1",
                ),
                rusqlite::params![
                    asset_id,
                    path_str,
                    filename,
                    mtime,
                    reg.as_ref().map(|r| r.volume_serial as i64),
                    reg.as_ref().map(|r| r.file_id.clone()),
                ],
            )
            .is_ok()
        });
        if !updated {
            return false;
        }
        self.report.rebound += 1;
        // presence 精准事件收集：missing → 在线（前端瓦片级刷新缺失角标）
        self.presence_restored.push(asset_id);
        // 缩略图按内容复用：旧路径键的缓存条目改名到新路径键（mtime 段保留）
        crate::thumbs::rebind_cache_keys(self.db_dir, Path::new(&old_path), new_path);
        // 边车补写新位置（与 rating 写入方向同路径）：离线期间改动待补写，
        // 或已有逻辑元数据时把 DB 真值投影落到新位置边车
        self.write_sidecar_projection(
            asset_id,
            new_path,
            rating,
            color_label.as_deref(),
            rejected,
            xmp_dirty,
        );
        true
    }

    /// DB 真值 → 边车投影落盘（rating/label；失败保 xmp_dirty 待下轮兜底）。
    fn write_sidecar_projection(
        &mut self,
        asset_id: i64,
        path: &Path,
        rating: i64,
        label: Option<&str>,
        rejected: bool,
        xmp_dirty: bool,
    ) {
        let has_metadata = xmp_dirty || rating > 0 || label.is_some() || rejected;
        if !has_metadata {
            return;
        }
        let projected = crate::metadata::xmp::projected_rating(rating, rejected);
        let mut ok = crate::metadata::xmp::sync_rating_to_sidecar(path, projected).is_ok();
        if let Some(label) = label {
            // DB 小写 token → XMP 标准色名（与写方向 IPC 同一投影；
            // label 已过 LR 五色校验，映射必命中）
            ok &= crate::metadata::xmp::sync_label_to_sidecar(
                path,
                crate::metadata::xmp::label_to_xmp(label),
            )
            .is_ok();
        }
        if ok && xmp_dirty {
            let _ = self
                .db
                .0
                .execute("UPDATE assets SET xmp_dirty = 0 WHERE id = ?1", [asset_id]);
        } else if !ok {
            if let Some(bus) = self.bus {
                bus.publish(AppEvent::AppError {
                    level: "warn".into(),
                    message: format!(
                        "文件重绑成功，但 XMP 边车补写失败（新位置 {}）",
                        path.display()
                    ),
                    recoverable: true,
                });
            }
        }
    }

    /// missing 资产的恢复校验（§八-1）：同哈希 = 单纯恢复；不一致 = 同名
    /// 不同内容 → 重算索引、保留逻辑元数据并提示。
    fn recover_missing_asset(&mut self, brief: &AssetBrief, path: &Path, meta: &fs::Metadata) {
        if self.in_cooldown(meta) {
            self.report.cooled += 1;
            return;
        }
        let Some((kind, file_meta, xxh, size)) = read_stable(path, meta) else {
            self.report.cooled += 1;
            return;
        };
        if kind == AssetKind::Other {
            return; // 内容不可识别：不动资产（missing 保持，下轮再试）
        }
        let reg = file_registration_id(path).ok();
        let same = size == brief.size && xxh == brief.xxhash;
        if same {
            // 单纯恢复：清 missing + 记录新指纹；xmp_dirty → 边车补写
            let mtime = meta.modified().map(rfc3339).unwrap_or_default();
            let db = self.db;
            let restored = self.registered(move || {
                db.0.execute(
                    "UPDATE assets SET missing = 0, mtime = ?2, volume_serial = ?3, \
                         file_id = ?4 WHERE id = ?1 AND missing = 1",
                    rusqlite::params![
                        brief.id,
                        mtime,
                        reg.as_ref().map(|r| r.volume_serial as i64),
                        reg.as_ref().map(|r| r.file_id.clone()),
                    ],
                )
                .map(|n| n > 0)
                .unwrap_or(false)
            });
            if !restored {
                return;
            }
            self.report.restored += 1;
            // presence 精准事件收集：missing → 在线（前端瓦片级刷新缺失角标）
            self.presence_restored.push(brief.id);
            if brief.xmp_dirty {
                self.write_sidecar_projection(
                    brief.id,
                    path,
                    brief.rating,
                    brief.color_label.as_deref(),
                    brief.rejected,
                    true,
                );
            }
            return;
        }
        // 同名不同内容：同路径新内容重算索引，保留逻辑元数据（评分/标签/
        // 旗标/相册引用不动）；deep EXIF 列清空由 exif 任务重填。
        let mtime = meta.modified().map(rfc3339).unwrap_or_default();
        let captured = file_meta.captured_at.map(rfc3339_dt);
        let db = self.db;
        let updated = self.registered(move || {
            db.0.execute(
                "UPDATE assets SET size = ?2, mtime = ?3, xxhash = ?4, missing = 0, \
                     captured_at = COALESCE(?5, captured_at), camera = ?6, \
                     width = NULL, height = NULL, iso = NULL, f_number = NULL, \
                     exposure_time = NULL, focal_length = NULL, orientation = NULL, \
                     flash = NULL, metering_mode = NULL, white_balance = NULL, \
                     exposure_program = NULL, software = NULL, artist = NULL, \
                     gps_lat = NULL, gps_lon = NULL, thumb_state = 0, phash = NULL, \
                     ai_indexed_at = NULL, face_indexed_at = NULL, \
                     volume_serial = ?7, file_id = ?8 \
                     WHERE id = ?1 AND missing = 1",
                rusqlite::params![
                    brief.id,
                    size as i64,
                    mtime,
                    xxh as i64,
                    captured,
                    file_meta.camera,
                    reg.as_ref().map(|r| r.volume_serial as i64),
                    reg.as_ref().map(|r| r.file_id.clone()),
                ],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
        });
        if !updated {
            return;
        }
        self.report.recomputed += 1;
        // presence 精准事件收集：同名不同内容重算后同样回到在线
        self.presence_restored.push(brief.id);
        // 索引重排：CPU 通道复位 pending + 无行补建（exif/phash 平时非登记
        // 即建——只有 thumb/eyes/blur 随入库建行；AI 通道由 ai_indexed_at=
        // NULL + 下次 kick 的 create_*_for_unindexed 补建）；近重复桶旧
        // phash 清理。
        let now = crate::db::now_rfc3339();
        let _ = self.db.0.execute(
            "UPDATE index_tasks SET state = 'pending', attempts = 0, updated_at = ?2 \
             WHERE asset_id = ?1 AND kind IN ('thumb', 'exif', 'phash')",
            rusqlite::params![brief.id, now],
        );
        // 无行补建（exif/phash 不随入库建行；唯一索引 (kind, asset_id) 幂等）
        for kind in ["thumb", "exif", "phash"] {
            let _ = self.db.0.execute(
                "INSERT OR IGNORE INTO index_tasks \
                 (kind, asset_id, state, attempts, created_at, updated_at) \
                 VALUES (?1, ?2, 'pending', 0, ?3, ?3)",
                rusqlite::params![kind, brief.id, now],
            );
        }
        let _ = self
            .db
            .0
            .execute("DELETE FROM similar_bucket WHERE asset_id = ?1", [brief.id]);
        if let Some(bus) = self.bus {
            bus.publish(AppEvent::AppError {
                level: "warn".into(),
                message: format!(
                    "缺失文件回到原位但内容已变化，已按新内容重算索引（评分等整理信息保留）：{}",
                    path.display()
                ),
                recoverable: true,
            });
        }
    }

    /// 晚到边车补读（§八-3）：同名边车 mtime 晚于登记时间 → 触发一次读入
    ///（与 exif 通道回填同路径；同 mtime 只读一次，边车再更新则再读）。
    fn check_late_sidecar(&mut self, brief: &AssetBrief, path: &Path) {
        let sidecar = crate::metadata::xmp::sidecar_path(path);
        let Ok(sidecar_meta) = fs::metadata(&sidecar) else {
            return;
        };
        let Ok(sidecar_mtime) = sidecar_meta.modified() else {
            return;
        };
        let Ok(created) = chrono::DateTime::parse_from_rfc3339(&brief.created_at) else {
            return;
        };
        let sidecar_time = chrono::DateTime::<chrono::Utc>::from(sidecar_mtime);
        if sidecar_time <= created.with_timezone(&chrono::Utc) {
            return; // 边车早于/等于登记时间：登记时已读过，非晚到
        }
        let mtime_str = rfc3339(sidecar_mtime);
        if self
            .db
            .library_scan_sidecar_read_at(brief.id)
            .ok()
            .flatten()
            .is_some_and(|read| read == mtime_str)
        {
            return; // 触发一次（§八-3）：同 mtime 不重读
        }
        let path_str = path.to_string_lossy().into_owned();
        crate::index::backfill_sidecar_tags(self.db, brief.id, &path_str);
        let _ = self.db.library_scan_sidecar_set_read(brief.id, &mtime_str);
        self.report.sidecar_read += 1;
    }

    /// 已登记资产缺登记指纹（导入旧链路/exFAT）时顺手补录（幂等，一次性）。
    fn backfill_registration_id(&mut self, brief: &AssetBrief, path: &Path) {
        if brief.volume_serial.is_some() {
            return;
        }
        let Ok(reg) = file_registration_id(path) else {
            return; // exFAT/FAT 等无 file id：走哈希路径，无指纹可记
        };
        let _ = self.db.0.execute(
            "UPDATE assets SET volume_serial = ?2, file_id = ?3 WHERE id = ?1",
            rusqlite::params![brief.id, reg.volume_serial as i64, reg.file_id],
        );
    }
}

/// 稳定读取（§八-2）：open（写入者持锁打不开 → None）→ head 截存（魔数/
/// EXIF）→ 全量 xxh64 → 读毕复核（两次探测间大小/mtime 变化 → None）。
/// 返回 (kind, EXIF-lite, xxhash, size)。
fn read_stable(path: &Path, first: &fs::Metadata) -> Option<(AssetKind, MetaLite, u64, u64)> {
    use std::io::Read;
    const HEAD_MAX: usize = 1024 * 1024;
    const CHUNK: usize = 8 * 1024 * 1024;
    let mut file = std::fs::File::open(path).ok()?; // 持锁/权限 → 本轮跳过
    let expected = first.len();
    let mut head = vec![0u8; expected.min(HEAD_MAX as u64) as usize];
    file.read_exact(&mut head).ok()?;
    let filename = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let kind = classify(&filename, &head);
    let meta = exif_lite::parse(&head);
    let mut xxh = Xxh64::new(0);
    xxh.update(&head);
    let mut read = head.len() as u64;
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match file.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                xxh.update(&chunk[..n]);
                read += n as u64;
            }
            Err(_) => return None, // 写入者干扰：不稳定
        }
    }
    if read != expected {
        return None; // 两次探测间大小变化（§八-2）
    }
    let after = fs::metadata(path).ok()?;
    if after.len() != expected || after.modified().ok()? != first.modified().ok()? {
        return None; // 读毕仍在写入：下轮再来
    }
    Some((kind, meta, xxh.digest(), expected))
}
