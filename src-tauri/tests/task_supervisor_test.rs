//! 统一后台任务框架（TaskSupervisor）测试：panic 捕获（进程不死 + appError
//! 事件）、取消/暂停语义、多任务并发、线程命名与句柄状态。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, settings, tasks, thumbs,
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

#[test]
fn unique_spawn_coalesces_automatic_and_manual_triggers() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let ran = Arc::new(AtomicUsize::new(0));
    let ran_first = Arc::clone(&ran);
    let first = supervisor
        .spawn_unique("index", "semantic-backfill".into(), move |_| {
            ran_first.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
        })
        .expect("首次触发应启动任务");

    let ran_duplicate = Arc::clone(&ran);
    let duplicate = supervisor.spawn_unique("index", "semantic-backfill".into(), move |_| {
        ran_duplicate.fetch_add(1, Ordering::SeqCst);
    });
    assert!(duplicate.is_none(), "同一管线运行中时应合并重复触发");

    assert!(tasks::wait_done(&first, Duration::from_secs(1)));
    assert_eq!(ran.load(Ordering::SeqCst), 1);

    let ran_again = Arc::clone(&ran);
    let again = supervisor
        .spawn_unique("index", "semantic-backfill".into(), move |_| {
            ran_again.fetch_add(1, Ordering::SeqCst);
        })
        .expect("前一任务结束后应允许再次触发");
    assert!(tasks::wait_done(&again, Duration::from_secs(1)));
    assert_eq!(ran.load(Ordering::SeqCst), 2);
}

#[test]
fn coalesced_wakeup_runs_again_after_queue_was_drained() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let runs = Arc::new(AtomicUsize::new(0));
    let (drained_tx, drained_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let counter = Arc::clone(&runs);
    let handle = supervisor
        .spawn_coalesced("index", "library-a".into(), move |_| {
            if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                drained_tx.send(()).unwrap();
                release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        })
        .unwrap();
    drained_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    for _ in 0..20 {
        assert!(supervisor
            .spawn_coalesced("index", "library-a".into(), |_| {
                panic!("同名请求应合并，不能并行执行");
            })
            .is_none());
    }
    release_tx.send(()).unwrap();
    assert!(tasks::wait_done(&handle, Duration::from_secs(5)));
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    let counter = Arc::clone(&runs);
    let handle = supervisor
        .spawn_coalesced("index", "library-a".into(), move |_| {
            counter.fetch_add(1, Ordering::SeqCst);
        })
        .unwrap();
    assert!(tasks::wait_done(&handle, Duration::from_secs(5)));
    assert_eq!(runs.load(Ordering::SeqCst), 3);
}

#[test]
fn coalesced_panic_releases_registration() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let handle = supervisor
        .spawn_coalesced("index", "library-a".into(), |_| {
            panic!("注入失败");
        })
        .unwrap();
    assert!(tasks::wait_done(&handle, Duration::from_secs(5)));
    let handle = supervisor
        .spawn_coalesced("index", "library-a".into(), |_| {})
        .unwrap();
    assert!(tasks::wait_done(&handle, Duration::from_secs(5)));
}

#[test]
fn pause_kind_and_resume_kind_toggle_running_tasks_by_kind() {
    let supervisor = TaskSupervisor::new(EventBus::new());
    let entered = Arc::new(AtomicUsize::new(0));
    let release = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let probe = Arc::clone(&entered);
    let flag = Arc::clone(&release);
    let handle = supervisor
        .spawn(
            "index",
            "pool-probe".into(),
            move |controls| {
                probe.fetch_add(1, Ordering::SeqCst);
                while !controls.is_cancelled() && !flag.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(5));
                }
            },
        );
    // 等任务真正进场（登记表有了条目）
    let started = std::time::Instant::now();
    while entered.load(Ordering::SeqCst) == 0 {
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    // 按 kind 暂停/恢复：命中在跑的 index 任务，不误伤其他 kind
    let other = supervisor
        .spawn("import", "other".into(), |controls| {
            while !controls.is_cancelled() {
                std::thread::sleep(Duration::from_millis(5));
            }
        });
    assert_eq!(supervisor.pause_kind("index"), 1);
    assert!(handle.is_paused());
    assert!(!other.is_paused(), "pause_kind 不得跨 kind 生效");
    assert_eq!(supervisor.resume_kind("index"), 1);
    assert!(!handle.is_paused());
    assert_eq!(supervisor.pause_kind("nothing"), 0);

    other.cancel();
    release.store(true, Ordering::SeqCst);
    assert!(tasks::wait_done(&handle, Duration::from_secs(5)));
    assert!(tasks::wait_done(&other, Duration::from_secs(5)));
}
