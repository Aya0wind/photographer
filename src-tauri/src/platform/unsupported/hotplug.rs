//! 热插拔适配占位：不监视设备，退出接口可安全调用。
use crate::events::EventBus;
use std::thread::JoinHandle;
pub fn spawn_hotplug_thread(bus: EventBus) -> JoinHandle<()> {
    std::thread::spawn(move || drop(bus))
}
pub fn stop(handle: JoinHandle<()>) {
    let _ = handle.join();
}
