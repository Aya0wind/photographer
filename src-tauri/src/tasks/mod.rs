//! 统一后台任务框架（TaskSupervisor，2026-09-18 统一架构）。
//!
//! 所有长活后台操作（设备扫描、导入引擎、未来的清卡/缩略图/AI）统一经
//! [`TaskSupervisor::spawn`] 派发，获得：
//!
//! - **panic 捕获**：任务体 panic 被 `catch_unwind` 拦下——`eprintln!` 带
//!   任务 kind/name 上下文 + 发布 [`AppEvent::AppError`]，进程绝不因子
//!   线程 panic 无声死亡（2026-09-18 真机教训：闪退无任何输出）。
//! - **线程统一命名**：`task-{kind}`（崩溃转储/日志可辨识）。
//! - **软取消/暂停语义**：[`TaskControls`]（原子标志，任务体轮询），
//!   与导入引擎的 `EngineControls` 同构——引擎经 supervisor 起线程，
//!   对外 pause/cancel/is_done 接口保持一致。
//!
//! 资源所有权规则（架构约定）：任务用到的设备源/句柄在任务线程内获取
//! 与释放；WPD 源为无状态代理（COM 生命周期收敛于 wpd worker 线程），
//! 任务线程可自由持有/丢弃。
//!
//! 请求式有返回值的工作（清卡/缩略图/DB 查询）走 `ipc::run_blocking`
//! （同为后台线程；panic 经 JoinError 归一为命令错误，同样不会带崩进程）。

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::events::{AppEvent, EventBus};

/// 任务控制柄（任务体收到的软控制接口）：pause/resume/cancel 均为原子
/// 标志，由任务体轮询决定何时生效（软语义）。
#[derive(Clone, Debug)]
/// 软控制柄。方法面向调用方/集成测试（lib 内部 spawn 体暂以 `|_|` 忽略
/// 控制——预留统一暂停/取消接线），豁免 lib 目标的 dead_code 检查。
#[allow(dead_code)]
pub struct TaskControls {
    paused: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl TaskControls {
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
}

/// 派发返回的任务句柄：调用方侧的控制与状态观测。
/// 任务句柄：观测（kind/name/id/is_done）与软控制；lib 内部暂只经
/// supervisor 使用（接口预留统一暂停/取消 + 前端任务中心）。
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct TaskHandle {
    controls: TaskControls,
    done: Arc<AtomicBool>,
    id: u64,
    kind: &'static str,
    name: String,
}

#[allow(dead_code)]
impl TaskHandle {
    /// 软暂停（任务体轮询生效）。
    pub fn pause(&self) {
        self.controls.pause();
    }

    /// 恢复。
    pub fn resume(&self) {
        self.controls.resume();
    }

    /// 软取消（任务体轮询生效）。
    pub fn cancel(&self) {
        self.controls.cancel();
    }

    pub fn is_paused(&self) -> bool {
        self.controls.is_paused()
    }

    pub fn is_cancelled(&self) -> bool {
        self.controls.is_cancelled()
    }

    /// 任务是否已收尾（正常结束或 panic 被捕获）。
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    pub fn kind(&self) -> &'static str {
        self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn id(&self) -> u64 {
        self.id
    }
}

/// 任务监督器：统一派发/命名/panic 捕获/事件上报。
/// 经 `AppState.supervisor` 全局一份。
pub struct TaskSupervisor {
    bus: EventBus,
    next_id: AtomicU64,
    /// id -> (kind, name)；Arc 以便任务线程收尾时自行摘除。
    running: Arc<Mutex<HashMap<u64, (String, String)>>>,
}

impl TaskSupervisor {
    pub fn new(bus: EventBus) -> Arc<Self> {
        Arc::new(Self {
            bus,
            next_id: AtomicU64::new(1),
            running: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// 派发一个后台任务：线程名 `task-{kind}`；panic 捕获 + appError 事件。
    /// 任务体收到 [`TaskControls`]（软取消/暂停），返回值丢弃（结果经事件/
    /// 数据库传递）。
    pub fn spawn<F>(&self, kind: &'static str, name: String, body: F) -> TaskHandle
    where
        F: FnOnce(TaskControls) + Send + 'static,
    {
        self.spawn_impl(kind, name, body, false)
            .expect("non-unique task spawn always returns a handle")
    }

    /// 按 `(kind, name)` 幂等派发。已有同名任务运行时不再新建线程，调用方
    /// 可把自动触发与手动触发安全地汇入同一后台管线。
    pub fn spawn_unique<F>(&self, kind: &'static str, name: String, body: F) -> Option<TaskHandle>
    where
        F: FnOnce(TaskControls) + Send + 'static,
    {
        self.spawn_impl(kind, name, body, true)
    }

    fn spawn_impl<F>(
        &self,
        kind: &'static str,
        name: String,
        body: F,
        unique: bool,
    ) -> Option<TaskHandle>
    where
        F: FnOnce(TaskControls) + Send + 'static,
    {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let controls = TaskControls {
            paused: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        let done = Arc::new(AtomicBool::new(false));
        let handle = TaskHandle {
            controls: controls.clone(),
            done: Arc::clone(&done),
            id,
            kind,
            name: name.clone(),
        };

        {
            let mut running = self
                .running
                .lock()
                .expect("supervisor running mutex poisoned");
            if unique
                && running.values().any(|(running_kind, running_name)| {
                    running_kind == kind && running_name == &name
                })
            {
                return None;
            }
            running.insert(id, (kind.to_string(), name.clone()));
        }

        let bus = self.bus.clone();
        let running = Arc::clone(&self.running);
        let done_flag = Arc::clone(&done);
        let log_name = name.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("task-{kind}"))
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(move || body(controls)));
                done_flag.store(true, Ordering::SeqCst);
                running
                    .lock()
                    .expect("supervisor running mutex poisoned")
                    .remove(&id);
                if let Err(payload) = outcome {
                    // 子线程 panic 绝不带崩进程：日志 + appError 事件（带任务上下文）
                    let detail = panic_message(&payload);
                    eprintln!("后台任务 panic（task-{kind}/{log_name}）: {detail}");
                    bus.publish(AppEvent::AppError {
                        level: "error".into(),
                        message: format!("后台任务 {kind}/{log_name} 崩溃: {detail}"),
                        recoverable: true,
                    });
                }
            });
        if let Err(err) = spawned {
            // 线程启动失败：同样收尾 + 上报（不静默）
            done.store(true, Ordering::SeqCst);
            self.running
                .lock()
                .expect("supervisor running mutex poisoned")
                .remove(&id);
            eprintln!("后台任务线程启动失败（task-{kind}/{name}）: {err}");
            self.bus.publish(AppEvent::AppError {
                level: "error".into(),
                message: format!("后台任务 {kind}/{name} 无法启动: {err}"),
                recoverable: true,
            });
        }
        Some(handle)
    }

    /// 当前运行中的任务数（观测；集成测试引用）。
    #[allow(dead_code)]
    pub fn running_count(&self) -> usize {
        self.running
            .lock()
            .expect("supervisor running mutex poisoned")
            .len()
    }
}

/// 等待句柄收尾（测试/运维用；超时返回 false）。
#[allow(dead_code)]
pub fn wait_done(handle: &TaskHandle, timeout: Duration) -> bool {
    let started = std::time::Instant::now();
    while !handle.is_done() {
        if started.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    true
}

/// panic payload → 可读消息（&str / String / 其他）。
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "未知 panic 载荷".to_string()
    }
}
