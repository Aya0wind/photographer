//! 存量设备枚举与「设备」语义过滤（启动补扫 + 热插共用，单一事实源）。
//!
//! 启动、轮询与热插拔共用卷探测。可移动介质须已插卡；CFexpress 等报告为
//! 固定磁盘的外接介质，还须通过硬件热插属性验证。本地硬盘和网络盘不注册。
//! 空读卡器仅供来源列表展示，不参与扫描；WPD 的盘符别名由卷通道管理。

// GetDriveTypeW 返回值（Win32 ABI 固定；windows crate 该组常量在未启用的
// feature 内，此处照抄数值——与 wpd.rs CLSID 同策略）。仅
// 可移动/光盘与经硬件属性验证的外接固定卷进入判定。
#[allow(dead_code)]
pub const DRIVE_UNKNOWN: u32 = 0;
#[allow(dead_code)]
pub const DRIVE_NO_ROOT_DIR: u32 = 1;
pub const DRIVE_REMOVABLE: u32 = 2;
#[allow(dead_code)]
pub const DRIVE_FIXED: u32 = 3;
#[allow(dead_code)]
pub const DRIVE_REMOTE: u32 = 4;
pub const DRIVE_CDROM: u32 = 5;
#[allow(dead_code)]
pub const DRIVE_RAMDISK: u32 = 6;

/// 卷是否应注册为导入「设备」：有媒体的可移动介质（读卡器/U 盘/照片光盘）。
///
/// 排除：本地硬盘（photoRoot 库目录常驻其上，不是设备）、网络盘（库目录/
/// NAS 映射，非导入源）、RAMDISK/未知类型、无媒体的空卡槽。
pub fn is_registrable_volume(drive_type: u32, media_present: bool) -> bool {
    media_present && matches!(drive_type, DRIVE_REMOVABLE | DRIVE_CDROM)
}

/// CFexpress 等外接介质也可能报告 DRIVE_FIXED；必须再查硬件热插属性。
pub fn is_importable_volume(drive_type: u32, media_present: bool, external: bool) -> bool {
    is_registrable_volume(drive_type, media_present)
        || (drive_type == DRIVE_FIXED && media_present && external)
}

#[cfg(windows)]
#[path = "../platform/windows/present.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "../platform/macos/present.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/present.rs"]
mod native;
#[cfg(target_os = "android")]
#[path = "../platform/android/present.rs"]
mod native;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "linux",
    target_os = "android"
)))]
#[path = "../platform/unsupported/present.rs"]
mod native;

pub use native::{enumerate_empty_readers, enumerate_present_volumes, probe_volume};
