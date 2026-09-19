//! 设备枚举独立排队；每台相机的数据操作与空闲探活共享一个通道。
//! 超时后隔离旧线程；Windows COM 无法安全强杀，最多容许每通道两个
//! 未退出的旧线程，避免坏驱动造成无限线程增长。旧结果不再提交。
use super::{DeviceError, DeviceResult};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

type Job = Box<dyn FnOnce() + Send>;
thread_local! {
    static OPERATION: std::cell::RefCell<Option<Operation>> = const { std::cell::RefCell::new(None) };
}
struct Operation {
    cancelled: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    pending: Arc<AtomicUsize>,
    progress: Arc<Mutex<std::time::Instant>>,
}
pub struct OperationLease(Arc<AtomicUsize>);
impl Drop for OperationLease {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
/// 流泵继承正在使用设备的标记，防止探活另开会话。
pub fn lease_current_operation() -> Option<OperationLease> {
    OPERATION.with(|context| {
        context.borrow().as_ref().map(|op| {
            op.pending.fetch_add(1, Ordering::AcqRel);
            OperationLease(Arc::clone(&op.pending))
        })
    })
}
/// 旧连接在下一次 COM 调用前退出，不能继续枚举重连后的相机。
pub fn checkpoint() -> DeviceResult<()> {
    OPERATION.with(|context| {
        if context.borrow().as_ref().is_some_and(|op| {
            op.cancelled.load(Ordering::Acquire) || op.stopped.load(Ordering::Acquire)
        }) {
            Err(DeviceError::Disconnected)
        } else {
            if let Some(op) = context.borrow().as_ref() {
                *op.progress.lock().expect("progress mutex poisoned") = std::time::Instant::now();
            }
            Ok(())
        }
    })
}
struct Worker {
    id: u64,
    tx: mpsc::Sender<Job>,
    handle: JoinHandle<()>,
    stopped: Arc<AtomicBool>,
    pending: Arc<AtomicUsize>,
}
#[derive(Default)]
struct Lane {
    next: u64,
    current: Option<Worker>,
    retired: Vec<JoinHandle<()>>,
}
pub struct WorkerPool {
    lanes: Mutex<HashMap<String, Lane>>,
    initialize: fn() -> Box<dyn std::any::Any>,
}

impl Default for WorkerPool {
    fn default() -> Self {
        Self::with_initializer(|| Box::new(()))
    }
}

impl WorkerPool {
    pub fn with_initializer(initialize: fn() -> Box<dyn std::any::Any>) -> Self {
        Self {
            lanes: Mutex::new(HashMap::new()),
            initialize,
        }
    }
    pub fn invalidate(&self, key: &str) {
        let mut lanes = self.lanes.lock().expect("worker lanes poisoned");
        if let Some(lane) = lanes.get_mut(key) {
            if let Some(worker) = lane.current.take() {
                worker.stopped.store(true, Ordering::Release);
                drop(worker.tx);
                lane.retired.push(worker.handle);
            }
        }
    }
    pub fn call<T: Send + 'static>(
        &self,
        key: &str,
        timeout: Duration,
        operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
    ) -> DeviceResult<T> {
        self.call_inner(key, timeout, false, None, operation)
            .map(|value| value.expect("regular calls are scheduled"))
    }
    /// 文件扫描按无进展时长超时；大卡持续读取时不受固定总时长截断。
    pub fn call_with_idle_timeout<T: Send + 'static>(
        &self,
        key: &str,
        timeout: Duration,
        operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
    ) -> DeviceResult<T> {
        self.call_inner(key, timeout, false, Some(timeout), operation)
            .map(|value| value.expect("regular calls are scheduled"))
    }
    /// 在同一个设备通道内原子检查并派发；忙碌时不排队、不计探活失败。
    pub fn call_if_idle<T: Send + 'static>(
        &self,
        key: &str,
        timeout: Duration,
        operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
    ) -> DeviceResult<Option<T>> {
        self.call_inner(key, timeout, true, None, operation)
    }
    fn call_inner<T: Send + 'static>(
        &self,
        key: &str,
        timeout: Duration,
        idle_only: bool,
        idle_timeout: Option<Duration>,
        operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
    ) -> DeviceResult<Option<T>> {
        let (tx, rx) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let skip = Arc::clone(&cancelled);
        let progress = Arc::new(Mutex::new(std::time::Instant::now()));
        let operation_progress = Arc::clone(&progress);
        let generation = {
            let mut lanes = self.lanes.lock().expect("worker lanes poisoned");
            let lane = lanes.entry(key.to_owned()).or_default();
            lane.retired.retain(|handle| !handle.is_finished());
            if idle_only
                && (lane
                    .current
                    .as_ref()
                    .is_some_and(|worker| worker.pending.load(Ordering::Acquire) > 0)
                    || !lane.retired.is_empty())
            {
                return Ok(None);
            }
            if lane.current.is_none() {
                if lane.retired.len() >= 2 {
                    return Err(DeviceError::Other(
                        "WPD 驱动仍无响应，请重新连接设备或重启应用".into(),
                    ));
                }
                let (sender, receiver) = mpsc::channel::<Job>();
                let stopped = Arc::new(AtomicBool::new(false));
                let stop = Arc::clone(&stopped);
                let initialize = self.initialize;
                let handle = std::thread::Builder::new()
                    .name("wpd-worker".into())
                    .spawn(move || {
                        // 在线程内创建并持有上下文，在线程内释放（WPD COM apartment）。
                        let _context = initialize();
                        while let Ok(job) = receiver.recv() {
                            if stop.load(Ordering::Acquire) {
                                break;
                            }
                            job();
                        }
                    })
                    .map_err(|err| DeviceError::Other(err.to_string()))?;
                lane.next += 1;
                lane.current = Some(Worker {
                    id: lane.next,
                    tx: sender,
                    handle,
                    stopped,
                    pending: Arc::new(AtomicUsize::new(0)),
                });
            }
            let worker = lane.current.as_ref().unwrap();
            worker.pending.fetch_add(1, Ordering::AcqRel);
            let lease = OperationLease(Arc::clone(&worker.pending));
            let pending = Arc::clone(&worker.pending);
            let stopped = Arc::clone(&worker.stopped);
            worker
                .tx
                .send(Box::new(move || {
                    let _lease = lease;
                    if skip.load(Ordering::Acquire) {
                        return;
                    }
                    OPERATION.with(|context| {
                        *context.borrow_mut() = Some(Operation {
                            cancelled: skip,
                            stopped,
                            pending,
                            progress: operation_progress,
                        })
                    });
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
                        .unwrap_or_else(|_| Err(DeviceError::Other("WPD worker panic".into())));
                    OPERATION.with(|context| *context.borrow_mut() = None);
                    let _ = tx.send(result);
                }))
                .map_err(|_| DeviceError::Other("WPD worker 不可用".into()))?;
            worker.id
        };
        let started = std::time::Instant::now();
        if !idle_only && key != "enumerate" {
            super::diagnostics::record(format!(
                "WPD request started: {key}; generation={generation}"
            ));
        }
        let response = if let Some(idle) = idle_timeout {
            loop {
                match rx.recv_timeout(idle.min(Duration::from_millis(50))) {
                    Err(mpsc::RecvTimeoutError::Timeout)
                        if progress.lock().expect("progress mutex poisoned").elapsed() < idle =>
                    {
                        continue
                    }
                    result => break result,
                }
            }
        } else {
            rx.recv_timeout(timeout)
        };
        match response {
            Ok(result) => {
                if !idle_only && key != "enumerate" {
                    super::diagnostics::record(format!(
                        "WPD request finished: {key}; elapsed={:?}; error={:?}",
                        started.elapsed(),
                        result.as_ref().err()
                    ));
                }
                result.map(Some)
            }
            Err(_) => {
                super::diagnostics::record(format!(
                    "WPD request timed out: {key}; elapsed={:?}",
                    started.elapsed()
                ));
                cancelled.store(true, Ordering::Release);
                let mut lanes = self.lanes.lock().expect("worker lanes poisoned");
                let lane = lanes.get_mut(key).unwrap();
                if lane
                    .current
                    .as_ref()
                    .is_some_and(|worker| worker.id == generation)
                {
                    let worker = lane.current.take().unwrap();
                    worker.stopped.store(true, Ordering::Release);
                    drop(worker.tx);
                    lane.retired.push(worker.handle);
                }
                Err(DeviceError::Other(format!("WPD {key} 超时或线程退出")))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ongoing_progress_can_exceed_idle_timeout() {
        let pool = WorkerPool::default();
        assert!(pool
            .call_with_idle_timeout("camera", Duration::from_millis(100), || {
                for _ in 0..20 {
                    checkpoint()?;
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            })
            .is_ok());
    }
    #[test]
    fn probe_does_not_interrupt_active_scan() {
        let pool = Arc::new(WorkerPool::default());
        let (started, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let worker_pool = Arc::clone(&pool);
        let task = std::thread::spawn(move || {
            worker_pool.call("camera", Duration::from_secs(2), move || {
                started.send(()).unwrap();
                gate.recv().unwrap();
                Ok(())
            })
        });
        wait.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(
            pool.call_if_idle("camera", Duration::from_millis(10), || panic!(
                "probe must not open another session"
            ))
            .unwrap(),
            None::<()>
        );
        release.send(()).unwrap();
        task.join().unwrap().unwrap();
    }
    #[test]
    fn invalidated_scan_stops_at_next_checkpoint() {
        let pool = Arc::new(WorkerPool::default());
        let (started, wait) = mpsc::channel();
        let (release, gate) = mpsc::channel();
        let worker_pool = Arc::clone(&pool);
        let task = std::thread::spawn(move || {
            worker_pool.call("camera", Duration::from_secs(2), move || {
                started.send(()).unwrap();
                gate.recv().unwrap();
                checkpoint()
            })
        });
        wait.recv_timeout(Duration::from_secs(1)).unwrap();
        pool.invalidate("camera");
        release.send(()).unwrap();
        assert!(matches!(
            task.join().unwrap(),
            Err(DeviceError::Disconnected)
        ));
    }
    #[test]
    fn blocked_lane_does_not_stop_detection_and_can_be_replaced() {
        let pool = WorkerPool::default();
        let (release, wait) = mpsc::channel();
        assert!(pool
            .call("data", Duration::from_millis(30), move || {
                let _ = wait.recv();
                Ok(())
            })
            .is_err());
        assert_eq!(
            pool.call("enumerate", Duration::from_secs(1), || Ok(7))
                .unwrap(),
            7
        );
        assert_eq!(
            pool.call("data", Duration::from_secs(1), || Ok(8)).unwrap(),
            8
        );
        release.send(()).unwrap();
    }
    #[test]
    fn repeated_hangs_are_bounded() {
        let pool = WorkerPool::default();
        let mut releases: Vec<mpsc::Sender<()>> = Vec::new();
        for _ in 0..2 {
            let (release, wait) = mpsc::channel();
            releases.push(release);
            assert!(pool
                .call("probe", Duration::from_millis(30), move || {
                    let _ = wait.recv();
                    Ok(())
                })
                .is_err());
        }
        assert!(pool
            .call("probe", Duration::from_secs(1), || Ok(()))
            .is_err());
        assert!(pool
            .call("other-camera", Duration::from_secs(1), || Ok(()))
            .is_ok());
        drop(releases);
    }
}
