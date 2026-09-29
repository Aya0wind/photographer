// android 适配入口；本轮不实现原生能力。
#[path = "../unsupported/hotplug.rs"]
mod fallback;
pub use fallback::*;
