//! thumb 命令与按需生成队列（M3）。
//!
//! - `asset_thumb_get(asset_id, size)`（画廊主通道）：缓存命中立即返回绝对路径；
//!   未命中把**已解析完整**的生成任务（asset 路径 + dbDir，worker 不回查
//!   AppState，避免 Arc 环）压入有界队列并返回 null——生成完成后经
//!   `AppEvent::ThumbnailReady` 回执，前端收到后重试即命中。
//! - `thumb_get_by_path(path, size)`（旧通道，导入卡等按路径场景）：同步
//!   生成语义不变（spawn_blocking 后台解码）。
//!
//! 队列语义：同 (asset,size) 去抖（pending/processing 期间重复请求直接吞并）；
//! 容量 [`QUEUE_CAPACITY`]，满则丢弃（push 返回 false → 命令返回 null，
//! 前端滚动重试自愈）；worker 经 TaskSupervisor 派发（panic 捕获 + 命名），
//! 惰性启动（首个未命中请求时拉起 2 个），ThumbQueue Drop 时停工。
//! 解码复用 `crate::thumbs`（turbojpeg 管线 + 自带超时/许可），worker 数
//! 固定 2 = 队列生成并发 ≤2。

use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use tauri::State;

use super::{run_blocking, SharedState};
use crate::events::{AppEvent, EventBus};
use crate::tasks::TaskSupervisor;

/// 队列容量（滚动网格远超屏幕可见数；满即丢，前端重试）。
pub const QUEUE_CAPACITY: usize = 256;
/// worker 数（= 按需生成并发上限）。
const WORKERS: usize = 2;

/// 一次按需生成任务：入队时已解析全部依赖，worker 无需 AppState。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThumbJob {
    pub asset_id: i64,
    pub size: u16,
    /// 源文件绝对路径（assets.path）。
    pub src: PathBuf,
    /// 库 dbDir（缓存根）。
    pub db_dir: PathBuf,
}

struct QueueState {
    pending: VecDeque<ThumbJob>,
    /// 待处理 + 处理中的 (asset,size) 键（去抖）。
    queued: HashSet<(i64, u16)>,
    stop: bool,
}

struct QueueInner {
    state: Mutex<QueueState>,
    ready: Condvar,
}

/// 按需缩略图生成队列（AppState 持有一份；Clone 共享同一队列）。
pub struct ThumbQueue {
    inner: Arc<QueueInner>,
    workers_started: Mutex<bool>,
}

impl Default for ThumbQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl ThumbQueue {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(QueueInner {
                state: Mutex::new(QueueState {
                    pending: VecDeque::new(),
                    queued: HashSet::new(),
                    stop: false,
                }),
                ready: Condvar::new(),
            }),
            workers_started: Mutex::new(false),
        }
    }

    /// 入队：去抖键命中返回 true（不重复入队）；队满丢弃返回 false。
    pub fn push(&self, job: ThumbJob) -> bool {
        let key = (job.asset_id, job.size);
        let mut state = self.inner.state.lock().expect("thumb queue mutex poisoned");
        if state.queued.contains(&key) {
            return true; // 去抖：已有同键任务在途
        }
        if state.pending.len() >= QUEUE_CAPACITY {
            return false; // 溢出丢弃（前端重试）
        }
        state.pending.push_back(job);
        state.queued.insert(key);
        drop(state);
        self.inner.ready.notify_one();
        true
    }

    /// worker 取任务：队列空则阻塞等待；停工返回 None。
    fn pop(inner: &QueueInner) -> Option<ThumbJob> {
        let mut state = inner.state.lock().expect("thumb queue mutex poisoned");
        loop {
            if let Some(job) = state.pending.pop_front() {
                state.queued.remove(&(job.asset_id, job.size));
                return Some(job);
            }
            if state.stop {
                return None;
            }
            state = inner.ready.wait(state).expect("thumb queue mutex poisoned");
        }
    }

    /// 惰性启动 worker（首个未命中请求时调用；幂等）。经 TaskSupervisor
    /// 派发获得 panic 捕获与统一命名。
    pub fn ensure_workers(&self, supervisor: &Arc<TaskSupervisor>, bus: &EventBus) {
        let mut started = self
            .workers_started
            .lock()
            .expect("thumb workers flag mutex poisoned");
        if *started {
            return;
        }
        *started = true;
        for n in 0..WORKERS {
            let inner = Arc::clone(&self.inner);
            let bus = bus.clone();
            supervisor.spawn("thumb", format!("on-demand-{n}"), move |_| {
                while let Some(job) = ThumbQueue::pop(&inner) {
                    // 复用 turbojpeg 管线：阻塞生成（自带并发许可 + 10s 超时）
                    let path = crate::thumbs::thumb_file(&job.db_dir, &job.src, job.size);
                    bus.publish(AppEvent::ThumbnailReady {
                        asset_id: job.asset_id,
                        size: crate::thumbs::snap_size(job.size),
                        path,
                    });
                }
            });
        }
    }

    /// 测试/观测钩子：待处理任务数（不含处理中）。
    #[doc(hidden)]
    #[allow(dead_code)] // 集成测试引用（tests/ 不可见 lib 内 cfg(test)）
    pub fn pending_len(&self) -> usize {
        self.inner
            .state
            .lock()
            .expect("thumb queue mutex poisoned")
            .pending
            .len()
    }
}

impl Drop for ThumbQueue {
    fn drop(&mut self) {
        // AppState 生命周期结束：唤醒全部 worker 退出（stop 标志由锁保护）
        let mut state = self.inner.state.lock().expect("thumb queue mutex poisoned");
        state.stop = true;
        drop(state);
        self.inner.ready.notify_all();
    }
}

/// 按需缩略图（画廊主通道）：命中返回缓存绝对路径；未命中入队后台生成
/// （完成后 `ThumbnailReady` 事件回执），本轮返回 None；RAW/视频/资产
/// 不存在也返回 None（不入队）。
pub fn fetch_asset_thumb(
    state: &super::AppState,
    asset_id: i64,
    size: u16,
) -> Result<Option<String>, String> {
    let library = state
        .settings
        .lock()
        .expect("settings mutex poisoned")
        .active_library()
        .cloned()
        .ok_or("尚未创建库")?;
    let db_dir = PathBuf::from(&library.db_dir);
    let db = super::open_library_db(&db_dir)?;
    let Some(path) = db.asset_path_by_id(asset_id).map_err(|e| e.to_string())? else {
        return Ok(None); // 资产不存在：null（不入队）
    };
    let src = PathBuf::from(path);
    if let Some(hit) = crate::thumbs::cached(&db_dir, &src, size) {
        return Ok(Some(hit)); // 命中直返，零事件零入队
    }
    if !crate::thumbs::is_decodable(&src) {
        return Ok(None); // RAW/视频 v1 不可解码：null（不入队，无谓的失败回执）
    }
    state.thumb_queue.push(ThumbJob {
        asset_id,
        size,
        src,
        db_dir,
    });
    state
        .thumb_queue
        .ensure_workers(&state.supervisor, &state.bus);
    Ok(None)
}

/// 取缩略图（画廊主通道，asset_id 语义）。解码/IO → 后台线程；错误与
/// 未命中统一归一 null（契约 `String | null`，不向前端抛 reject）。
/// 命名 asset_thumb_get 与路径语义的 thumb_get_by_path 区分（前端契约）。
#[tauri::command]
pub async fn asset_thumb_get(
    state: State<'_, SharedState>,
    asset_id: i64,
    size: u16,
) -> Result<Option<String>, String> {
    let shared = state.inner().clone();
    Ok(run_blocking(shared, move |state| {
        fetch_asset_thumb(state, asset_id, size)
    })
    .await
    .ok()
    .flatten())
}

/// 取缩略图（路径语义，旧通道：导入进度卡等已知路径场景）。同步生成，
/// 返回缓存文件绝对路径；无法生成返回 null。
#[tauri::command]
pub async fn thumb_get_by_path(
    state: State<'_, SharedState>,
    path: String,
    size: u16,
) -> Result<Option<String>, String> {
    let shared = state.inner().clone();
    let thumb = run_blocking(shared, move |state| {
        let library = state
            .settings
            .lock()
            .expect("settings mutex poisoned")
            .active_library()
            .cloned();
        let thumb: Option<String> = library.and_then(|lib| {
            let db_dir = PathBuf::from(lib.db_dir);
            crate::thumbs::thumb_file(&db_dir, std::path::Path::new(&path), size)
        });
        Ok::<Option<String>, String>(thumb)
    })
    .await
    .ok()
    .flatten();
    Ok(thumb)
}
