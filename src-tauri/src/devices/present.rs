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
fn is_external_volume(drive: &str) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::{
        Foundation::CloseHandle,
        Storage::FileSystem::{
            CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        },
        System::{
            Ioctl::{IOCTL_STORAGE_GET_HOTPLUG_INFO, STORAGE_HOTPLUG_INFO},
            IO::DeviceIoControl,
        },
    };
    let path: Vec<u16> = format!("\\\\.\\{drive}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    // 零访问权限即可查询属性，不读取或修改介质内容。
    let Ok(handle) = (unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }) else {
        return false;
    };
    let mut info = STORAGE_HOTPLUG_INFO {
        Size: std::mem::size_of::<STORAGE_HOTPLUG_INFO>() as u32,
        ..Default::default()
    };
    let mut returned = 0;
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_GET_HOTPLUG_INFO,
            None,
            0,
            Some((&mut info as *mut STORAGE_HOTPLUG_INFO).cast()),
            info.Size,
            Some(&mut returned),
            None,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.is_ok()
        && returned >= std::mem::size_of::<STORAGE_HOTPLUG_INFO>() as u32
        && (info.MediaRemovable || info.DeviceHotplug)
}

/// 探测盘符是否应注册为设备：应注册返回 `Some(展示名)`（卷标，缺失回退
/// 盘符），否则 None。启动枚举与热插卷到达分支共用（单一事实源）。
#[cfg(windows)]
pub fn probe_volume(drive: &str) -> Option<String> {
    use windows::core::PCWSTR;

    use super::volume;

    let root: Vec<u16> = format!("{drive}\\")
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // SAFETY: root 为 NUL 结尾的合法宽字符串，在本调用内存活
    let drive_type =
        unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr())) };
    // 媒体探测：空卡槽/未就绪 GetVolumeInformationW 失败 → None（= 无媒体）
    let media = volume::drive_label(drive);
    if is_importable_volume(
        drive_type,
        media.is_some(),
        drive_type == DRIVE_FIXED && is_external_volume(drive),
    ) {
        Some(
            media
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| drive.to_string()),
        )
    } else {
        None
    }
}

/// 空读卡器仍显示在来源列表，但不能注册、扫描或触发相机连接提示。
#[cfg(windows)]
pub fn enumerate_empty_readers() -> Vec<String> {
    use windows::core::PCWSTR;
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    super::hotplug::unitmask_to_drives(mask)
        .into_iter()
        .filter(|drive| {
            let root: Vec<u16> = format!("{drive}\\").encode_utf16().chain(Some(0)).collect();
            let kind = unsafe {
                windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr()))
            };
            (kind == DRIVE_REMOVABLE || (kind == DRIVE_FIXED && is_external_volume(drive)))
                && super::volume::drive_label(drive).is_none()
        })
        .collect()
}

#[cfg(not(windows))]
pub fn enumerate_empty_readers() -> Vec<String> {
    Vec::new()
}

/// 非 Windows 桩（本产品仅面向 Windows）。
#[cfg(not(windows))]
#[allow(dead_code)]
pub fn probe_volume(_drive: &str) -> Option<String> {
    None
}

/// 存量可导入卷：`(盘符 "E:", 展示名)`。过滤决策经 [`probe_volume`]
/// （与热插到达共用）。
#[cfg(windows)]
pub fn enumerate_present_volumes() -> Vec<(String, String)> {
    // 掩码格式与 DBT dbcv_unitmask 相同（bit0='A'），复用热插解码
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    super::hotplug::unitmask_to_drives(mask)
        .into_iter()
        .filter_map(|drive| probe_volume(&drive).map(|label| (drive, label)))
        .collect()
}

/// 非 Windows 桩（本产品仅面向 Windows）。
#[cfg(not(windows))]
pub fn enumerate_present_volumes() -> Vec<(String, String)> {
    Vec::new()
}
