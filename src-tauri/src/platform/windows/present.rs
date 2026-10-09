use crate::devices::present::{is_importable_volume, DRIVE_FIXED, DRIVE_REMOVABLE};

/// USB 读卡器可能仅报告 USB，此时按未知来源处理，不误伤外置 SSD/U 盘。
pub fn is_storage_card(path: &std::path::Path) -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::{
        Foundation::CloseHandle,
        Storage::FileSystem::{
            BusTypeMmc, BusTypeSd, CreateFileW, GetVolumeNameForVolumeMountPointW,
            GetVolumePathNameW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
            OPEN_EXISTING,
        },
        System::{
            Ioctl::{
                PropertyStandardQuery, StorageDeviceProperty, IOCTL_STORAGE_QUERY_PROPERTY,
                STORAGE_DEVICE_DESCRIPTOR, STORAGE_PROPERTY_QUERY,
            },
            IO::DeviceIoControl,
        },
    };
    let input: Vec<u16> = path
        .as_os_str()
        .to_string_lossy()
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut root = [0u16; 1024];
    let mut volume = [0u16; 1024];
    // 查询真实卷根，使从存储卡的子文件夹导入也采用相同限制。
    if unsafe { GetVolumePathNameW(PCWSTR(input.as_ptr()), &mut root) }.is_err()
        || unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), &mut volume) }.is_err()
    {
        return false;
    }
    let length = volume
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(volume.len());
    if length == 0 {
        return false;
    }
    if volume[length - 1] == b'\\' as u16 {
        volume[length - 1] = 0;
    }
    let Ok(handle) = (unsafe {
        CreateFileW(
            PCWSTR(volume.as_ptr()),
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
    let query = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        ..Default::default()
    };
    let mut buffer = [0u8; 4096];
    let mut returned = 0;
    let result = unsafe {
        DeviceIoControl(
            handle,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some((&query as *const STORAGE_PROPERTY_QUERY).cast()),
            std::mem::size_of_val(&query) as u32,
            Some(buffer.as_mut_ptr().cast()),
            buffer.len() as u32,
            Some(&mut returned),
            None,
        )
    };
    unsafe {
        let _ = CloseHandle(handle);
    }
    if result.is_err() || (returned as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return false;
    }
    // 输出是字节缓冲，使用非对齐读取；长度由上面的返回字节数校验。
    let descriptor =
        unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<STORAGE_DEVICE_DESCRIPTOR>()) };
    descriptor.BusType == BusTypeSd || descriptor.BusType == BusTypeMmc
}

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
