//! 事件转发链路测试（P0 修复）：转发计数与活性、emit 侧 panic 捕获自恢复
//! （进程存活、后续事件继续转发、on_panic 上报）、event_ping 自检往返、
//! Probe 序列化契约。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings, tasks, thumbs};

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use events::{AppEvent, EventBus};

/// 等待条件成立（上限 5s）。
fn eventually(cond: impl Fn() -> bool) -> bool {
    let started = std::time::Instant::now();
    while !cond() {
        if started.elapsed() > Duration::from_secs(5) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn forwarder_delivers_all_events_to_sink() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let count = Arc::new(AtomicUsize::new(0));
    let sink_count = Arc::clone(&count);
    let emit: Arc<dyn Fn(&AppEvent) + Send + Sync> = Arc::new(move |_event| {
        sink_count.fetch_add(1, Ordering::SeqCst);
    });
    // 转发监督循环在后台线程（通道由测试持有 sender，不会 Closed）
    std::thread::spawn(move || {
        events::forward_supervised(&mut rx, emit, |_msg| {});
    });

    for i in 0..10 {
        bus.publish(AppEvent::DeviceRemoved {
            id: format!("E{i}"),
        });
    }
    assert!(
        eventually(|| count.load(Ordering::SeqCst) >= 10),
        "全部事件应送达 sink: {}",
        count.load(Ordering::SeqCst)
    );
}

#[test]
fn forwarder_recovers_from_emit_panic() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let delivered = Arc::new(AtomicUsize::new(0));
    let panics = Arc::new(AtomicUsize::new(0));
    let first = Arc::new(std::sync::atomic::AtomicBool::new(true));

    let d2 = Arc::clone(&delivered);
    let f2 = Arc::clone(&first);
    let emit: Arc<dyn Fn(&AppEvent) + Send + Sync> = Arc::new(move |_event| {
        if f2.swap(false, Ordering::SeqCst) {
            panic!("注入的 emit panic");
        }
        d2.fetch_add(1, Ordering::SeqCst);
    });

    let p2 = Arc::clone(&panics);
    let reported: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let r2 = Arc::clone(&reported);
    std::thread::spawn(move || {
        events::forward_supervised(&mut rx, emit, move |msg| {
            p2.fetch_add(1, Ordering::SeqCst);
            r2.lock().unwrap().push(msg);
        });
    });

    // 第一条触发 sink panic（该条丢失），随后自恢复
    bus.publish(AppEvent::DeviceRemoved { id: "first".into() });
    assert!(
        eventually(|| panics.load(Ordering::SeqCst) >= 1),
        "panic 应被上报"
    );
    // 后续事件正常转发（进程存活 + 循环重启）
    bus.publish(AppEvent::DeviceRemoved {
        id: "second".into(),
    });
    bus.publish(AppEvent::DeviceRemoved { id: "third".into() });
    assert!(
        eventually(|| delivered.load(Ordering::SeqCst) >= 2),
        "自恢复后事件继续转发: {}",
        delivered.load(Ordering::SeqCst)
    );
    let msgs = reported.lock().unwrap().clone();
    assert!(
        msgs.iter().any(|m| m.contains("自恢复")),
        "上报应含自恢复上下文: {msgs:?}"
    );
}

#[test]
fn event_ping_roundtrip_via_bus_and_probe_contract() {
    // 核心函数：发布 Probe{ts} 且返回同一 ts（订阅方应收到）
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let ts = ipc::event_ping(&bus);
    assert!(!ts.is_empty());
    match rx.blocking_recv().expect("应收到 Probe") {
        AppEvent::Probe { ts: got } => assert_eq!(got, ts),
        other => panic!("应为 Probe 事件: {other:?}"),
    }

    // 序列化契约：tag=probe（camelCase 扁平枚举）
    let value = serde_json::to_value(AppEvent::Probe { ts: "t1".into() }).unwrap();
    assert_eq!(value["type"], "probe");
    assert_eq!(value["ts"], "t1");
}
