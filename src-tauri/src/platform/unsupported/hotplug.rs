//! Unavailable watcher: no thread is created.
use crate::events::EventBus;
pub struct HotplugHandle;
pub fn spawn_hotplug_thread(_: EventBus) -> std::io::Result<HotplugHandle> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台尚未实现热插拔监视",
    ))
}
pub fn stop(_: HotplugHandle) {}
