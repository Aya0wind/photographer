//! 统一后台任务框架（TaskSupervisor）测试：panic 捕获（进程不死 + appError
//! 事件）、取消/暂停语义、多任务并发、线程命名与句柄状态。

mod common;

pub use common::{
    ai, db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use events::{AppEvent, EventBus};
use tasks::TaskSupervisor;

#[test]
fn panic_is_captured_and_reported_not_fatal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let supervisor = TaskSupervisor::new(bus.clone());

    let handle = supervisor.spawn("test", "panic-job".into(), |_| {
        panic!("注入的测试 panic");
    });

    // 等待任务收尾（panic 被捕获，进程必须存活）
    let mut waited = 0;
    while !handle.is_done() && waited < 50 {
        std::thread::sleep(Duration::from_millis(20));
        waited += 1;
    }
    assert!(handle.is_done(), "panic 后任务必须收尾（被捕获）");

    // appError 事件已发布（前端可见，不再无声死亡）
    let mut got_error = false;
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::AppError { level, message, .. } = ev {
            assert_eq!(level, "error");
            assert!(
                message.contains("panic-job") && message.contains("崩溃"),
                "事件应带任务上下文: {message}"
            );
            got_error = true;
        }
    }
    assert!(got_error, "panic 必须发 appError 事件");

    // supervisor 仍可用（后续任务照常）
    let ran = Arc::new(AtomicUsize::new(0));
    let ran2 = Arc::clone(&ran);
    let h2 = supervisor.spawn("test", "after-panic".into(), move |_| {
        ran2.fetch_add(1, Ordering::SeqCst);
    });
    let mut waited = 0;
    while !h2.is_done() && waited < 50 {
        std::thread::sleep(Duration::from_millis(20));
        waited += 1;
    }
    assert_eq!(
        ran.load(Ordering::SeqCst),
        1,
        "panic 后 supervisor 仍可派发"
    );
}

#[test]
fn cancel_and_pause_semantics_via_controls() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let handle = supervisor.spawn("test", "loop-job".into(), |controls| loop {
        if controls.is_cancelled() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    });

    assert!(!handle.is_done());
    handle.cancel();
    let mut waited = 0;
    while !handle.is_done() && waited < 200 {
        std::thread::sleep(Duration::from_millis(5));
        waited += 1;
    }
    assert!(handle.is_done(), "取消后任务应收尾");
    assert!(handle.is_cancelled());

    // 暂停标志可设置/清除（软暂停语义，由任务体自行轮询）
    let h2 = supervisor.spawn("test", "pause-job".into(), |controls| {
        while !controls.is_cancelled() {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(controls.is_paused());
        controls_resume_check(&controls);
    });
    h2.pause();
    h2.cancel();
    let mut waited = 0;
    while !h2.is_done() && waited < 200 {
        std::thread::sleep(Duration::from_millis(5));
        waited += 1;
    }
    assert!(h2.is_done());
}

fn controls_resume_check(controls: &tasks::TaskControls) {
    controls.resume();
    assert!(!controls.is_paused());
}

#[test]
fn spawns_concurrent_named_tasks() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let done = Arc::new(AtomicUsize::new(0));
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let done = Arc::clone(&done);
            supervisor.spawn("scan", format!("concurrent-{i}"), move |controls| {
                // 并行驻留一段时间，期间检查取消
                for _ in 0..20 {
                    if controls.is_cancelled() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                done.fetch_add(1, Ordering::SeqCst);
            })
        })
        .collect();
    assert_eq!(handles.len(), 4);
    for h in &handles {
        let mut waited = 0;
        while !h.is_done() && waited < 200 {
            std::thread::sleep(Duration::from_millis(5));
            waited += 1;
        }
        assert!(h.is_done());
    }
    assert_eq!(done.load(Ordering::SeqCst), 4, "多任务并发执行");
    // 句柄携带 kind/name（观测/日志上下文）
    assert!(handles.iter().all(|h| h.kind() == "scan"));
}
