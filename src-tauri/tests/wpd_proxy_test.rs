//! WPD 代理化测试（统一架构）：worker 消息路径往返、worker 命令 panic
//! 捕获（进程存活、后续命令可用）、代理无 COM 资源可任意线程 drop。
//! 真机设备的实际 list/stream 由 M1 忽略用例 + 用户真机验证覆盖。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings, tasks, thumbs};

use devices::wpd::{self, WpdSource};
use devices::{DeviceSource, SourceKind};

#[test]
fn proxy_command_roundtrip_via_worker() {
    // 不存在的设备：消息经 worker 往返后返回设备错误（证明命令真实走
    // 了 worker 线程执行——错误发生在 open_device 而非消息层）
    let bogus = r"\\?\USB#VID_0000&PID_0000#NOSUCH#{6AC27878-A6FA-4155-BA85-F98F491D4F33}";
    let src = WpdSource::new(bogus, "测试代理");

    // id 规范化 + kind/name 透传（代理是纯数据）
    assert_eq!(src.id(), bogus.to_ascii_lowercase());
    assert_eq!(src.kind(), SourceKind::Mtp);
    assert_eq!(src.name(), "测试代理");

    let err = src.list().expect_err("不存在设备应报错");
    assert!(
        matches!(&err, devices::DeviceError::Other(m) if m.contains("WPD error") || m.contains("worker")),
        "应透传 worker 执行的设备错误: {err}"
    );

    // open_head / delete 同一消息路径
    assert!(src.open_head("oXXX", 64).is_err());
    assert!(src.delete("oXXX").is_err());
}

#[test]
fn worker_panic_is_captured_and_survives() {
    // 注入 panic：必须返回 Err（而非带崩进程）
    let first = wpd::panic_probe().expect_err("注入 panic 必须被捕获为 Err");
    assert!(
        matches!(&first, devices::DeviceError::Other(m) if m.contains("panic")),
        "panic 转错误回执: {first}"
    );

    // worker 存活：后续命令照常应答（仍是不存在设备 → 设备错误，非无响应）
    let bogus = r"\\?\USB#VID_0000&PID_0000#NOSUCH#{6AC27878-A6FA-4155-BA85-F98F491D4F33}";
    let err = WpdSource::new(bogus, "x").list().expect_err("应答到达");
    assert!(
        !matches!(&err, devices::DeviceError::Other(m) if m.contains("无响应") || m.contains("不可用")),
        "panic 后 worker 仍应服务: {err}"
    );

    // 再次注入 panic 仍被捕获（稳定性）
    assert!(wpd::panic_probe().is_err());
}

#[test]
fn enumerate_via_worker_responds_and_proxy_drop_is_safe() {
    // 枚举走 worker：无相机机器返回 Ok(空) 或设备错误均可——关键是不挂死、
    // 不带崩（本机若插着相机会返回真实列表）
    let _ = wpd::enumerate_mtp_devices();

    // 代理在任意线程 drop（无 COM 资源）：直接构造即弃，无线程亲和
    let bogus = r"\\?\USB#VID_054C&PID_0E0B#S#{6AC27878-A6FA-4155-BA85-F98F491D4F33}";
    let handles: Vec<_> = (0..4)
        .map(|_| {
            std::thread::spawn(move || {
                let src = WpdSource::new(bogus, "drop-test");
                let _ = src.id();
                drop(src); // 任意线程 drop 安全（纯数据代理）
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
}
