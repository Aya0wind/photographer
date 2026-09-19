//! 存量设备枚举与「设备」语义过滤（启动补扫 + 热插共用，单一事实源）。
//!
//! 背景（2026-09-18 两次实测反馈）：① app 重启窗口内插入的相机收不到
//! WM_DEVICECHANGE，仅靠热插事件驱动会永久漏注册——启动时需主动枚举
//! 存量设备，走与热插完全相同的 `DeviceArrived` → 建源 → 扫描 → 注册 →
//! `DeviceScanned` 链路（lib.rs `spawn_device_orchestrator` 在订阅总线后
//! 调 [`enumerate_present_devices`] 并发布）。② 映射网络盘（Y:/Z:）在
//! 会话/网络恢复时也会触发 DBT 卷到达事件，混进设备区——「设备」语义
//! 必须过滤：**仅有媒体的可移动介质（读卡器/U 盘/照片光盘）可注册**；
//! 本地硬盘（FIXED）与网络盘（REMOTE）不是导入源。
//!
//! 过滤决策拆成纯函数 [`is_registrable_volume`]（单测见
//! tests/device_present_test.rs）；Win32 探测封装为 [`probe_volume`]，
//! 启动枚举与热插 DBT_DEVTYP_VOLUME 到达分支共用（**只滤到达，不滤
//! 移除**——拔盘瞬间 GetDriveTypeW 已失效，移除事件必须照发才能清注册表）。
//! 文件系统树浏览（fs_list_dirs）不过滤：从 NAS 文件夹导入是合法场景。
//!
//! WPD 存量：`IPortableDeviceManager::GetDevices`（`wpd::enumerate_mtp_devices`，
//! COM 初始化由 wpd.rs 每入口自带且幂等，编排线程无需预初始化）。

// GetDriveTypeW 返回值（Win32 ABI 固定；windows crate 该组常量在未启用的
// feature 内，此处照抄数值——与 wpd.rs CLSID 同策略）。仅
// DRIVE_REMOVABLE/DRIVE_CDROM 进入 lib 判定，其余供测试矩阵与文档完整性。
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
    if is_registrable_volume(drive_type, media.is_some()) {
        Some(media.unwrap_or_else(|| drive.to_string()))
    } else {
        None
    }
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
