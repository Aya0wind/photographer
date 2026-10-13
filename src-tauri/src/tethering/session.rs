//! A tethered session pins its destination library and album for its full lifetime.
//! Each completed download is copied and registered without creating an import job.
use super::backend::{
    backend_registry, establish_backend, CameraBackend, CameraSetting, CapturedObject, TetherError,
};
use super::receipts_store::ReceiptsStore;
use crate::db::{AssetRow, Db};
use crate::events::AppEvent;
use crate::ipc::tethering::CameraDto;
use crate::ipc::SharedState;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};
use std::time::{Duration, Instant};

struct PendingCommand<'a>(&'a AtomicUsize);
impl Drop for PendingCommand<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct CapturePermit<'a>(&'a AtomicBool);
impl Drop for CapturePermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TetherPhoto {
    pub id: i64,
    pub name: String,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub library_id: String,
    pub album_id: i64,
    pub album_name: String,
    pub camera: CameraDto,
    pub settings: Vec<CameraSetting>,
    pub photos: Vec<TetherPhoto>,
    pub connected: bool,
    pub receiving: bool,
    pub error: Option<String>,
    /// 收片队列待接收对象数（后端 FIFO 真值；「N 待收」显示依据，
    /// RAW+JPG 双文件、机身连拍都按队列实数计）。
    pub pending_receipts: usize,
}
/// 联拍落库归属（照片库基准）：目标照片库 id（photos_libraries.id，事件
/// 载荷 TetheringPhotoAdded.libraryId 同源）+ 库 root（纯时间布局锚点）。
#[derive(Debug, Clone)]
pub struct TetherLibrary {
    pub id: String,
    pub root: PathBuf,
    pub organization_template:String,
}

pub struct Session {
    pub id: String,
    /// 落库归属照片库（2026-10-09 §三 联拍落库照片库基准）：id + root。
    pub library: TetherLibrary,
    pub album_id: i64,
    pub album_name: String,
    pub camera: CameraDto,
    /// 物理相机键（2026-10-11 §E：serial 优先，缺省 vid+pid+model+实例
    /// 路径）——同一物理机跨端点互斥、收据账本分文件的依据。
    pub physical_key: String,
    pub backend: Arc<dyn CameraBackend>,
    /// 崩溃安全收据账本（§B1）：`<config>/tethering/receipts/<物理键>.jsonl`；
    /// None = 无持久化注入（引擎级测试形态）。
    pub ledger: Option<Arc<ReceiptsStore>>,
    pub operation: Mutex<()>,
    pending_commands: AtomicUsize,
    capture_in_flight: AtomicBool,
    cancelled: AtomicBool,
    disconnected: AtomicBool,
    connected: AtomicBool,
    receiving: AtomicBool,
    settings: Mutex<Vec<CameraSetting>>,
    /// 参数失效收敛态（§B7）：事件只标记，拉取时按失效集合合并读取。
    convergence: Mutex<SettingsConvergence>,
    /// 最近一次真实相机参数读取（失效风暴节流，见 [`SETTINGS_READ_MIN_INTERVAL`]）。
    last_settings_read: Mutex<Option<Instant>>,
    /// 最近一次成功取景帧（取景活跃判定：参数读取间隔在活跃期间放大，
    /// 见 [`LIVE_VIEW_SETTINGS_READ_INTERVAL`]）。
    last_frame_at: Mutex<Option<Instant>>,
    /// 强制下次参数读取绕过节流（模式切换等改变**描述符可写性**的事件；
    /// 值类变化走缓存合并即可，不必绕过）。
    settings_read_bypass: AtomicBool,
    /// 收据序号源（账本 pending 的 seq；进程内单调）。
    receipt_seq: AtomicU64,
    photos: Mutex<Vec<TetherPhoto>>,
    seen: Mutex<HashSet<String>>,
    error: Mutex<Option<String>>,
}
/// 多会话存储：key = 会话 id。独占以**物理身份**为准（2026-10-11 §E：
/// 同一物理相机跨端点——wpd 与 gphoto 同报一台——同一时刻只允许一个
/// 会话，USB 控制权互斥是物理事实）；不同物理机不合并。
#[derive(Default)]
pub struct SessionStore {
    sessions: std::collections::HashMap<String, Arc<Session>>,
}

impl SessionStore {
    pub fn insert(&mut self, session: Arc<Session>) {
        self.sessions.insert(session.id.clone(), session);
    }
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.get(id).cloned()
    }
    pub fn remove(&mut self, id: &str) -> Option<Arc<Session>> {
        self.sessions.remove(id)
    }
    pub fn clear(&mut self) {
        self.sessions.clear();
    }
    /// 该相机端点（pnp_id）是否已被某个在册会话占用（快速预检）。
    pub fn camera_in_use(&self, pnp_id: &str) -> bool {
        self.sessions.values().any(|s| s.camera.pnp_id == pnp_id)
    }
    /// 该物理相机是否已被某个在册会话占用（§E 独占键：跨端点互斥）。
    pub fn physical_camera_in_use(&self, physical_key: &str) -> bool {
        self.sessions
            .values()
            .any(|s| s.physical_key == physical_key)
    }
    pub fn all(&self) -> Vec<Arc<Session>> {
        self.sessions.values().cloned().collect()
    }
}

fn current() -> &'static Mutex<SessionStore> {
    static CURRENT: OnceLock<Mutex<SessionStore>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(SessionStore::default()))
}

// ---------------------------------------------------------------------------
// 参数失效收敛（§B7）：事件只标记，拉取时按失效集合合并读取
// ---------------------------------------------------------------------------

/// 一批失效参数：`All` = 后端事件无细分粒度（整表失效）；`Ids` = 已知
/// 变化的参数 id 集合。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Invalidation {
    All,
    Ids(Vec<String>),
}

impl Invalidation {
    fn contains(&self, id: &str) -> bool {
        match self {
            Invalidation::All => true,
            Invalidation::Ids(ids) => ids.iter().any(|candidate| candidate == id),
        }
    }
}

/// 参数快照的失效账（版本号 + 失效集合）。事件到达 → `invalidate`（版本
/// +1）；UI 拉取 → `take_invalid` 拿走失效集合并决定是否重读相机。
#[derive(Debug, Default)]
struct SettingsConvergence {
    /// 失效事件计数（单调 +1；诊断/测试断言用）。
    revision: u64,
    invalid: Option<Invalidation>,
    /// 是否已有可用快照（首次拉取前必须整表读）。
    has_snapshot: bool,
}

impl SettingsConvergence {
    fn invalidate(&mut self, ids: Option<Vec<String>>) {
        self.revision = self.revision.saturating_add(1);
        // `None`（无细分粒度）与空列表都算整表失效。
        let incoming = match ids {
            Some(ids) if !ids.is_empty() => Invalidation::Ids(ids),
            _ => Invalidation::All,
        };
        // 连续事件合并：已整表失效不缩回部分失效（保守，不丢变更）。
        self.invalid = Some(match self.invalid.take() {
            None => incoming,
            Some(Invalidation::All) => Invalidation::All,
            Some(Invalidation::Ids(mut known)) => match incoming {
                Invalidation::All => Invalidation::All,
                Invalidation::Ids(fresh) => {
                    known.extend(fresh);
                    Invalidation::Ids(known)
                }
            },
        });
    }
    /// 拿走当前失效集合（None = 干净，可直接用缓存，不读相机）。
    fn take_invalid(&mut self) -> Option<Invalidation> {
        if !self.has_snapshot {
            return Some(Invalidation::All);
        }
        self.invalid.take()
    }
    /// 放回失效集合（读取节流时用）：与期间新到达的失效合并，All 不降级。
    fn restore_invalid(&mut self, invalid: Invalidation) {
        match self.invalid.take() {
            None => self.invalid = Some(invalid),
            Some(Invalidation::All) => self.invalid = Some(Invalidation::All),
            Some(Invalidation::Ids(mut known)) => match invalid {
                Invalidation::All => self.invalid = Some(Invalidation::All),
                Invalidation::Ids(fresh) => {
                    known.extend(fresh);
                    self.invalid = Some(Invalidation::Ids(known));
                }
            },
        }
    }
    fn mark_snapshot(&mut self) {
        self.has_snapshot = true;
        self.invalid = None;
    }
}

/// 合并失效项：缓存的非失效项原样保留；失效项用新快照替换（相机不再
/// 报告的失效项移除；新出现的失效项追加在后）。「只重读失效项」的读取
/// 语义在快照层体现为——干净状态下不读相机、部分失效时非失效项不采纳
/// 新值（它们本轮未被重读，缓存即最新已知值）。
fn merge_settings(
    cached: Vec<CameraSetting>,
    fresh: Vec<CameraSetting>,
    invalid: &Invalidation,
) -> Vec<CameraSetting> {
    let mut merged: Vec<CameraSetting> = cached
        .into_iter()
        .filter_map(|setting| {
            if !invalid.contains(&setting.id) {
                return Some(setting);
            }
            fresh
                .iter()
                .find(|candidate| candidate.id == setting.id)
                .cloned() // 新快照缺席 → 相机已不再报告该参数，移除
        })
        .collect();
    for candidate in fresh {
        if invalid.contains(&candidate.id)
            && !merged.iter().any(|setting| setting.id == candidate.id)
        {
            merged.push(candidate); // 失效集合里新出现的参数
        }
    }
    merged
}

/// 无参数事件能力相机的低频兜底节流（§B7）：距上次全量刷新 ≥20s 才允许
/// 再刷（`last=None` 视为立即可刷）。`now` 注入以便测试。
fn fallback_refresh_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|last| now.duration_since(last) >= FALLBACK_SETTINGS_INTERVAL)
}

/// 兜底全量刷新间隔。
const FALLBACK_SETTINGS_INTERVAL: Duration = Duration::from_secs(20);

/// 失效风暴读取节流（2026-10-09 联机取景卡顿）：取景期间相机持续上报
/// 属性变更（测光类），事件 → 前端拉取 → 真实相机读取若不加节流会近乎
/// 连续占用相机通道，取景帧排队掉帧。距上次真实读取 ≥1s 才允许再读；
/// 不满足时返回缓存并把失效标记原样放回（不丢失，仅推迟 ≤1s）。
const SETTINGS_READ_MIN_INTERVAL: Duration = Duration::from_millis(1000);

/// 取景活跃期间的读取间隔：gphoto 等后端的一次全量参数读取要持相机锁
/// 数百毫秒到秒级（索尼 set-parameter 实测 1.3-2s）——取景中每次读取都
/// 是一次预览冻结。取景活跃（近 2s 内有帧到达）时把读取间隔放大到 10s：
/// 一次冻结束死 ≤1 次/10s；机身拨盘改动最迟 10s 反映到面板（用户改参
/// 走 set_setting 回读，不受影响）。
const LIVE_VIEW_SETTINGS_READ_INTERVAL: Duration = Duration::from_secs(10);

/// 判定「取景活跃」的帧静默窗口（最近一帧距今不足此值 = 活跃）。
const LIVE_VIEW_ACTIVE_WINDOW: Duration = Duration::from_secs(2);

/// 读取是否到期（`last=None` 视为立即可读；间隔由调用方按取景状态选择）。
/// `now` 注入以便测试。
fn settings_read_due(last: Option<Instant>, now: Instant, min_interval: Duration) -> bool {
    last.is_none_or(|last| now.duration_since(last) >= min_interval)
}

pub fn get(id: &str) -> Result<Arc<Session>, String> {
    current()
        .lock()
        .unwrap()
        .get(id)
        .filter(|session| !session.is_stopping())
        .ok_or_else(|| "拍摄会话已结束".into())
}
pub fn all() -> Vec<Arc<Session>> {
    current().lock().unwrap().all()
}
impl Session {
    pub fn claim_capture(&self) -> Result<CapturePermit<'_>, String> {
        self.ensure_open()?;
        if self.receiving.load(Ordering::Acquire) {
            return Err("相机正在接收照片，请稍候".into());
        }
        self.capture_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "相机正在拍摄或接收照片，请稍候".to_string())?;
        Ok(CapturePermit(&self.capture_in_flight))
    }
    pub fn command<T>(
        &self,
        name: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.pending_commands.fetch_add(1, Ordering::AcqRel);
        let _pending = PendingCommand(&self.pending_commands);
        let started = Instant::now();
        let _operation = self.operation.lock().unwrap();
        self.ensure_open()?;
        let result = action();
        if started.elapsed() >= Duration::from_millis(500) {
            crate::devices::diagnostics::record(format!(
                "tethering command={name} camera={} elapsed={:?} error={:?}",
                self.camera.pnp_id,
                started.elapsed(),
                result.as_ref().err()
            ));
        }
        result
    }
    pub fn try_background(&self) -> Option<MutexGuard<'_, ()>> {
        if self.is_stopping()
            || self.pending_commands.load(Ordering::Acquire) > 0
            || self.capture_in_flight.load(Ordering::Acquire)
        {
            return None;
        }
        let guard = self.operation.try_lock().ok()?;
        if self.pending_commands.load(Ordering::Acquire) > 0 || self.is_stopping() {
            return None;
        }
        Some(guard)
    }
    pub fn is_stopping(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn ensure_open(&self) -> Result<(), String> {
        if self.is_stopping() || !self.connected.load(Ordering::Acquire) {
            return Err("拍摄会话已结束或相机已断开".into());
        }
        Ok(())
    }
    fn disconnect(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _operation = self.operation.lock().unwrap();
        if !self.disconnected.swap(true, Ordering::AcqRel) {
            self.backend.disconnect(&self.camera.pnp_id);
        }
        self.connected.store(false, Ordering::Release);
    }
    pub fn dto(&self) -> SessionDto {
        SessionDto {
            id: self.id.clone(),
            library_id: self.library.id.clone(),
            album_id: self.album_id,
            album_name: self.album_name.clone(),
            camera: self.camera.clone(),
            settings: self.settings.lock().unwrap().clone(),
            photos: self.photos.lock().unwrap().clone(),
            connected: self.connected.load(Ordering::Acquire),
            receiving: self.receiving.load(Ordering::Acquire),
            error: self.error.lock().unwrap().clone(),
            pending_receipts: self.backend.pending_receipts(&self.camera.pnp_id),
        }
    }
    fn record_error(&self, state: &SharedState, error: String) {
        *self.error.lock().unwrap() = Some(error.clone());
        state.bus.publish(AppEvent::TetheringStatus {
            session_id: self.id.clone(),
            connected: self.connected.load(Ordering::Acquire),
            error: Some(error),
        });
    }
    pub fn refresh_settings(&self) -> Result<Vec<CameraSetting>, String> {
        self.ensure_open()?;
        let Some(_operation) = self.try_background() else {
            return Ok(self.settings.lock().unwrap().clone());
        };
        // §B7 收敛：干净状态（无失效标记）直接用缓存，不读相机；失效则
        // 读快照后只合并失效项。前端契约不变——返回值始终是全量表。
        // 失效集合在拿到操作锁**之后**取走——锁忙时返回缓存，失效标记
        // 留给下一次拉取，不丢失。
        let Some(invalid) = self.convergence.lock().unwrap().take_invalid() else {
            return Ok(self.settings.lock().unwrap().clone());
        };
        let fresh = {
            // 强制旁路（模式切换）：消耗一次性标记，无视两种节流直接读。
            let bypass = self.settings_read_bypass.swap(false, Ordering::AcqRel);
            if !bypass {
                let live_view_active = self
                    .last_frame_at
                    .lock()
                    .unwrap()
                    .is_some_and(|at| Instant::now().duration_since(at) < LIVE_VIEW_ACTIVE_WINDOW);
                let min_interval = if live_view_active {
                    LIVE_VIEW_SETTINGS_READ_INTERVAL
                } else {
                    SETTINGS_READ_MIN_INTERVAL
                };
                if !settings_read_due(
                    *self.last_settings_read.lock().unwrap(),
                    Instant::now(),
                    min_interval,
                ) {
                    self.convergence.lock().unwrap().restore_invalid(invalid);
                    return Ok(self.settings.lock().unwrap().clone());
                }
            }
            let fresh = self
                .backend
                .settings(&self.camera.pnp_id)
                .map_err(|e| e.to_string())?;
            *self.last_settings_read.lock().unwrap() = Some(Instant::now());
            fresh
        };
        let merged = match invalid {
            Invalidation::All => fresh.clone(),
            Invalidation::Ids(_) => {
                let cached = std::mem::take(&mut *self.settings.lock().unwrap());
                merge_settings(cached, fresh.clone(), &invalid)
            }
        };
        *self.settings.lock().unwrap() = merged.clone();
        self.convergence.lock().unwrap().mark_snapshot();
        Ok(merged)
    }
    /// 参数事件到达：标记对应参数失效（版本号 +1）。`ids=None` 表示后端
    /// 事件无细分粒度（整表失效）。
    fn invalidate_settings(&self, ids: Option<Vec<String>>) {
        self.convergence.lock().unwrap().invalidate(ids);
    }
    /// 取景帧到达打点（IPC 取帧成功时调用）：参数读取节流的取景活跃判定。
    pub fn note_frame(&self) {
        *self.last_frame_at.lock().unwrap() = Some(Instant::now());
    }
    /// 强制下一次参数读取绕过节流（一次性）：模式切换等改变描述符可写性
    /// 的事件用——面板的可调参数集合依赖最新 writable，不能等节流窗口。
    pub fn force_next_settings_read(&self) {
        self.settings_read_bypass.store(true, Ordering::Release);
    }
    /// 兜底全量刷新（§B7：无参数事件能力的相机 20s 一次；节流由调用方
    /// 的 `fallback_refresh_due` 把关）。刷新后向窗口广播重拉。
    fn fallback_refresh_settings(&self, state: &SharedState) {
        let Some(_operation) = self.try_background() else {
            return;
        };
        if let Ok(fresh) = self.backend.settings(&self.camera.pnp_id) {
            *self.last_settings_read.lock().unwrap() = Some(Instant::now());
            *self.settings.lock().unwrap() = fresh;
            self.convergence.lock().unwrap().mark_snapshot();
            state.bus.publish(AppEvent::TetheringSettingsChanged {
                session_id: self.id.clone(),
            });
        }
    }
    pub fn set_setting(&self, id: &str, value: &str) -> Result<Vec<CameraSetting>, String> {
        self.command("set-parameter", || {
            self.backend
                .set_setting(&self.camera.pnp_id, id, value)
                .map_err(|e| e.to_string())?;
            // 写入和回读在同一次操作内，Sony 的异步属性需要以回读确认。
            let deadline = Instant::now() + Duration::from_millis(1500);
            loop {
                let settings = self
                    .backend
                    .settings(&self.camera.pnp_id)
                    .map_err(|e| e.to_string())?;
                *self.last_settings_read.lock().unwrap() = Some(Instant::now());
                let confirmed = settings.iter().any(|setting| {
                    setting.id == id
                        && (setting.current == value
                            || setting.kind == super::backend::SettingKind::Action
                            || (setting.kind == super::backend::SettingKind::Range
                                && setting
                                    .current
                                    .parse::<f64>()
                                    .ok()
                                    .zip(value.parse::<f64>().ok())
                                    .is_some_and(|(a, b)| (a - b).abs() < 0.0001)))
                });
                *self.settings.lock().unwrap() = settings.clone();
                if confirmed {
                    // 回读确认的快照是权威值：清空失效账。
                    self.convergence.lock().unwrap().mark_snapshot();
                    // The mode can be acknowledged before dependent exposure
                    // descriptors update. Force the next refresh to reread
                    // them——模式切换改变可写性（A 档快门只读/M 档可调），
                    // 面板更新依赖描述符重读，绕过节流立即执行。
                    if id == "expprogram" {
                        self.invalidate_settings(None);
                        self.force_next_settings_read();
                    }
                    return Ok(settings);
                }
                self.ensure_open()?;
                if Instant::now() >= deadline {
                    let actual = settings.iter().find(|setting| setting.id == id)
                        .map(|setting| setting.current.as_str()).unwrap_or("不可用");
                    crate::devices::diagnostics::record(format!(
                        "tethering parameter unconfirmed backend={} id={id} requested={value} actual={actual}",
                        self.backend.id()
                    ));
                    return Err(format!("相机未确认参数 {id} 的修改（请求 {value}，实际 {actual}），请检查拍摄模式或机身控制"));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    }
}
/// 联拍目标照片库解析（2026-10-09 审计 P1）：相册现有资产的 library_id
/// 众数（SQL GROUP BY 计数取最大——新拍照片归入相册主体所在库，多库时
/// 不再把整册新片灌进首库）；空相册（无在册资产）回退首库（无相册
/// 上下文，与监视文件夹同策略）。众数指向已移除登记的库 → 明确报错。
pub fn resolve_tether_library(
    db: &Db,
    album_id: i64,
) -> Result<crate::db::libraries::PhotosLibraryRow, String> {
    let libraries = db.photos_library_list().map_err(|e| e.to_string())?;
    let majority: Option<String> = db
        .0
        .query_row(
            "SELECT a.library_id FROM album_item i JOIN assets a ON a.id = i.asset_id \
             WHERE i.album_id = ?1 AND a.library_id IS NOT NULL \
             GROUP BY a.library_id ORDER BY COUNT(*) DESC, a.library_id LIMIT 1",
            [album_id],
            |r| r.get(0),
        )
        .ok();
    match majority {
        Some(id) => libraries
            .into_iter()
            .find(|library| library.id == id)
            .ok_or_else(|| "相册资产归属的照片库已不存在，无法开始联拍".into()),
        None => libraries
            .into_iter()
            .next()
            .ok_or_else(|| "尚未建立照片库".into()),
    }
}

pub fn start(state: SharedState, album_id: i64, camera_id: &str) -> Result<SessionDto, String> {
    let mut guard = current().lock().unwrap();
    if guard.camera_in_use(camera_id) {
        return Err("该相机已在联机拍摄会话中，请先结束其窗口".into());
    }
    // 2026-10-09 审计 P1：目标照片库 = 相册现有资产的 library_id 众数
    //（新拍照片归相册主体库）；空相册回退首个登记库（无相册上下文）。
    let db = crate::ipc::app_database_db(&state)?;
    let album_name: String =
        db.0.query_row("SELECT name FROM album WHERE id=?1", [album_id], |r| {
            r.get(0)
        })
        .map_err(|_| "相册不存在")?;
    let library = resolve_tether_library(&db, album_id)?;
    let library = TetherLibrary {
        id: library.id,
        root: PathBuf::from(library.root_path),
    organization_template: library.organization_template,
};
    // 2026-10-11 §E-2 自动后端选择：原生 → 已安装 SDK → libgphoto2，按
    // connect 探测的实际能力选路（探测不触发快门/改设置）；存在未确认
    // 收据的相机被钉在产生收据的后端上（选择函数内返回 Err 说明）。
    // 账本目录与 db_dir 同源注入（state.config_dir，不新建全局）。
    let receipts_dir = state.config_dir.join("tethering").join("receipts");
    let (backend, info) = establish_backend(
        backend_registry().all(),
        camera_id,
        Some(&receipts_dir),
    )?;
    // §E-3 物理独占：同一物理相机跨端点（wpd 与 gphoto 同报一台）互斥——
    // 与插入存储在存储锁下原子判定，冲突时释放刚建立的连接。
    let physical_key = info.physical_key();
    if guard.physical_camera_in_use(&physical_key) {
        backend.disconnect(camera_id);
        return Err(
            "该相机已在联机拍摄会话中（同一物理相机的其他连接端点），请先结束其窗口".into(),
        );
    }
    let ledger = Arc::new(ReceiptsStore::open(&receipts_dir, &physical_key));
    let mut camera: CameraDto = info.into();
    if camera_id.starts_with("sony-sdk:") {
        camera.name = "Sony A7R V".into();
    }
    let settings = match backend.settings(camera_id) {
        Ok(value) => value,
        Err(e) => {
            backend.disconnect(camera_id);
            return Err(e.to_string());
        }
    };
    let session = Arc::new(Session {
        id: uuid::Uuid::new_v4().to_string(),
        library,
        album_id,
        album_name,
        camera,
        physical_key,
        backend,
        ledger: Some(ledger),
        operation: Mutex::new(()),
        pending_commands: AtomicUsize::new(0),
        capture_in_flight: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        disconnected: AtomicBool::new(false),
        connected: AtomicBool::new(true),
        receiving: AtomicBool::new(false),
        settings: Mutex::new(settings),
        convergence: Mutex::new(SettingsConvergence {
            revision: 0,
            invalid: None,
            has_snapshot: true,
        }),
        last_settings_read: Mutex::new(None),
        last_frame_at: Mutex::new(None),
        settings_read_bypass: AtomicBool::new(false),
        receipt_seq: AtomicU64::new(0),
        photos: Mutex::new(Vec::new()),
        seen: Mutex::new(HashSet::new()),
        error: Mutex::new(None),
    });
    let dto = session.dto();
    guard.insert(session.clone());
    drop(guard);
    // §B1 崩溃恢复：载入未确认收据并优先按对象 id 主动收取（恢复的照
    // 片走与正常收片完全相同的 receive/ingest/ack 路径）。
    recover_unacked(&session);
    std::thread::spawn(move || {
        let mut notified_revision = session.backend.settings_revision(&session.camera.pnp_id);
        let mut notified_mode_revision = session.backend.mode_revision(&session.camera.pnp_id);
        let mut last_notification = Instant::now();
        // §B7 兜底：无参数事件能力（revision 恒 0）的相机 20s 全量刷新一次。
        let mut last_fallback = Instant::now();
        let mut consecutive_errors = 0;
        while !session.cancelled.load(Ordering::Acquire) {
            // 轮询也纳入操作锁：旧会话清理后绝不能再操作相机，影响新会话。
            let Some(operation) = session.try_background() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            if session.cancelled.load(Ordering::Acquire) {
                break;
            }
            match session.backend.poll_objects(&session.camera.pnp_id) {
                Ok(objects) => {
                    consecutive_errors = 0;
                    // 收片与用户操作（拍摄/参数）互斥：断开清理 incoming
                    // 目录时不会删到在途文件
                    for object in &objects {
                        if let Err(error) = receive(&state, &session, object) {
                            session.record_error(&state, error);
                        }
                    }
                    drop(operation);
                    let revision = session.backend.settings_revision(&session.camera.pnp_id);
                    // 曝光模式切换（机身转盘/软件改 expprogram 的另一侧回声）：
                    // 改变描述符可写性——立即强制重读（绕过节流），通知不受
                    // 100ms 合并节流（面板可调参数集合不能等窗口）。
                    let mode_revision = session.backend.mode_revision(&session.camera.pnp_id);
                    if mode_revision != notified_mode_revision {
                        session.invalidate_settings(None);
                        session.force_next_settings_read();
                        state.bus.publish(AppEvent::TetheringSettingsChanged {
                            session_id: session.id.clone(),
                        });
                        notified_mode_revision = mode_revision;
                    }
                    // 短暂合并参数事件，及时同步模式切换带来的可写性变化。
                    if revision != notified_revision
                        && last_notification.elapsed() >= Duration::from_millis(100)
                    {
                        // §B7：通知前先标记失效（版本号 +1），窗口下一次
                        // 拉取 settings 时才重读失效项并合并。
                        session.invalidate_settings(None);
                        state.bus.publish(AppEvent::TetheringSettingsChanged {
                            session_id: session.id.clone(),
                        });
                        notified_revision = revision;
                        last_notification = Instant::now();
                    }
                    // §B7 低频兜底：事件能力缺失（revision 恒 0）时 20s
                    // 全量刷新一次；取景轮询不读参数（per-frame 保持禁用）。
                    if revision == 0 && fallback_refresh_due(Some(last_fallback), Instant::now())
                    {
                        session.fallback_refresh_settings(&state);
                        last_fallback = Instant::now();
                    }
                    // 排空模式（高速连拍）：拿到事件立即回轮（驱动侧事件
                    // 还在排队）；空轮才歇——固定 200ms 会把相机自拍收片
                    // 上限压到 ~5 张/秒
                    let pace = poll_pace_ms(objects.len());
                    if pace > 0 {
                        std::thread::sleep(Duration::from_millis(pace));
                    }
                }
                Err(error) => {
                    consecutive_errors += 1;
                    if matches!(error, TetherError::Disconnected | TetherError::AccessDenied)
                        || consecutive_errors >= 3
                    {
                        session.connected.store(false, Ordering::Release);
                        session.record_error(&state, error.to_string());
                        break;
                    }
                    session.record_error(&state, error.to_string());
                    drop(operation);
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        // 与在途操作互斥后再断开（gp_camera_exit + 删收片暂存目录）
        stop(&session.id);
    });
    Ok(dto)
}

/// §B1 崩溃/重连恢复：从磁盘账本载入未确认收据（最后一态非 acked），
/// 通过 `request_objects` 优先按对象 id 主动收取——后端把仍存在的对象
/// 重新入收片队列，poller 随后走正常收片路径；后端不支持按需收取
///（返回空）或对象已消失（瞬态 id）则退回事件驱动等待，账本保留。
fn recover_unacked(session: &Session) {
    let Some(ledger) = session.ledger.as_ref() else {
        return;
    };
    let unacked = ledger.unacked();
    if unacked.is_empty() {
        return;
    }
    let ids: Vec<String> = unacked.iter().map(|receipt| receipt.id.clone()).collect();
    crate::devices::diagnostics::record(format!(
        "tethering recovery camera={} unacked={} backend={}",
        session.physical_key,
        ids.len(),
        session.backend.id()
    ));
    match session
        .backend
        .request_objects(&session.camera.pnp_id, &ids)
    {
        Ok(requeued) => {
            crate::devices::diagnostics::record(format!(
                "tethering recovery requeued={} dropped={}",
                requeued.len(),
                ids.len() - requeued.len()
            ));
        }
        Err(error) => {
            // 不阻断会话：退回事件驱动等待，未确认条目保留待后续恢复。
            crate::devices::diagnostics::record(format!(
                "tethering recovery unavailable: {error}"
            ));
        }
    }
}
/// poller 节奏：空轮歇 200ms；拿到事件立即回轮（排空模式——高速连拍
/// 时驱动侧事件还在排队，固定间隔会把收片上限压到 ~5 张/秒）。
fn poll_pace_ms(received: usize) -> u64 {
    if received == 0 {
        200
    } else {
        0
    }
}

pub fn stop(id: &str) {
    // 断开完成后才解除相机占用；后端断开幂等，避免旧线程误断开新会话。
    let session = current().lock().unwrap().get(id);
    if let Some(session) = session {
        session.disconnect();
        current().lock().unwrap().remove(id);
    }
}
/// 结束全部联机会话（切库/删库联动：会话锚定旧库的相册与落库路径，
/// 库切换后继续收片会把照片写进错误的数据库）。先置取消再断开——
/// 收片循环在对象间检查取消位，尽量不做半程写入。
pub fn stop_all() {
    let sessions = current().lock().unwrap().all();
    for session in &sessions {
        session.cancelled.store(true, Ordering::Release);
    }
    for session in &sessions {
        session.disconnect();
    }
    current().lock().unwrap().clear();
}
/// 窗口事件回调只发取消信号，真正断开交给后台，保持 UI 响应。
pub fn request_stop(id: &str) {
    // 建会话时可能持有存储锁；窗口回调不得等待它，后台 stop 会补发取消。
    if let Ok(store) = current().try_lock() {
        if let Some(session) = store.get(id) {
            session.cancelled.store(true, Ordering::Release);
            session.connected.store(false, Ordering::Release);
        }
    }
}
pub fn capture(state: &SharedState, id: &str) -> Result<(), String> {
    let session = get(id)?;
    let _capture = session.claim_capture()?;
    session.command("capture", || {
        *session.error.lock().unwrap() = None;
        let objects = match session.backend.trigger_capture(&session.camera.pnp_id) {
            Ok(objects) => objects,
            Err(error) => {
                // 拔线等硬断连：置断开态并发事件（UI 横幅），错误文案照常透传
                if matches!(error, TetherError::Disconnected) {
                    session.connected.store(false, Ordering::Release);
                    session.record_error(state, error.to_string());
                }
                return Err(error.to_string());
            }
        };
        for object in objects {
            receive(state, &session, &object)?;
        }
        Ok(())
    })
}
fn receive(state: &SharedState, session: &Session, object: &CapturedObject) -> Result<(), String> {
    if session.is_stopping() {
        return Ok(()); // 会话收尾中（删库软停等）：剩余对象不再入册
    }
    if session.seen.lock().unwrap().contains(&object.object_id) {
        return Ok(());
    }
    if !crate::devices::is_media_ext(object.object_name.rsplit('.').next().unwrap_or("")) {
        return Ok(());
    }
    session.receiving.store(true, Ordering::Release);
    // §B1 先记 Pending：对象进入收片管线即落盘（崩溃后可恢复；写失败仅
    // 日志，绝不阻断收片）。seq 保持进程内递增，恢复时按其排序。
    if let Some(ledger) = session.ledger.as_ref() {
        let seq = session.receipt_seq.fetch_add(1, Ordering::AcqRel) + 1;
        ledger.record_pending(
            session.backend.id(),
            &object.object_id,
            seq,
            &chrono::Utc::now().to_rfc3339(),
        );
    }
    let result: Result<(), String> = (|| {
        let db = crate::ipc::app_database_db(state)?;
        // 删库软停（2026-10-09 审计 P0）：ingest 前查 pin 的照片库仍在册，
        // 不在 → 结束会话（request_stop 软信号——poller 线程此刻持操作锁，
        // 直接 stop() 会同线程重入死锁；循环顶检测 cancelled 后收尾断开），
        // 错误经既有 TetheringStatus 通道上报（poller 侧 record_error 收口）。
        if db.photos_library_get(&session.library.id).ok().flatten().is_none() {
            request_stop(&session.id);
            return Err("目标照片库已被删除，拍摄会话已结束".into());
        }
        let (photo, cleanup_error) = ingest_from_backend(
            &db,
            &session.library,
            session.album_id,
            session.backend.as_ref(),
            &session.camera.pnp_id,
            object,
            Some(&state.register_gate),
        )?;
        session
            .seen
            .lock()
            .unwrap()
            .insert(object.object_id.clone());
        // Cleanup failure does not invalidate a committed asset or trigger a
        // duplicate ingestion. The download is retained for later recovery.
        if let Some(error) = cleanup_error {
            session.record_error(state, format!("照片已入库，暂存副本清理失败: {error}"));
        }
        {
            let mut photos = session.photos.lock().unwrap();
            photos.push(photo.clone());
            if photos.len() > 64 {
                photos.remove(0);
            }
        }
        state.bus.publish(AppEvent::TetheringPhotoAdded {
            session_id: session.id.clone(),
            library_id: session.library.id.clone(),
            album_id: session.album_id,
            asset_id: photo.id,
            name: photo.name,
            kind: photo.kind,
        });
        let app_db_dir = crate::ipc::app_database_dir(state)?;
        // pHash 任务补差（2026-10-09 审计 P0-C）+ eyes/blur 选片建档
        //（2026-10-12：插入不再登记这两类；AI 开启才批量幂等补建）。
        let _ = db.create_phash_tasks_for_unindexed();
        crate::index::kick(app_db_dir.clone(), &state.supervisor);
        // 激活数据库（多数据库修正）：AI 回填直接触发。
        {
            let settings = state.settings.lock().unwrap().clone();
            if settings.ai.enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    app_db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            if settings.ai.enable_face {
                crate::ai::face::kick_face_if_ready(
                    app_db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            // 选片通道（eyes/blur）：AI 开启才建档并触发闭眼回填
            if settings.ai.enable_selection {
                let _ = db.create_eyes_tasks_for_unindexed();
                let _ = db.create_blur_tasks_for_unindexed();
                crate::ai::selection::kick_eyes_if_ready(
                    app_db_dir,
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
        }
        Ok(())
    })();
    // §B1 终态落盘：入库成功（含 acknowledge_captured 已过）记 Acked；
    // 失败记 Failed——账本保留该 id（重试由现有内存退避管，崩溃后由
    // 恢复路径重收）。终态写失败仍仅日志。
    if let Some(ledger) = session.ledger.as_ref() {
        let ts = chrono::Utc::now().to_rfc3339();
        match &result {
            Ok(()) => ledger.record_acked(&object.object_id, &ts),
            Err(error) => ledger.record_failed(&object.object_id, error, &ts),
        }
    }
    session.receiving.store(false, Ordering::Release);
    result
}

fn ingest_from_backend(
    db: &Db,
    library: &TetherLibrary,
    album_id: i64,
    backend: &dyn CameraBackend,
    camera_id: &str,
    object: &CapturedObject,
    gate: Option<&std::sync::Arc<std::sync::Mutex<()>>>,
) -> Result<(TetherPhoto, Option<String>), String> {
    let mut source = backend
        .open_captured(camera_id, object)
        .map_err(|e| e.to_string())?;
    let photo = ingest(
        db,
        library,
        album_id,
        &object.object_name,
        object.object_size,
        &mut source,
        gate,
    )?;
    drop(source); // Windows cannot remove the local receipt while still open.
    let cleanup_error = backend
        .acknowledge_captured(camera_id, object)
        .err()
        .map(|e| e.to_string());
    Ok((photo, cleanup_error))
}

/// Shared by the real camera download path and regression tests; no job/journal rows.
/// 2026-10-09 §三：落盘布局 = 纯时间 `{库root}/{拍摄年}/{拍摄月}/{原文件名}`
///（EXIF 拍摄时间，缺失回退当前时刻——联拍文件刚落盘，mtime≈now）；
/// 相册只是逻辑引用（§一），不影响落位。
/// 2026-10-09 审计 P2：登记段（路径冲突处理 → 原子 rename → 相册校验 →
/// 入库）持 `register_gate`（§八-5 单管道）——与库扫描/导入的登记段
/// 互斥串行；未注入（引擎级测试形态）跳过。查重不设：新拍照片语义上
/// 就是新资产。
pub fn ingest(
    db: &Db,
    library: &TetherLibrary,
    album_id: i64,
    name: &str,
    expected_size: u64,
    source: &mut dyn Read,
    gate: Option<&std::sync::Arc<std::sync::Mutex<()>>>,
) -> Result<TetherPhoto, String> {
    let filename = name
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or("照片文件名无效")?;
    let kind = crate::devices::classify(filename, &[]);
    if !matches!(
        kind,
        crate::events::AssetKind::Photo | crate::events::AssetKind::Raw
    ) {
        return Err("仅接收照片文件".into());
    }
    // 先流式落 .part 到库根暂存（拍摄时间在读流后才能从 EXIF 解出，
    // 最终目录随后渲染），再原子改名进纯时间目录。
    std::fs::create_dir_all(&library.root).map_err(|e| e.to_string())?;
    let part = library
        .root
        .join(format!(".tether-{}.part", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part)
            .map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        let mut size = 0;
        let mut hash = xxhash_rust::xxh64::Xxh64::new(0);
        loop {
            let n = source.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            hash.update(&buf[..n]);
            size += n as u64;
            let take = (1024 * 1024 - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        if size == 0 || (expected_size > 0 && size != expected_size) {
            return Err("拍摄照片传输不完整，源文件已保留".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let meta = crate::metadata::exif_lite::parse(&head);
        let captured =
            crate::db::organization::resolve_captured(meta.captured_at, chrono::Utc::now());
        let relative=crate::db::organization::organized_rel(&library.organization_template,captured,filename,meta.camera.as_deref())?;
        let intended=library.root.join(relative);let dir=intended.parent().ok_or("组织模板路径无效")?.to_path_buf();
        let organized_filename=intended.file_name().ok_or("组织模板文件名无效")?.to_string_lossy();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // 登记单管道闸（§八-5，审计 P2）：冲突处理 → rename → 入库整段
        // 持闸——return 路径随栈释放守卫。
        let _registration_guard = gate.map(|gate| {
            gate.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
        let destination = crate::ipc::claim::resolve_conflict(&dir, &organized_filename);
        std::fs::rename(&part, &destination).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let filename = destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let row = AssetRow {
            path: destination.to_string_lossy().into_owned(),
            filename: filename.clone(),
            size,
            mtime: std::fs::metadata(&destination)
                .and_then(|meta| meta.modified())
                .map_err(|e| e.to_string())
                .map(|time| chrono::DateTime::<chrono::Utc>::from(time)
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true))?,
            xxhash: hash.digest(),
            kind,
            captured_at: Some(
                meta.captured_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| now.clone()),
            ),
            camera: meta.camera,
            source: "imported".into(),
            created_at: now,
            origin: "imported".into(),
            width: meta.width,
            height: meta.height,
            iso: meta.iso,
            f_number: meta.f_number,
            exposure_time: meta.exposure_time,
            focal_length: meta.focal_length,
            lens: meta.lens,
            pair_asset_id: None,
            thumb_state: 0,
            orientation: meta.deep.orientation,
            flash: meta.deep.flash,
            metering_mode: meta.deep.metering_mode,
            white_balance: meta.deep.white_balance,
            exposure_program: meta.deep.exposure_program,
            software: meta.deep.software,
            artist: meta.deep.artist,
            gps_lat: meta.deep.gps_lat,
            gps_lon: meta.deep.gps_lon,
            rating: 0,
            flagged: 0,
            color_label: None,
            rejected: 0,
            library_id: Some(library.id.clone()),
            missing: 0,
            xmp_dirty: 0,
            volume_serial: None,
            file_id: None,
        };
        // Check the album inside the transaction, so concurrent deletion cannot
        // silently register a photograph outside its selected destination.
        if !db.album_exists(album_id).map_err(|e| e.to_string())? {
            return Err(format!(
                "拍摄相册已删除；照片已保留在 {}",
                destination.display()
            ));
        }
        db.insert_captured_asset(&row, album_id)
            .map_err(|e| e.to_string())?;
        let id = db
            .asset_id_by_path(&row.path)
            .map_err(|e| e.to_string())?
            .ok_or("照片入册失败")?;
        Ok(TetherPhoto {
            id,
            name: filename,
            kind: kind.as_db_str().into(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NoopBackend {
        disconnects: std::sync::atomic::AtomicUsize,
        settings_reads: AtomicUsize,
        parameter: Mutex<Option<(String, String)>>,
        disconnect_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        disconnect_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }
    impl CameraBackend for NoopBackend {
        fn id(&self) -> &'static str {
            "noop"
        }
        fn name(&self) -> &'static str {
            "noop"
        }
        fn capabilities(&self) -> super::super::backend::Capabilities {
            super::super::backend::Capabilities::FILE_TRANSFER
        }
        fn enumerate(&self) -> Result<Vec<super::super::backend::CameraInfo>, TetherError> {
            Ok(Vec::new())
        }
        fn connect(&self, _pnp_id: &str) -> Result<super::super::backend::CameraInfo, TetherError> {
            Err(TetherError::Other("noop".into()))
        }
        fn disconnect(&self, _pnp_id: &str) {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self.disconnect_started.lock().unwrap().take() {
                started.send(()).unwrap();
            }
            if let Some(release) = self.disconnect_release.lock().unwrap().take() {
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        fn set_setting(&self, _pnp_id: &str, id: &str, value: &str) -> Result<(), TetherError> {
            *self.parameter.lock().unwrap() = Some((id.into(), value.into()));
            self.settings_reads.store(0, Ordering::Release);
            Ok(())
        }
        fn settings(&self, _pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
            let reads = self.settings_reads.fetch_add(1, Ordering::AcqRel);
            let parameter = self.parameter.lock().unwrap();
            Ok(parameter
                .as_ref()
                .map(|(id, value)| CameraSetting {
                    id: id.clone(),
                    label: id.clone(),
                    kind: super::super::backend::SettingKind::Choice,
                    current: if reads == 0 {
                        "100".into()
                    } else {
                        value.clone()
                    },
                    writable: true,
                    options: Vec::new(),
                    min: None,
                    max: None,
                    step: None,
                })
                .into_iter()
                .collect())
        }
        fn capture_still(
            &self,
            _pnp_id: &str,
            _timeout: Duration,
        ) -> Result<CapturedObject, TetherError> {
            Err(TetherError::CaptureNotSupported)
        }
    }

    fn make_session(id: &str, camera_pnp: &str, album_id: i64) -> Arc<Session> {
        make_session_with(id, camera_pnp, album_id, None, None)
    }

    /// 测试会话工厂：可注入物理键与磁盘账本（§B1/§E 测试形态）。
    pub(super) fn make_session_with(
        id: &str,
        camera_pnp: &str,
        album_id: i64,
        physical_key: Option<&str>,
        ledger: Option<Arc<ReceiptsStore>>,
    ) -> Arc<Session> {
        Arc::new(Session {
            id: id.to_string(),
            library: TetherLibrary {
                id: "lib".into(),
                root: PathBuf::from("unused"),
            organization_template: crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.to_string(),
},
            album_id,
            album_name: format!("相册{album_id}"),
            camera: crate::ipc::tethering::CameraDto {
                pnp_id: camera_pnp.to_string(),
                name: "假相机".into(),
                capabilities: crate::ipc::tethering::CameraCapabilitiesDto {
                    file_transfer: true,
                    standard_capture: false,
                    vendor_capture_nikon: false,
                    object_added_events: false,
                    live_view: false,
                },
            },
            // 测试缺省物理键：回退 pnp 端点形态（与生产无 serial 时同构）。
            physical_key: physical_key
                .map(str::to_string)
                .unwrap_or_else(|| format!("usb:::假相机:{camera_pnp}")),
            backend: Arc::new(NoopBackend::default()),
            ledger,
            operation: Mutex::new(()),
            pending_commands: AtomicUsize::new(0),
            capture_in_flight: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
            connected: AtomicBool::new(true),
            receiving: AtomicBool::new(false),
            settings: Mutex::new(Vec::new()),
            convergence: Mutex::new(SettingsConvergence::default()),
            last_settings_read: Mutex::new(None),
        last_frame_at: Mutex::new(None),
        settings_read_bypass: AtomicBool::new(false),
            receipt_seq: AtomicU64::new(0),
            photos: Mutex::new(Vec::new()),
            seen: Mutex::new(HashSet::new()),
            error: Mutex::new(None),
        })
    }

    #[test]
    fn store_supports_multiple_sessions_same_or_different_albums() {
        let mut store = SessionStore::default();
        let a = make_session("s1", "gphoto:usb:001,010", 1);
        let b = make_session("s2", "gphoto:usb:001,011", 1); // 同相册不同相机
        let c = make_session("s3", "wpd:X", 2);
        store.insert(a.clone());
        store.insert(b.clone());
        store.insert(c.clone());
        assert!(
            store.get("s1").is_some() && store.get("s2").is_some() && store.get("s3").is_some()
        );
        assert_eq!(store.sessions.len(), 3);
        // 停止一个不影响其他会话
        let removed = store.remove("s2");
        assert!(removed.is_some());
        assert!(!removed.unwrap().cancelled.load(Ordering::Acquire)); // 置位由 stop() 全局入口做
        assert!(store.get("s1").is_some());
        assert!(store.get("s3").is_some());
        assert!(store.get("s2").is_none());
    }

    #[test]
    fn poll_pace_drains_burst_without_fixed_interval() {
        // 空轮限频、有事件立即回轮（高速连拍排空）
        assert_eq!(poll_pace_ms(0), 200);
        assert_eq!(poll_pace_ms(1), 0);
        assert_eq!(poll_pace_ms(10), 0);
    }

    #[test]
    fn store_rejects_second_session_on_same_camera() {
        let mut store = SessionStore::default();
        store.insert(make_session("s1", "gphoto:usb:001,010", 1));
        // 同相机不同相册也不行（USB 控制权互斥）
        assert!(store.camera_in_use("gphoto:usb:001,010"));
        assert!(!store.camera_in_use("gphoto:usb:001,011"));
        // 停止后可重开
        store.remove("s1");
        assert!(!store.camera_in_use("gphoto:usb:001,010"));
    }

    /// stop_all（切库/删库联动）：先置取消再断开，全部会话结束、存储清空。
    #[test]
    fn stop_all_cancels_then_disconnects_every_session() {
        let mut store = SessionStore::default();
        let a = make_session("s1", "gphoto:usb:001,010", 1);
        let b = make_session("s2", "gphoto:usb:001,011", 2);
        store.insert(a.clone());
        store.insert(b.clone());
        let sessions = store.all();
        for session in &sessions {
            session.cancelled.store(true, Ordering::Release);
        }
        for session in &sessions {
            session.disconnect();
        }
        store.clear();
        assert!(store.all().is_empty());
        assert!(!a.connected.load(Ordering::Acquire));
        assert!(!b.connected.load(Ordering::Acquire));
        assert!(a.cancelled.load(Ordering::Acquire));
        assert!(b.cancelled.load(Ordering::Acquire));
    }

    // -----------------------------------------------------------------------
    // §E-3 物理独占键：同一物理相机跨端点互斥，不同物理机同名不合并
    // -----------------------------------------------------------------------

    #[test]
    fn physical_key_mutexes_cross_endpoint_sessions_but_not_same_models() {
        let mut store = SessionStore::default();
        // 同一台 Z5II：wpd 端点在会话中，gphoto 端点试图再开。
        store.insert(make_session_with(
            "s1",
            r"\\?\usb#vid_04b0&pid_0456#sn1#{g}",
            1,
            Some("serial:SN-1"),
            None,
        ));
        assert!(store.physical_camera_in_use("serial:SN-1"));
        assert!(
            !store.physical_camera_in_use("serial:SN-2"),
            "同型不同机不合并"
        );
        // pnp 预检不误伤：跨端点 pnp 不同 → 快速预检放行，物理键在 connect
        // 之后原子判定（start 语义）。
        assert!(!store.camera_in_use("gphoto:usb:001,010"));
        store.remove("s1");
        assert!(!store.physical_camera_in_use("serial:SN-1"), "停止后可重开");
    }

    // -----------------------------------------------------------------------
    // §B7 参数失效收敛
    // -----------------------------------------------------------------------

    fn setting(id: &str, current: &str) -> CameraSetting {
        CameraSetting {
            id: id.into(),
            label: id.into(),
            kind: super::super::backend::SettingKind::Choice,
            current: current.into(),
            writable: true,
            options: Vec::new(),
            min: None,
            max: None,
            step: None,
        }
    }

    #[test]
    fn invalidation_merges_only_invalidated_ids_into_cache() {
        let cached = vec![setting("iso", "100"), setting("f-number", "4")];
        let fresh = vec![setting("iso", "800"), setting("f-number", "11")];
        // 部分失效：只采纳 iso 的新值，f-number 保持缓存（本轮未重读）。
        let merged = merge_settings(
            cached.clone(),
            fresh,
            &Invalidation::Ids(vec!["iso".into()]),
        );
        assert_eq!(merged[0].current, "800");
        assert_eq!(merged[1].current, "4", "非失效项不采纳未重读的新值");
        // 整表失效：全部替换。
        let merged = merge_settings(cached, vec![setting("iso", "1600")], &Invalidation::All);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].current, "1600", "相机不再报告的失效项移除");
        // 失效集合里新出现的参数追加。
        let merged = merge_settings(
            vec![setting("iso", "100")],
            vec![setting("iso", "200"), setting("whitebalance", "auto")],
            &Invalidation::Ids(vec!["whitebalance".into()]),
        );
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].current, "100", "iso 未失效");
        assert_eq!(merged[1].id, "whitebalance");
    }

    #[test]
    fn invalidation_events_bump_revision_and_merge_consecutive_signals() {
        let mut convergence = SettingsConvergence::default();
        assert_eq!(
            convergence.take_invalid(),
            Some(Invalidation::All),
            "无快照前要求整表读"
        );
        convergence.mark_snapshot();
        assert_eq!(convergence.take_invalid(), None, "干净状态不读相机");
        convergence.invalidate(Some(vec!["iso".into()]));
        assert_eq!(convergence.revision, 1);
        assert_eq!(
            convergence.take_invalid(),
            Some(Invalidation::Ids(vec!["iso".into()]))
        );
        // 连续部分失效合并；整表失效不缩回部分。
        convergence.invalidate(Some(vec!["iso".into()]));
        convergence.invalidate(Some(vec!["f-number".into()]));
        assert_eq!(
            convergence.take_invalid(),
            Some(Invalidation::Ids(vec!["iso".into(), "f-number".into()]))
        );
        convergence.invalidate(None);
        convergence.invalidate(Some(vec!["iso".into()]));
        assert_eq!(convergence.take_invalid(), Some(Invalidation::All));
        convergence.mark_snapshot();
        assert_eq!(convergence.take_invalid(), None);
    }

    #[test]
    fn settings_refresh_hits_camera_only_through_invalidation() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("conv", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        // 首次拉取（无快照）→ 整表读。
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 干净状态重复拉取 → 缓存，不读相机。
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 事件失效（节流窗口已过）→ 再读一次。
        session.invalidate_settings(None);
        *session.last_settings_read.lock().unwrap() = None;
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
        assert_eq!(session.dto().settings.len(), 0);
    }

    #[test]
    fn settings_read_storm_is_throttled_and_invalidation_survives() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("storm", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 取景中测光类属性变更风暴：失效 → 立即拉取 → 1s 窗口内返回缓存
        // 不读相机，失效标记保留（不丢失，仅推迟）。
        session.invalidate_settings(None);
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 窗口过后同一份失效 → 真实读取。
        *session.last_settings_read.lock().unwrap() = None;
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
    }

    #[test]
    fn live_view_activity_widens_settings_read_interval() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("lv", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 取景活跃（2s 内有帧）：常规 1s 节流已过，但取景间隔（10s）未到
        // → 仍读缓存；失效保留。
        session.note_frame();
        *session.last_settings_read.lock().unwrap() =
            Some(Instant::now() - Duration::from_secs(2));
        session.invalidate_settings(None);
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 取景静默（最后帧 >2s）：回到常规 1s 节流 → 读取放行。
        *session.last_frame_at.lock().unwrap() =
            Some(Instant::now() - Duration::from_secs(5));
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
    }

    #[test]
    fn settings_read_intervals_gate_by_injected_window() {
        let now = Instant::now();
        assert!(settings_read_due(None, now, SETTINGS_READ_MIN_INTERVAL), "从未读过 → 立即可读");
        assert!(!settings_read_due(Some(now), now, SETTINGS_READ_MIN_INTERVAL), "刚读过 → 节流");
        assert!(
            !settings_read_due(
                Some(now - Duration::from_millis(999)),
                now,
                SETTINGS_READ_MIN_INTERVAL
            ),
            "不足 1s → 仍节流"
        );
        assert!(
            settings_read_due(Some(now - Duration::from_secs(1)), now, SETTINGS_READ_MIN_INTERVAL),
            "满 1s → 放行"
        );
        // 取景活跃期间按 10s 间隔：9s 仍节流、10s 放行。
        assert!(
            !settings_read_due(
                Some(now - Duration::from_secs(9)),
                now,
                LIVE_VIEW_SETTINGS_READ_INTERVAL
            )
        );
        assert!(
            settings_read_due(
                Some(now - Duration::from_secs(10)),
                now,
                LIVE_VIEW_SETTINGS_READ_INTERVAL
            )
        );
    }

    #[test]
    fn fallback_refresh_throttles_to_twenty_seconds() {
        let now = Instant::now();
        assert!(fallback_refresh_due(None, now), "从未刷新过 → 立即可刷");
        assert!(!fallback_refresh_due(Some(now), now), "刚刷过 → 节流");
        assert!(
            !fallback_refresh_due(Some(now - Duration::from_secs(19)), now),
            "19s → 仍节流"
        );
        assert!(
            fallback_refresh_due(Some(now - Duration::from_secs(20)), now),
            "满 20s → 放行"
        );
        assert!(fallback_refresh_due(
            Some(now - Duration::from_secs(120)),
            now
        ));
    }

    #[test]
    fn parameter_write_is_confirmed_by_readback_before_cache_update_completes() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("confirm", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        let settings = session.set_setting("iso", "400").unwrap();
        assert_eq!(settings[0].current, "400");
        assert_eq!(session.dto().settings[0].current, "400");
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
        assert_eq!(session.pending_commands.load(Ordering::Acquire), 0);
    }

    #[test]
    fn mode_confirmation_invalidates_dependent_parameter_descriptors() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("mode", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        session.set_setting("expprogram", "M").unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
        // 模式确认即置强制旁路：紧接着的刷新无视节流立即重读描述符
        //（模式切换改变可写性，面板更新不能等节流窗口）。
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 3);
    }

    #[test]
    fn forced_read_is_one_shot_and_bypasses_live_view_interval() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("force", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 1);
        // 取景活跃 + 刚读过（10s 间隔远未到），但 force 置位 → 立即真读。
        session.note_frame();
        session.invalidate_settings(None);
        session.force_next_settings_read();
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
        // 旁路一次性：紧接着的失效回到节流（返回缓存，标记保留）。
        session.invalidate_settings(None);
        session.refresh_settings().unwrap();
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
    }

    #[test]
    fn background_refresh_uses_cache_without_queuing_behind_user_command() {
        let session = make_session("priority", "test-camera", 1);
        let operation = session.operation.lock().unwrap();
        let command_session = session.clone();
        let command = std::thread::spawn(move || command_session.command("test", || Ok(())));
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.pending_commands.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(session.pending_commands.load(Ordering::Acquire) > 0);
        assert!(session.try_background().is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let background_session = session.clone();
        let background =
            std::thread::spawn(move || tx.send(background_session.refresh_settings()).unwrap());
        let cached = rx.recv_timeout(Duration::from_secs(1));
        drop(operation);
        command.join().unwrap().unwrap();
        background.join().unwrap();
        assert!(cached.unwrap().unwrap().is_empty());
        assert_eq!(session.pending_commands.load(Ordering::Acquire), 0);
    }

    #[test]
    fn duplicate_capture_is_rejected_and_failure_releases_busy_state() {
        let session = make_session("capture", "test-camera", 1);
        let capture = session.claim_capture().unwrap();
        assert!(session.claim_capture().is_err());
        assert!(session.try_background().is_none());
        drop(capture);
        let failed: Result<(), String> = (|| {
            let _capture = session.claim_capture()?;
            session.command("capture", || Err("camera error".into()))
        })();
        assert!(failed.is_err());
        assert!(session.claim_capture().is_ok());
    }

    #[test]
    fn stop_keeps_camera_reserved_until_disconnect_and_only_disconnects_once() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(NoopBackend {
            disconnect_started: Mutex::new(Some(started_tx)),
            disconnect_release: Mutex::new(Some(release_rx)),
            ..Default::default()
        });
        let id = uuid::Uuid::new_v4().to_string();
        let camera = format!("test-camera-{id}");
        let mut session = make_session(&id, &camera, 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        current().lock().unwrap().insert(session.clone());
        request_stop(&id);
        assert!(session.is_stopping());
        let stop_id = id.clone();
        let first = std::thread::spawn(move || stop(&stop_id));
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(current().lock().unwrap().camera_in_use(&camera));
        let stop_id = id.clone();
        let second = std::thread::spawn(move || stop(&stop_id));
        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert!(!current().lock().unwrap().camera_in_use(&camera));
        assert!(get(&id).is_err());
        assert_eq!(backend.disconnects.load(Ordering::SeqCst), 1);
        // 已释放的旧 worker 再清理，也不能断开该相机的新会话。
        let next_id = uuid::Uuid::new_v4().to_string();
        current()
            .lock()
            .unwrap()
            .insert(make_session(&next_id, &camera, 1));
        session.disconnect();
        stop(&id);
        assert!(get(&next_id).is_ok());
        assert_eq!(backend.disconnects.load(Ordering::SeqCst), 1);
        stop(&next_id);
    }

    // -----------------------------------------------------------------------
    // 2026-10-09 审计：相册众数选库（P1）/ 删库软停（P0）/ 登记闸（P2）
    // -----------------------------------------------------------------------

    /// 测试 AppState（激活数据库指向 db_dir；联拍收片路径的最小真值）。
    pub(super) fn test_state(db_dir: &std::path::Path) -> crate::ipc::SharedState {
        let supervisor = crate::tasks::TaskSupervisor::new(crate::events::EventBus::new());
        let settings = crate::settings::Settings {
            databases: vec![crate::settings::DatabaseEntry {
                id: "test-db".into(),
                name: "测试数据库".into(),
                db_dir: db_dir.to_string_lossy().into_owned(),
            }],
            active_database_id: Some("test-db".into()),
            ..Default::default()
        };
        std::sync::Arc::new(crate::ipc::AppState {
            settings: Mutex::new(settings),
            config_dir: db_dir.to_path_buf(),
            bus: crate::events::EventBus::new(),
            devices: Mutex::new(std::collections::HashMap::new()),
            active_import: Mutex::new(None),
            import_running: Arc::new(AtomicBool::new(false)),
            register_gate: Arc::new(Mutex::new(())),
            library_scan_kick: Arc::new(AtomicBool::new(false)),
            supervisor: supervisor.clone(),
            thumb_queue: crate::ipc::thumb::ThumbQueue::new(),
            ai: crate::ai::ModelManager::new(
                db_dir.join("models"),
                crate::events::EventBus::new(),
                supervisor,
            ),
        })
    }

    #[test]
    fn tether_library_follows_album_majority_with_first_library_fallback() {
        let holder = tempfile::tempdir().unwrap();
        let roots = tempfile::tempdir().unwrap(); // 库根与数据库目录分离（§八-6 互斥）
        for name in ["a", "b"] {
            std::fs::create_dir_all(roots.path().join(name)).unwrap();
        }
        let db = Db::open(&holder.path().join("library.db")).unwrap();
        let lib_a = db
            .photos_library_register(
                "库A",
                &roots.path().join("a").to_string_lossy(),
                holder.path(),
            )
            .unwrap();
        let lib_b = db
            .photos_library_register(
                "库B",
                &roots.path().join("b").to_string_lossy(),
                holder.path(),
            )
            .unwrap();
        let album = db.album_create("相册").unwrap();
        // 相册资产：3 行属库 B + 1 行属库 A（众数 = B；首库 = A，两者不同
        // 才能区分两种策略）
        for (i, library) in [(0i64, &lib_b), (1, &lib_b), (2, &lib_b), (3, &lib_a)] {
            let path = holder.path().join(format!("p{i}.jpg"));
            db.0.execute(
                "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
                 created_at, origin, library_id, missing) \
                 VALUES (?1, ?2, 1, '2026-01-01T00:00:00Z', ?3, 'photo', 'imported', \
                 '2026-01-01T00:00:00Z', 'imported', ?4, 0)",
                rusqlite::params![
                    path.to_string_lossy().to_string(),
                    format!("p{i}.jpg"),
                    i,
                    library.id
                ],
            )
            .unwrap();
            let id = db
                .asset_id_by_path(&path.to_string_lossy())
                .unwrap()
                .unwrap();
            db.0.execute(
                "INSERT INTO album_item (album_id, asset_id, added_at) \
                 VALUES (?1, ?2, '2026-01-01T00:00:00Z')",
                rusqlite::params![album.id, id],
            )
            .unwrap();
        }
        assert_eq!(
            resolve_tether_library(&db, album.id).unwrap().id,
            lib_b.id,
            "相册资产众数在库 B → 新拍照片落库 B"
        );
        let empty = db.album_create("空相册").unwrap();
        // 首库 = 登记表序首行（同毫秒登记的并列序由 id 决胜——断言与列表
        // 头一致即生产语义，不钉死具体哪个库）
        let list_head = db.photos_library_list().unwrap()[0].clone();
        assert_eq!(
            resolve_tether_library(&db, empty.id).unwrap().id,
            list_head.id,
            "空相册回退首库（登记序，无相册上下文）"
        );
    }

    #[test]
    fn receive_ends_session_when_pinned_library_removed() {
        let holder = tempfile::tempdir().unwrap();
        let db_dir = holder.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        let state = test_state(&db_dir);
        let _db = crate::ipc::open_library_db(&db_dir).unwrap();
        // 会话 pin 的库 id 不在登记表（移除登记后的形态）
        let id = uuid::Uuid::new_v4().to_string();
        let session = make_session(&id, "test-camera-gone", 1);
        current().lock().unwrap().insert(session.clone());
        let object = CapturedObject {
            object_id: "o1".into(),
            object_name: "DSC_0001.jpg".into(),
            object_size: 4,
        };
        let err = receive(&state, &session, &object).unwrap_err();
        assert!(err.contains("目标照片库已被删除"), "{err}");
        assert!(session.is_stopping(), "会话应结束（软信号，poller 收尾断开）");
        stop(&id);
    }

    #[test]
    fn ingest_registration_waits_for_register_gate() {
        let holder = tempfile::tempdir().unwrap();
        let db = Db::open(&holder.path().join("library.db")).unwrap();
        let album = db.album_create("闸").unwrap();
        let library = TetherLibrary {
            id: "lib-gate".into(),
            root: holder.path().join("photos"),
        organization_template: crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.to_string(),
};
        let gate = Arc::new(Mutex::new(()));
        // 外持闸（模拟库扫描登记段在跑）：ingest 只能完成流式落盘，登记段
        // 阻塞等待，不写库
        let guard = gate.lock().unwrap();
        let bytes: Vec<u8> = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];
        let ingest_thread = {
            let gate = Arc::clone(&gate);
            let db = Db::open(&holder.path().join("library.db")).unwrap();
            let library = library.clone();
            let bytes = bytes.clone();
            std::thread::spawn(move || {
                let mut cursor = std::io::Cursor::new(bytes.clone());
                ingest(
                    &db,
                    &library,
                    album.id,
                    "gate.jpg",
                    bytes.len() as u64,
                    &mut cursor,
                    Some(&gate),
                )
            })
        };
        std::thread::sleep(Duration::from_millis(300));
        let blocked: i64 = db
            .0
            .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(blocked, 0, "登记闸被持有时不得入库");
        drop(guard);
        let photo = ingest_thread.join().unwrap().unwrap();
        let settled: i64 = db
            .0
            .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(settled, 1, "放行后登记完成");
        assert!(photo.id > 0);
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::super::backend::{CameraInfo, Capabilities, TetherError};
    use super::super::staging;
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    struct ReceiptBackend {
        root: PathBuf,
        acknowledged: AtomicUsize,
        /// request_objects 收到的 id（恢复路径断言用）。
        requested: Mutex<Vec<String>>,
    }

    impl CameraBackend for ReceiptBackend {
        fn id(&self) -> &'static str {
            "receipt-test"
        }

        fn name(&self) -> &'static str {
            "receipt-test"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::NONE
        }

        fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
            Ok(vec![])
        }

        fn connect(&self, _: &str) -> Result<CameraInfo, TetherError> {
            Err(TetherError::Disconnected)
        }

        fn disconnect(&self, _: &str) {
            staging::disconnect(&self.root);
        }

        fn capture_still(&self, _: &str, _: Duration) -> Result<CapturedObject, TetherError> {
            Err(TetherError::CaptureNotSupported)
        }

        /// 按需收取：对象仍在（暂存文件在）才算可恢复——与 wpd 校验
        /// 「对象还存在」的语义一致；消失的 id 不返回（退回事件驱动）。
        fn request_objects(&self, _: &str, ids: &[String]) -> Result<Vec<String>, TetherError> {
            self.requested.lock().unwrap().extend(ids.iter().cloned());
            Ok(ids
                .iter()
                .filter(|id| Path::new(id).is_file())
                .cloned()
                .collect())
        }

        fn open_captured(
            &self,
            _: &str,
            object: &CapturedObject,
        ) -> Result<Box<dyn Read + Send>, TetherError> {
            let file = std::fs::File::open(staging::validated_path(&self.root, object)?)
                .map_err(|error| TetherError::Other(error.to_string()))?;
            Ok(Box::new(file))
        }

        fn acknowledge_captured(
            &self,
            _: &str,
            object: &CapturedObject,
        ) -> Result<(), TetherError> {
            self.acknowledged.fetch_add(1, Ordering::SeqCst);
            staging::acknowledge(&self.root, object)
        }
    }

    #[test]
    fn failed_ingestion_keeps_receipt_and_success_acknowledges_only_after_commit() {
        let root = tempfile::tempdir().unwrap();
        let incoming = root.path().join("incoming");
        std::fs::create_dir(&incoming).unwrap();
        let db = Db::open(&root.path().join("library.db")).unwrap();
        let album = db.album_create("Capture").unwrap();
        let library = TetherLibrary {
            id: "lib-test".into(),
            root: root.path().join("photos"),
        organization_template: crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.to_string(),
};
        let backend = ReceiptBackend {
            root: incoming.clone(),
            acknowledged: AtomicUsize::new(0),
            requested: Mutex::new(Vec::new()),
        };
        let receipt = staging::persist(&incoming, "original.jpg", b"retained bytes").unwrap();
        let mut truncated = receipt.clone();
        truncated.object_size += 1;
        assert!(
            ingest_from_backend(&db, &library, album.id, &backend, "camera", &truncated, None)
                .is_err()
        );
        backend.disconnect("camera");
        assert_eq!(
            std::fs::read(&receipt.object_id).unwrap(),
            b"retained bytes"
        );
        assert_eq!(backend.acknowledged.load(Ordering::SeqCst), 0);

        let (photo, error) =
            ingest_from_backend(&db, &library, album.id, &backend, "camera", &receipt, None)
                .unwrap();
        assert!(error.is_none());
        assert_eq!(backend.acknowledged.load(Ordering::SeqCst), 1);
        assert!(!Path::new(&receipt.object_id).exists());
        let path: String =
            db.0.query_row("SELECT path FROM assets WHERE id = ?1", [photo.id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"retained bytes");
    }

    // -----------------------------------------------------------------------
    // §B1 会话恢复：request_objects 优先收取 + Ack 落盘
    // -----------------------------------------------------------------------

    /// 崩溃形态：磁盘账本有未确认收据（一台相机两条），重启后新会话
    /// 建立 → 恢复路径必须调用 request_objects；可恢复对象走正常
    /// receive/ingest 后账本落 Acked，消失对象保持未确认。
    #[test]
    fn session_recovery_requests_unacked_and_acks_ledger_after_ingest() {
        let root = tempfile::tempdir().unwrap();
        let db_dir = root.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        let state = super::tests::test_state(&db_dir);
        let db = crate::ipc::open_library_db(&db_dir).unwrap();
        let album = db.album_create("恢复").unwrap();
        let photos_root = root.path().join("photos");
        std::fs::create_dir(&photos_root).unwrap();
        let library = db
            .photos_library_register("恢复库", &photos_root.to_string_lossy(), &db_dir)
            .unwrap();
        let incoming = root.path().join("incoming");
        std::fs::create_dir(&incoming).unwrap();
        // 「崩溃前」的暂存对象 + 磁盘账本两条未确认收据。
        let retained = staging::persist(&incoming, "DSC_9999.jpg", b"recovered").unwrap();
        let receipts_dir = root.path().join("receipts");
        let ledger = Arc::new(ReceiptsStore::open(&receipts_dir, "serial:SN-1"));
        ledger.record_pending("receipt-test", &retained.object_id, 1, "t1");
        ledger.record_pending("receipt-test", "object-gone", 2, "t2");
        let backend = Arc::new(ReceiptBackend {
            root: incoming.clone(),
            acknowledged: AtomicUsize::new(0),
            requested: Mutex::new(Vec::new()),
        });
        let mut session = super::tests::make_session_with(
            "recover",
            "test-camera-recover",
            album.id,
            Some("serial:SN-1"),
            Some(ledger.clone()),
        );
        {
            let session = Arc::get_mut(&mut session).unwrap();
            session.backend = backend.clone();
            // receive() 校验会话 pin 的照片库在册（删库软停）——对齐真实库。
            session.library = TetherLibrary {
                id: library.id,
                root: photos_root,
                organization_template:
                    crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.to_string(),
            };
        }
        // 会话启动恢复（生产在 start() 建立后、poller 拉起前调用）。
        recover_unacked(&session);
        // request_objects 必须收到两条未确认 id（含已消失的）。
        let requested = backend.requested.lock().unwrap().clone();
        assert_eq!(
            requested,
            vec![retained.object_id.clone(), "object-gone".to_string()],
            "恢复按账本未确认 id 调用 request_objects"
        );
        // 恢复对象走正常收片路径（poller 交付形态）→ 入库 + Ack 落盘。
        receive(&state, &session, &retained).unwrap();
        assert_eq!(backend.acknowledged.load(Ordering::SeqCst), 1);
        let unacked = ledger.unacked();
        assert_eq!(unacked.len(), 1, "只剩消失对象的未确认条目");
        assert_eq!(unacked[0].id, "object-gone");
        assert!(!Path::new(&retained.object_id).exists(), "暂存已确认清理");
    }

    /// 后端不支持按需收取（trait 默认返回空）→ 恢复退回事件驱动等待，
    /// 账本条目保留（不删除、不阻断会话）。
    #[test]
    fn recovery_without_backend_support_falls_back_to_event_driven() {
        let root = tempfile::tempdir().unwrap();
        let receipts_dir = root.path().join("receipts");
        let ledger = Arc::new(ReceiptsStore::open(&receipts_dir, "serial:SN-2"));
        ledger.record_pending("noop", "o1", 1, "t1");
        // NoopBackend 未覆写 request_objects → 默认空。
        let session = super::tests::make_session_with(
            "recover-default",
            "test-camera-default",
            1,
            Some("serial:SN-2"),
            Some(ledger.clone()),
        );
        recover_unacked(&session); // 不 panic、不报错
        assert_eq!(
            ledger.unacked().len(),
            1,
            "默认后端无法按需收取 → 账本保留待事件驱动"
        );
    }

    /// ingest 失败 → 账本记 Failed 且保持未确认（重试语义与 pending 同）。
    #[test]
    fn failed_ingestion_records_failed_state_in_ledger() {
        let root = tempfile::tempdir().unwrap();
        let db_dir = root.path().join("db");
        std::fs::create_dir_all(&db_dir).unwrap();
        let state = super::tests::test_state(&db_dir);
        let db = crate::ipc::open_library_db(&db_dir).unwrap();
        let album = db.album_create("失败").unwrap();
        let photos_root = root.path().join("photos");
        std::fs::create_dir(&photos_root).unwrap();
        let library = db
            .photos_library_register("失败库", &photos_root.to_string_lossy(), &db_dir)
            .unwrap();
        let incoming = root.path().join("incoming");
        std::fs::create_dir(&incoming).unwrap();
        let receipts_dir = root.path().join("receipts");
        let ledger = Arc::new(ReceiptsStore::open(&receipts_dir, "serial:SN-3"));
        let backend = ReceiptBackend {
            root: incoming.clone(),
            acknowledged: AtomicUsize::new(0),
            requested: Mutex::new(Vec::new()),
        };
        let mut session = super::tests::make_session_with(
            "failed",
            "test-camera-failed",
            album.id,
            Some("serial:SN-3"),
            Some(ledger.clone()),
        );
        {
            let session = Arc::get_mut(&mut session).unwrap();
            session.backend = Arc::new(backend);
            session.library = TetherLibrary {
                id: library.id,
                root: photos_root,
                organization_template:
                    crate::db::organization::DEFAULT_ORGANIZATION_TEMPLATE.to_string(),
            };
        }
        let mut truncated = staging::persist(&incoming, "broken.jpg", b"short").unwrap();
        truncated.object_size += 1; // 传输不完整 → ingest 失败
        assert!(receive(&state, &session, &truncated).is_err());
        let unacked = ledger.unacked();
        assert_eq!(unacked.len(), 1, "失败保持未确认（可重试）");
        assert_eq!(unacked[0].id, truncated.object_id);
    }
}
