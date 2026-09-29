use crate::devices::present::{is_importable_volume, DRIVE_FIXED, DRIVE_REMOVABLE};
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
pub fn probe_volume(drive: &str) -> Option<String> {
    use windows::core::PCWSTR;

    use crate::devices::volume;

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
pub fn enumerate_empty_readers() -> crate::devices::DeviceResult<Vec<String>> {
    use windows::core::PCWSTR;
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    if mask == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(crate::devices::hotplug::unitmask_to_drives(mask)
        .into_iter()
        .filter(|drive| {
            let root: Vec<u16> = format!("{drive}\\").encode_utf16().chain(Some(0)).collect();
            let kind = unsafe {
                windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr()))
            };
            (kind == DRIVE_REMOVABLE || (kind == DRIVE_FIXED && is_external_volume(drive)))
                && crate::devices::volume::drive_label(drive).is_none()
        })
        .collect())
}

/// 存量可导入卷：`(盘符 "E:", 展示名)`。过滤决策经 [`probe_volume`]
/// （与热插到达共用）。
pub fn enumerate_present_volumes() -> crate::devices::DeviceResult<Vec<(String, String)>> {
    // 掩码格式与 DBT dbcv_unitmask 相同（bit0='A'），复用热插解码
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    if mask == 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(crate::devices::hotplug::unitmask_to_drives(mask)
        .into_iter()
        .filter_map(|drive| probe_volume(&drive).map(|label| (drive, label)))
        .collect())
}
