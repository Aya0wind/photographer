//! 热插拔检测（WM_DEVICECHANGE）：隐藏顶层窗口 + RegisterDeviceNotificationW，
//! 同时监听卷（DBT_DEVTYP_VOLUME）与 WPD 设备接口（GUID_DEVINTERFACE_WPD），
//! 独立线程泵消息并发布内部 DeviceTopologyChanged 信号。
//!
//! 卷使用顶层窗口广播；WPD 接口显式注册。
//! 窗口泵不做自动化测试（需真机插拔，手动验收）；纯函数有单测
//! （tests/devices_test.rs）。

/// DBT_DEVTYP_VOLUME 的 dbcv_unitmask → 盘符列表（bit0='A'，bit25='Z'）。
pub fn unitmask_to_drives(mask: u32) -> Vec<String> {
    (0..26u32)
        .filter(|&i| mask & (1 << i) != 0)
        .map(|i| format!("{}:", char::from(b'A' + i as u8)))
        .collect()
}

/// WPD PnP 路径 → 展示名：取 `#` 分段的最后一个非空段。
///
/// 例：`\\?\usb#vid_04a9&pid_31f4#serial#{6ac27878-...}` → `{6ac27878-...}`。
pub fn pnp_display_name(pnp_path: &str) -> String {
    pnp_path
        .split('#')
        .rev()
        .find(|seg| !seg.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| pnp_path.to_string())
}

#[cfg(windows)]
#[path = "../platform/windows/hotplug.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "../platform/macos/hotplug.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/hotplug.rs"]
mod native;
#[cfg(target_os = "android")]
#[path = "../platform/android/hotplug.rs"]
mod native;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "linux",
    target_os = "android"
)))]
#[path = "../platform/unsupported/hotplug.rs"]
mod native;

// 平台接缝 API：个别符号暂无消费方（脚手架期），保留导出
#[allow(unused_imports)]
pub use native::{spawn_hotplug_thread, stop};
