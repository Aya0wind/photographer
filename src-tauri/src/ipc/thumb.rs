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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};

use serde::Serialize;
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
    /// (asset,size) 连续失败计数（成功清零；≥3 视为永久失败不再入队，
    /// 防止「事件→重试→再入队→再失败」的低频死循环）。
    failures: HashMap<(i64, u16), u32>,
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
                    failures: HashMap::new(),
                    stop: false,
                }),
                ready: Condvar::new(),
            }),
            workers_started: Mutex::new(false),
        }
    }

    /// 入队：去抖键命中返回 true（不重复入队）；连续失败 ≥3 次返回 false
    /// （调用方报 Unavailable）；队满丢弃返回 false。
    pub fn push(&self, job: ThumbJob) -> bool {
        let key = (job.asset_id, job.size);
        let mut state = self.inner.state.lock().expect("thumb queue mutex poisoned");
        if state.queued.contains(&key) {
            return true; // 去抖：已有同键任务在途
        }
        if state.failures.get(&key).copied().unwrap_or(0) >= 3 {
            return false; // 永久失败：不再入队
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

    /// 生成结果记账：成功清失败计数；失败 +1（达 3 后不再入队）。
    fn record_result(inner: &QueueInner, key: (i64, u16), ok: bool) {
        let mut state = inner.state.lock().expect("thumb queue mutex poisoned");
        if ok {
            state.failures.remove(&key);
        } else {
            *state.failures.entry(key).or_insert(0) += 1;
        }
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
                    ThumbQueue::record_result(&inner, (job.asset_id, job.size), path.is_some());
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

/// 按需缩略图结果四态（前端「排队中≠失败≠源丢失」的关键区分）：
/// - Ready：缓存命中，附绝对路径
/// - Pending：已入队后台生成，`ThumbnailReady` 事件到达后前端重试即 Ready
/// - Missing：**源文件不在盘**（第三方移动/删除）——终态，不入队不重试；
///   `cached_path` 尽力恢复已生成过的缓存缩略图（无则 null），查看器可用
///   它直接展示而不是无限转圈（2026-09-28 边界修复）
/// - Unavailable：永久不可用（资产不存在 / thumb_state=2 / 不可解码）
///   ——此前统一返回 null，前端把「排队中」误判成「无内嵌预览」而过早
///   启用 rawler 显影兜底（真机 NAS ARW 219ms 即回落显影的根因）。
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ThumbOutcome {
    Ready {
        path: String,
    },
    Pending,
    /// 源缺失：`cachedPath` = 恢复出的既有缓存缩略图绝对路径（或 null）。
    #[serde(rename_all = "camelCase")]
    Missing {
        cached_path: Option<String>,
    },
    Unavailable,
}

/// 按需缩略图（画廊主通道）：命中返回缓存绝对路径；未命中入队后台生成
/// （完成后 `ThumbnailReady` 事件回执）；源文件缺失返回 Missing（终态，
/// 带尽力恢复的 cached_path）；无法解码/资产不存在返回 Unavailable（不入队）。
pub fn fetch_asset_thumb(
    state: &super::AppState,
    asset_id: i64,
    size: u16,
) -> Result<ThumbOutcome, String> {
    let db_dir = super::app_database_dir(state);
    let db = super::open_library_db(&db_dir)?;
    // 状态分流（thumb_state 列 O(1) 判断）：1=缓存命中直返（文件被清则
    // 落入兜底入队重生成）；2=永久占位（不可解码/三次失败）不排队；
    // 0=pending 按需兜底（插队生成，索引 worker 之外的快速通道）。
    let Some((path, thumb_state)) = db.thumb_info_by_id(asset_id).map_err(|e| e.to_string())?
    else {
        return Ok(ThumbOutcome::Unavailable); // 资产不存在：不入队
    };
    let src = PathBuf::from(path);
    // 访问时惰性缺失检测（§五 M2c）：画廊瓦片是最高频访问点，stat 在盘性
    // 顺手对账 missing 标记（两轮确认，与库扫描共用缺席账）。整库离线 /
    // 资产不存在返回 None → 退回直接 stat 判定（不标缺失）。
    let src_exists = super::assets::detect_missing_on_access(&db, asset_id).unwrap_or(src.exists());
    if let Some(hit) = crate::thumbs::cached(&db_dir, &src, size) {
        // 命中直返（零事件零入队）。state=2 也不再保留：源在盘 + 缓存在盘
        // = 一切正常，顺手治愈（2026-09-28 自愈修复：此前 state 2 命中
        // 缓存也不翻 1，DB 永久卡占位）。
        let _ = db.set_thumb_state(asset_id, 1);
        return Ok(ThumbOutcome::Ready { path: hit });
    }
    // 源缺失：终态 Missing（不入队——解码注定失败，入队只会让前端无限
    // Pending 重拉）。cached_path 尽力恢复既有缓存（按 xxh64 前缀扫档位
    // 目录），查看器/编辑器拿它直接出图而非空白舞台。
    if !src_exists {
        let cached_path = crate::thumbs::cached_without_source(&db_dir, &src, size);
        return Ok(ThumbOutcome::Missing { cached_path });
    }
    let decodable = crate::thumbs::is_decodable(&src);
    if thumb_state == 2 && decodable {
        // state=2 自愈（2026-09-28）：文件被移回 + 缓存被清的组合下，2 是
        // 缺失/失败期留下的脏占位——源在盘且可解码就重置 0 走正常入队重生成。
        let _ = db.set_thumb_state(asset_id, 0);
    } else if thumb_state == 2 || !decodable {
        return Ok(ThumbOutcome::Unavailable); // 永久占位/不可解码：不入队
    }
    let queued = state.thumb_queue.push(ThumbJob {
        asset_id,
        size,
        src,
        db_dir,
    });
    if queued {
        state
            .thumb_queue
            .ensure_workers(&state.supervisor, &state.bus);
        Ok(ThumbOutcome::Pending)
    } else {
        // 队满丢弃：没有回执事件，语义上仍是「生成中」（前端滚动重试自愈）
        Ok(ThumbOutcome::Pending)
    }
}

/// 取缩略图（画廊主通道，asset_id 语义）。解码/IO → 后台线程；错误归一
/// Unavailable，契约四态 `ThumbOutcome`（Ready/Pending/Missing/Unavailable）。
/// 命名 asset_thumb_get 与路径语义的 thumb_get_by_path 区分（前端契约）。
#[tauri::command]
pub async fn asset_thumb_get(
    state: State<'_, SharedState>,
    asset_id: i64,
    size: u16,
) -> Result<ThumbOutcome, String> {
    let shared = state.inner().clone();
    Ok(run_blocking(shared, move |state| {
        fetch_asset_thumb(state, asset_id, size)
    })
    .await
    .unwrap_or(ThumbOutcome::Unavailable))
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
        let db_dir = super::app_database_dir(state);
        let thumb = crate::thumbs::thumb_file(&db_dir, std::path::Path::new(&path), size);
        Ok::<Option<String>, String>(thumb)
    })
    .await
    .ok()
    .flatten();
    Ok(thumb)
}

/// 相机导入预览缓存。先取内存中的源代理，释放注册表锁后才访问设备。
pub fn fetch_device_thumb(
    state: &super::AppState,
    device_id: &str,
    object_id: &str,
    version: &str,
    size: u16,
) -> Result<Option<String>, String> {
    let key = crate::devices::normalize_device_id(device_id);
    let source = state
        .devices
        .lock()
        .map_err(|_| "设备状态不可用")?
        .get(&key)
        .map(|entry| Arc::clone(&entry.source))
        .ok_or("设备已断开")?;
    let db_dir = {
        let settings = state.settings.lock().map_err(|_| "库状态不可用")?;
        settings.database_dir_path(&state.config_dir)
    };
    let size = crate::thumbs::snap_size(size);
    let cache_key =
        serde_json::to_vec(&(key, object_id, version, size)).map_err(|e| e.to_string())?;
    let dir = db_dir.join("thumbs").join("import-device");
    let target = dir.join(format!(
        "{:016x}.jpg",
        xxhash_rust::xxh64::xxh64(&cache_key, 0)
    ));
    if target.is_file() {
        return Ok(Some(target.to_string_lossy().into_owned()));
    }
    let Some(bytes) = source.thumbnail(object_id).map_err(|e| e.to_string())? else {
        return Ok(None);
    };
    if bytes.len() > 2 * 1024 * 1024 {
        return Ok(None);
    }
    if !state
        .devices
        .lock()
        .map_err(|_| "设备状态不可用")?
        .get(&source.id())
        .is_some_and(|entry| Arc::ptr_eq(&entry.source, &source))
    {
        return Err("设备已断开或重新连接".into());
    }
    // 编码体和解码尺寸都有上限，异常设备资源不会变成一次原图解码。
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|e| format!("无法读取相机预览: {e}"))?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temporary = dir.join(format!("{}.tmp", uuid::Uuid::new_v4()));
    let result = image
        .thumbnail(size as u32, size as u32)
        .to_rgb8()
        .save_with_format(&temporary, image::ImageFormat::Jpeg)
        .map_err(|e| e.to_string())
        .and_then(|_| std::fs::rename(&temporary, &target).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result?;
    Ok(Some(target.to_string_lossy().into_owned()))
}

#[tauri::command]
pub async fn device_thumb_get(
    state: State<'_, SharedState>,
    device_id: String,
    object_id: String,
    version: String,
    size: u16,
) -> Result<Option<String>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |state| {
        fetch_device_thumb(state, &device_id, &object_id, &version, size)
    })
    .await
}
