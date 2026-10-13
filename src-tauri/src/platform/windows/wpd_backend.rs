//! WpdMtpBackend：WPD MTP 扩展透传联拍后端（Windows COM；非 Windows 桩）。
//!
//! 复用既有 WPD 栈（`crate::devices::wpd`：ComApartment / open_device /
//! worker 池——COM 全部在 worker 线程内创建、调用与释放，UI 线程零阻塞），
//! 不平行造轮子。透传机制：`IPortableDevice::SendCommand` +
//! `WPD_COMMAND_MTP_EXT_*`（类别 `WPD_CATEGORY_MTP_EXT_VENDOR_OPERATIONS`，
//! fmtid `4D545058-1A2E-4106-A357-771E0819FC56`；常量取自 windows crate
//! 收录的 SDK 头文件 PortableDevice.h / WpdMtpExtensions.h 数值）。
//!
//! 拍摄闭环：Advise 订阅 OBJECT_ADDED → 透传发 0x90C0/0x100E →
//! 回调线程只提取新对象 ID 唤醒等待 → worker 线程富化对象名/大小 →
//! Unadvise 清理（任何路径都执行）。超时文案「相机未响应拍摄命令」。

use std::time::Duration;

use super::backend::{
    apply_cached_capabilities, usb_vid_pid_from_pnp, CameraBackend, CameraInfo, CameraSetting,
    Capabilities, CapturedObject, LiveViewFrame, TetherError,
};

/// v1 唯一联拍后端：WPD 文件通道（L0）+ MTP 透传（L1）合一。
pub struct WpdMtpBackend;

impl WpdMtpBackend {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WpdMtpBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// 后端标识（注册表键）。
#[allow(dead_code)] // v1 契约面（trait id() 调用点待 UI/L3 接线）
const BACKEND_ID: &str = "wpd-mtp";
/// 后端展示名。
#[allow(dead_code)] // 同上
const BACKEND_NAME: &str = "WPD MTP（Windows 便携设备）";

// ---------------------------------------------------------------------------
// 探测缓存（进程级会话态，不入库——联拍会话瞬态）
// ---------------------------------------------------------------------------

mod cache {
    use super::super::backend::{CameraInfo, Capabilities};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::{Mutex, OnceLock};

    fn map() -> &'static Mutex<HashMap<String, CameraInfo>> {
        static CACHE: OnceLock<Mutex<HashMap<String, CameraInfo>>> = OnceLock::new();
        CACHE.get_or_init(|| Mutex::new(HashMap::new()))
    }

    /// 写入/覆盖探测结果（key = 规范化设备 id）。
    pub(super) fn insert(info: CameraInfo) {
        let key = crate::devices::normalize_device_id(&info.pnp_id);
        map()
            .lock()
            .expect("tethering cache mutex poisoned")
            .insert(key, info);
    }

    /// 移除设备缓存（disconnect；v1 lib 内调用点待会话管理接线）。
    #[allow(dead_code)]
    pub(super) fn remove(pnp: &str) {
        map()
            .lock()
            .expect("tethering cache mutex poisoned")
            .remove(&crate::devices::normalize_device_id(pnp));
    }

    /// 缓存快照（枚举合并用）。
    pub(super) fn snapshot() -> BTreeMap<String, Capabilities> {
        map()
            .lock()
            .expect("tethering cache mutex poisoned")
            .iter()
            .map(|(k, v)| (k.clone(), v.capabilities))
            .collect()
    }

    /// 探测得到的机身序列号（枚举时物理身份合并用；未探测 None）。
    pub(super) fn identity(pnp: &str) -> Option<String> {
        map()
            .lock()
            .expect("tethering cache mutex poisoned")
            .get(&crate::devices::normalize_device_id(pnp))
            .and_then(|info| info.serial.clone())
    }
}

impl CameraBackend for WpdMtpBackend {
    fn id(&self) -> &'static str {
        BACKEND_ID
    }

    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    /// 类型上限（设备级以探测为准）：文件通道 + 两种拍摄码 + OBJECT_ADDED
    /// 均为 WPD 通道潜在能力；实时取景按尼康原生操作集探测。
    fn capabilities(&self) -> Capabilities {
        Capabilities::FILE_TRANSFER
            .union(Capabilities::STANDARD_CAPTURE)
            .union(Capabilities::VENDOR_CAPTURE_NIKON)
            .union(Capabilities::OBJECT_ADDED_EVENTS)
            .union(Capabilities::LIVE_VIEW)
    }

    /// 枚举 WPD 设备（复用既有 worker 枚举，不逐台 Open）；已探测设备
    /// 以缓存能力位填充，未探测设备只给默认位（fileTransfer）。物理身份
    /// （vid/pid）从设备路径解析；serial 只有 connect 探测后才填充（缓存
    /// 命中时带出）。
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        let devices = crate::devices::wpd::enumerate_mtp_devices().map_err(TetherError::from)?;
        let infos = devices
            .into_iter()
            .map(|(pnp, name)| {
                let (vid, pid) = usb_vid_pid_from_pnp(&pnp);
                let serial = cache::identity(&pnp);
                CameraInfo {
                    pnp_id: pnp,
                    name,
                    capabilities: Capabilities::FILE_TRANSFER,
                    serial,
                    vid,
                    pid,
                }
            })
            .collect();
        Ok(apply_cached_capabilities(infos, &cache::snapshot()))
    }

    /// 打开会话 + 全量探测（worker 线程执行；probe 通道与数据通道分离）。
    fn probe(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        let id = pnp_id.to_string();
        let info = crate::devices::wpd::schedule_com_operation(
            &format!("probe:{id}"),
            Duration::from_secs(10),
            move || com::probe_device(&id),
        )
        .map_err(TetherError::from)?;
        cache::insert(info.clone());
        Ok(info)
    }
    fn connect(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        let id = pnp_id.to_string();
        let info = crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::native::connect(&id),
        )
        .map_err(TetherError::from)?;
        cache::insert(info.clone());
        Ok(info)
    }
    fn trigger_capture(&self, pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        let id = pnp_id.to_string();
        crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(30),
            move || com::native::capture(&id, Duration::from_secs(20)),
        )
        .map_err(Into::into)
    }
    fn poll_objects(&self, pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        let id = pnp_id.to_string();
        // 静音：空轮不产生 COM 读，唯一开销就是通道往返与日志——200ms 一轮
        // 的 start/finish 诊断在长会话里只稀释真正的异常信号。
        crate::devices::wpd::schedule_com_operation_quiet(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::native::poll(&id),
        )
        .map_err(Into::into)
    }
    /// 崩溃/重连恢复：按对象 id 校验仍在设备上并重新入收片队列（§B）。
    fn request_objects(&self, pnp_id: &str, ids: &[String]) -> Result<Vec<String>, TetherError> {
        let id = pnp_id.to_string();
        let ids = ids.to_vec();
        crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::native::request_objects(&id, &ids),
        )
        .map_err(Into::into)
    }
    /// 参数事件序号（设备属性变更事件在回调里只递增此值，不读相机）。
    fn settings_revision(&self, pnp_id: &str) -> u64 {
        com::native::settings_revision(pnp_id)
    }
    fn acknowledge_captured(
        &self,
        pnp_id: &str,
        object: &CapturedObject,
    ) -> Result<(), TetherError> {
        com::native::acknowledge(pnp_id, &object.object_id);
        Ok(())
    }
    fn live_view_frame(&self, pnp_id: &str) -> Result<LiveViewFrame, TetherError> {
        let id = pnp_id.to_string();
        // 静音：30-60fps 下 start/finish 双记录 = 每秒上百条诊断（chrono
        // 格式化 + eprintln），是实测可见的帧延迟。
        crate::devices::wpd::schedule_com_operation_quiet(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::native::frame(&id),
        )
        .map_err(Into::into)
    }
    /// 待接收对象数（进程级 FIFO 直读，纯锁访问无需 COM/worker 往返）。
    fn pending_receipts(&self, pnp_id: &str) -> usize {
        com::native::pending_receipts(pnp_id)
    }

    fn settings(&self, pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
        let id = pnp_id.to_string();
        crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || {
                // 原生连接优先：复用已打开的设备 + 缓存操作码表（取景期间
                // 属性变更频繁刷新，重开设备/重读 DeviceInfo 是通道占用
                // 大头）；无原生连接（临时会话）回退独立连接整读。
                match com::native::settings(&id) {
                    Ok(values) => Ok(values),
                    Err(crate::devices::DeviceError::Disconnected) => com::control::settings(&id),
                    Err(error) => Err(error),
                }
            },
        )
        .map_err(Into::into)
    }

    fn set_setting(&self, pnp_id: &str, setting: &str, value: &str) -> Result<(), TetherError> {
        let id = pnp_id.to_string();
        let setting = setting.to_string();
        let value = value.to_string();
        crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::control::set_setting(&id, &setting, &value),
        )
        .map_err(Into::into)
    }

    fn focus_at(&self, pnp_id: &str, x: f64, y: f64) -> Result<(), TetherError> {
        let id = pnp_id.to_string();
        crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || {
                match com::native::focus_at(&id, x, y) {
                    Ok(()) => Ok(()),
                    // 无原生连接（未开取景/临时会话）→ 独立连接整帧 AF 驱动
                    Err(crate::devices::DeviceError::Disconnected) => com::control::focus_at(&id, x, y),
                    Err(error) => Err(error),
                }
            },
        )
        .map_err(Into::into)
    }

    /// 断开：丢弃会话态缓存并隔离旧连接（复用既有 invalidate）。
    fn disconnect(&self, pnp_id: &str) {
        let id = pnp_id.to_string();
        if let Err(error) = crate::devices::wpd::schedule_com_operation(
            &format!("data:{id}"),
            Duration::from_secs(10),
            move || com::native::close(&id),
        ) {
            crate::devices::diagnostics::record(format!(
                "Native camera cleanup failed; retaining worker: {error}"
            ));
            return;
        }
        cache::remove(pnp_id);
        crate::devices::wpd::invalidate_device(pnp_id);
    }

    /// 拍摄（worker 线程执行，与导入共享 data: 通道 → WPD 会话互斥；
    /// 池超时 = 用户超时 + 30s 余量，避免等待期间被通道隔离）。
    fn capture_still(
        &self,
        pnp_id: &str,
        timeout: Duration,
    ) -> Result<CapturedObject, TetherError> {
        {
            let pnp = pnp_id.to_string();
            let pool_timeout = timeout + Duration::from_secs(30);
            match crate::devices::wpd::schedule_com_operation(
                &format!("data:{pnp}"),
                pool_timeout,
                move || com::native::capture_once(&pnp, timeout),
            ) {
                // 拍摄业务结果（含 Timeout/CaptureNotSupported 等）
                Ok(object) => Ok(object),
                // 调度/COM 层失败（会话打不开、拔线等）
                Err(device_err) => Err(TetherError::from(device_err)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows COM 实现（全部调用带 SAFETY 注释；对象生命周期归宿 worker 线程）
// ---------------------------------------------------------------------------

pub(crate) fn fast_media_counts(id: &str) -> crate::devices::DeviceResult<Option<std::collections::BTreeMap<crate::events::AssetKind, u64>>> {
    let id = id.to_owned();
    crate::devices::wpd::schedule_com_operation(&format!("data:{id}"), std::time::Duration::from_secs(10), move || com::fast_media_counts(&id))
}

mod com {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use windows::core::{implement, Ref};
    use windows::Win32::Devices::PortableDevices::{
        IPortableDevice, IPortableDeviceEventCallback, IPortableDeviceEventCallback_Impl,
        IPortableDeviceKeyCollection, IPortableDevicePropVariantCollection, IPortableDeviceValues,
        WPD_COMMAND_MTP_EXT_END_DATA_TRANSFER,
        WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE,
        WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_READ, WPD_COMMAND_MTP_EXT_READ_DATA,
        WPD_EVENT_DEVICE_CAPABILITIES_UPDATED, WPD_EVENT_NOTIFICATION, WPD_EVENT_OBJECT_ADDED,
        WPD_EVENT_PARAMETER_EVENT_ID, WPD_OBJECT_ID, WPD_OBJECT_NAME,
        WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_OBJECT_SIZE, WPD_PROPERTY_COMMON_COMMAND_CATEGORY,
        WPD_PROPERTY_COMMON_COMMAND_ID, WPD_PROPERTY_COMMON_HRESULT,
        WPD_PROPERTY_MTP_EXT_OPERATION_CODE, WPD_PROPERTY_MTP_EXT_OPERATION_PARAMS,
        WPD_PROPERTY_MTP_EXT_OPTIMAL_TRANSFER_BUFFER_SIZE, WPD_PROPERTY_MTP_EXT_RESPONSE_CODE,
        WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT, WPD_PROPERTY_MTP_EXT_TRANSFER_DATA,
        WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_READ,
        WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_READ,
        WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE,
    };
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VT_UI4;

    use crate::devices::wpd::com as wpd_com;
    use crate::devices::{DeviceError, DeviceResult};

    use super::super::backend::{CameraInfo, CapturedObject};
    use super::super::mtp;
    use super::super::ptp_control;
    use super::super::vendor_dispatch;

    pub(super) mod native {
        include!("wpd_native.rs");
    }

    pub(super) mod control {
        include!("wpd_control.rs");
    }

    /// 默认读块（驱动建议值缺省时的回退）。
    const READ_CHUNK_FALLBACK: u32 = 64 * 1024;
    /// 单块读取上限（大读取按 limit 放宽后的封顶，见 [`chunk_cap`]）。
    const MAX_READ_CHUNK: u32 = 1024 * 1024;
    /// DeviceInfo 数据集大小上限（正常 ≤ 数 KB；防御异常设备）。
    const DEVICE_INFO_MAX_BYTES: usize = 256 * 1024;

    /// 读块上限按读取自身的 limit 放宽：小读取（描述符 64KB / DeviceInfo
    /// 256KB）保持原钳制；取景帧一类的 16MB 上限读取可到 1MB 块——
    /// 驱动建议的最优块（常 ≥1MB）不再被 64KB 钳死。
    fn chunk_cap(limit: usize) -> u32 {
        limit
            .clamp(READ_CHUNK_FALLBACK as usize, MAX_READ_CHUNK as usize)
            as u32
    }

    // -----------------------------------------------------------------------
    // MTP 透传参数组装 + SendCommand
    // -----------------------------------------------------------------------

    /// 构造 VT_UI4 PROPVARIANT（值拷贝无堆内存；与 wpd.rs lpstr_propvariant
    /// 同款 union 写法）。
    fn u32_propvariant(value: u32) -> PROPVARIANT {
        let mut prop = PROPVARIANT::default();
        // SAFETY: 写本地零初始化 PROPVARIANT：vt 为具名字段；ulVal 为 union
        // 成员且随后以 VT_UI4 解释，二者一致。外层 Anonymous 是
        // ManuallyDrop<PROPVARIANT_0_0>，写入须显式 DerefMut。
        unsafe {
            let inner = &mut *prop.Anonymous.Anonymous;
            inner.vt = VT_UI4;
            inner.Anonymous.ulVal = value;
        }
        prop
    }

    /// 组装透传命令的公共头部：命令类别 + 命令 ID（SendCommand 靠这两键
    /// 识别要执行的 WPD 命令——每个透传命令都必须携带）。
    fn base_command_values(command_key: *const PROPERTYKEY) -> DeviceResult<IPortableDeviceValues> {
        // SAFETY: CLSID 为静态常量；无外部聚合
        let values: IPortableDeviceValues = unsafe {
            CoCreateInstance(
                &wpd_com::CLSID_PORTABLE_DEVICE_VALUES,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(wpd_com::win_error)?;
        // SAFETY: command_key 指向模块级静态常量
        unsafe {
            values.SetGuidValue(&WPD_PROPERTY_COMMON_COMMAND_CATEGORY, &(*command_key).fmtid)
        }
        .map_err(wpd_com::win_error)?;
        // SAFETY: 键为静态常量
        unsafe {
            values.SetUnsignedIntegerValue(&WPD_PROPERTY_COMMON_COMMAND_ID, (*command_key).pid)
        }
        .map_err(wpd_com::win_error)?;
        Ok(values)
    }

    /// 组装 EXECUTE_COMMAND_* 参数集合：命令键 + MTP 操作码 + 参数表。
    fn build_command_values(
        command_key: *const PROPERTYKEY,
        code: u16,
        params: &[u32],
    ) -> DeviceResult<IPortableDeviceValues> {
        let values = base_command_values(command_key)?;
        // SAFETY: 键为静态常量
        unsafe {
            values.SetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_OPERATION_CODE, code as u32)
        }
        .map_err(wpd_com::win_error)?;

        // 参数表（VT_UI4 × N；驱动要求键恒存在，空命令也传空集合）
        // SAFETY: CLSID 为静态常量；无外部聚合
        let collection: IPortableDevicePropVariantCollection = unsafe {
            CoCreateInstance(
                &wpd_com::CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(wpd_com::win_error)?;
        for &value in params {
            let mut prop = u32_propvariant(value);
            // SAFETY: prop 为本地合法 VT_UI4 PROPVARIANT；Add 拷贝值入集合
            let added = unsafe { collection.Add(&prop) }.is_ok();
            // SAFETY: VT_UI4 无堆内存；Clear 维持 API 契约（无操作）
            let _ = unsafe { PropVariantClear(&mut prop) };
            if !added {
                return Err(DeviceError::Other("构造 MTP 参数集合失败".into()));
            }
        }
        // SAFETY: 键为静态常量
        unsafe { values.SetIUnknownValue(&WPD_PROPERTY_MTP_EXT_OPERATION_PARAMS, &collection) }
            .map_err(wpd_com::win_error)?;
        Ok(values)
    }

    /// SendCommand（dwflags=0，官方透传示例同款）。失败 HRESULT 即设备级
    /// 错误（拔线/驱动拒绝）。
    fn send_command(
        device: &IPortableDevice,
        command_key: *const PROPERTYKEY,
        code: u16,
        params: &[u32],
    ) -> DeviceResult<IPortableDeviceValues> {
        let values = build_command_values(command_key, code, params)?;
        // SAFETY: 设备已 Open；values 为本地合法参数集合
        send_values(device, &values)
    }

    fn send_values(
        device: &IPortableDevice,
        values: &IPortableDeviceValues,
    ) -> DeviceResult<IPortableDeviceValues> {
        // SendCommand can succeed while the command result carries a failure HRESULT.
        // SAFETY: device is open and values owns the complete command payload.
        let results = unsafe { device.SendCommand(0, values) }.map_err(wpd_com::win_error)?;
        // SAFETY: static property key; results lives through the read.
        if let Ok(status) = unsafe { results.GetErrorValue(&WPD_PROPERTY_COMMON_HRESULT) } {
            status.ok().map_err(wpd_com::win_error)?;
        }
        Ok(results)
    }

    /// 读取响应码（驱动成功送达命令时必含；缺失视为透传层异常）。
    fn response_code(results: &IPortableDeviceValues) -> DeviceResult<u32> {
        // SAFETY: 键为静态常量
        unsafe { results.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_RESPONSE_CODE) }
            .map_err(|_| DeviceError::Other("MTP 透传响应缺少响应码".into()))
    }

    /// 无数据相位命令（拍摄码 0x90C0/0x100E 用）。返回 PTP 响应码。
    fn execute_without_data(
        device: &IPortableDevice,
        code: u16,
        params: &[u32],
    ) -> DeviceResult<u32> {
        let results = send_command(
            device,
            &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE,
            code,
            params,
        )?;
        response_code(&results)
    }

    /// 数据相位读取命令（GetDeviceInfo 用）。返回 (PTP 响应码, 数据集)。
    fn execute_with_data_to_read(
        device: &IPortableDevice,
        code: u16,
        params: &[u32],
    ) -> DeviceResult<(u32, Vec<u8>)> {
        execute_with_data_limit(device, code, params, DEVICE_INFO_MAX_BYTES)
    }

    fn execute_with_data_limit(
        device: &IPortableDevice,
        code: u16,
        params: &[u32],
        limit: usize,
    ) -> DeviceResult<(u32, Vec<u8>)> {
        let results = send_command(
            device,
            &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_READ,
            code,
            params,
        )?;
        if let Ok(response) = response_code(&results) {
            if response != mtp::PTP_RESP_OK {
                return Ok((response, Vec::new()));
            }
        }
        // 传输上下文（驱动分配的字符串句柄；后续 READ_DATA/END 均携带）
        // SAFETY: 键为静态常量
        let context = unsafe { results.GetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT) }
            .map_err(wpd_com::win_error)
            .and_then(|p| {
                // SAFETY: GetStringValue 出参为 NUL 结尾宽字符串
                let s = unsafe { wpd_com::pwstr_to_string(p) };
                // SAFETY: 出参缓冲由 API 以 CoTaskMem 分配
                unsafe { wpd_com::free_pwstr(p) };
                if s.is_empty() {
                    Err(DeviceError::Other("MTP 透传缺少传输上下文".into()))
                } else {
                    Ok(s)
                }
            })?;
        // 驱动建议块大小（尽力而为；缺省 64KB）。上限随读取自身的 limit
        // 放宽：取景帧读取（16MB 上限）一帧 500KB+ 按 64KB 分块要 8-9 次
        // COM 往返（每次含目标缓冲双重拷贝）——大读取允许到 1MB 块。
        // SAFETY: 键为静态常量
        let chunk = unsafe {
            results.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_OPTIMAL_TRANSFER_BUFFER_SIZE)
        }
        .unwrap_or(READ_CHUNK_FALLBACK)
        .clamp(1024, chunk_cap(limit)) as usize;

        // The begin response supplies a context and size, not the PTP response code.
        let total = unsafe {
            results.GetUnsignedLargeIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE)
        }
        .ok();
        let data = read_payload_limit(total, chunk, limit, |size| {
            read_data_chunk(device, &context, size)
        });
        // Always close the transfer, including read/size failures, to release the camera.
        let end = end_transfer(device, &context);
        let data = data?;
        let response = response_code(&end?)?;
        Ok((response, data))
    }

    fn end_transfer(
        device: &IPortableDevice,
        context: &str,
    ) -> DeviceResult<IPortableDeviceValues> {
        let values = base_command_values(&WPD_COMMAND_MTP_EXT_END_DATA_TRANSFER)?;
        let wide = wpd_com::to_wide(context);
        // SAFETY: static property key; the NUL-terminated string lives through SendCommand.
        unsafe {
            values.SetStringValue(
                &WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT,
                windows::core::PCWSTR(wide.as_ptr()),
            )
        }
        .map_err(wpd_com::win_error)?;
        send_values(device, &values)
    }

    /// DeviceInfo 默认上限便捷包装（传输纯函数单测形态）。
    #[cfg(test)]
    fn read_payload(
        total: Option<u64>,
        chunk: usize,
        read: impl FnMut(usize) -> DeviceResult<Vec<u8>>,
    ) -> DeviceResult<Vec<u8>> {
        read_payload_limit(total, chunk, DEVICE_INFO_MAX_BYTES, read)
    }

    fn read_payload_limit(
        total: Option<u64>,
        chunk: usize,
        limit: usize,
        mut read: impl FnMut(usize) -> DeviceResult<Vec<u8>>,
    ) -> DeviceResult<Vec<u8>> {
        let known = total.filter(|n| *n != u32::MAX as u64 && *n != u64::MAX);
        if known.is_some_and(|n| n > limit as u64) {
            return Err(DeviceError::Other("MTP DeviceInfo 数据集异常超限".into()));
        }
        let mut data = Vec::new();
        loop {
            if known == Some(data.len() as u64) {
                return Ok(data);
            }
            let requested = known
                .map(|n| (n as usize - data.len()).min(chunk))
                .unwrap_or(chunk);
            let block = read(requested)?;
            if block.len() > requested {
                return Err(DeviceError::Other("MTP 返回的数据超过请求大小".into()));
            }
            if block.is_empty() && known.is_some() {
                return Err(DeviceError::Other("MTP DeviceInfo 数据集未完整传输".into()));
            }
            let short = block.len() < requested;
            data.extend_from_slice(&block);
            if data.len() > limit {
                return Err(DeviceError::Other("MTP DeviceInfo 数据集异常超限".into()));
            }
            if known.is_none() && short {
                return Ok(data);
            }
        }
    }

    /// READ_DATA 单块：携带上下文 + 请求字节数，返回数据块。
    fn read_data_chunk(
        device: &IPortableDevice,
        context: &str,
        chunk: usize,
    ) -> DeviceResult<Vec<u8>> {
        let values = base_command_values(&WPD_COMMAND_MTP_EXT_READ_DATA)?;
        let wide = wpd_com::to_wide(context);
        // SAFETY: 键为静态常量；wide 以 NUL 结尾且在本调用内存活
        unsafe {
            values.SetStringValue(
                &WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT,
                windows::core::PCWSTR(wide.as_ptr()),
            )
        }
        .map_err(wpd_com::win_error)?;
        // SAFETY: 键为静态常量
        unsafe {
            values.SetUnsignedIntegerValue(
                &WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_READ,
                chunk as u32,
            )
        }
        .map_err(wpd_com::win_error)?;
        // READ_DATA requires the host to provide the destination buffer.
        let buffer = vec![0u8; chunk];
        // SAFETY: SetBufferValue copies this buffer into the values collection.
        unsafe { values.SetBufferValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, &buffer) }
            .map_err(wpd_com::win_error)?;
        // SAFETY: 设备已 Open
        let results = send_values(device, &values)?;
        // SAFETY: 键为静态常量
        let got = unsafe {
            results.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_READ)
        }
        .map_err(wpd_com::win_error)?;
        if got == 0 {
            return Ok(Vec::new());
        }
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut count: u32 = 0;
        // SAFETY: 出参指针指向本地变量
        unsafe {
            results.GetBufferValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, &mut ptr, &mut count)
        }
        .map_err(wpd_com::win_error)?;
        if ptr.is_null() || count < got || got as usize > chunk {
            if !ptr.is_null() {
                unsafe { CoTaskMemFree(Some(ptr as *const core::ffi::c_void)) };
            }
            return Err(DeviceError::Other("MTP 返回的缓冲区长度无效".into()));
        }
        // SAFETY: ptr 指向驱动分配的 count 字节缓冲（本块生命周期至下方 free）
        let chunk_data = unsafe { std::slice::from_raw_parts(ptr, got as usize) }.to_vec();
        // SAFETY: GetBufferValue 缓冲由 API 以 CoTaskMem 分配
        unsafe { CoTaskMemFree(Some(ptr as *const core::ffi::c_void)) };
        Ok(chunk_data)
    }

    // -----------------------------------------------------------------------
    // 能力探测（camera_probe / capture 前置）
    // -----------------------------------------------------------------------

    pub(super) fn fast_media_counts(id: &str) -> DeviceResult<Option<std::collections::BTreeMap<crate::events::AssetKind, u64>>> {
        use windows::core::Interface;
        use windows::Win32::Devices::PortableDevices::WPD_PROPERTY_MTP_EXT_RESPONSE_PARAMS;
        let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
        let device = wpd_com::open_device(id)?;
        let (code, bytes) = execute_with_data_to_read(&device, 0x1001, &[])?;
        if code != 0x2001 { return Ok(None); }
        let info = mtp::parse_device_info(&bytes).map_err(|error| DeviceError::Other(error.to_string()))?;
        // Sony's ARW format was verified on hardware. Other vendors keep the
        // general enumerator until their raw format/count contract is verified.
        if !info.manufacturer.to_ascii_lowercase().contains("sony") || !info.operations_supported.contains(&0x1006) { return Ok(None); }
        let count = |format: u32| -> DeviceResult<Option<u64>> {
            let results = send_command(&device, &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE, 0x1006, &[u32::MAX, format, 0])?;
            if response_code(&results)? != 0x2001 { return Ok(None); }
            let values = unsafe { results.GetIUnknownValue(&WPD_PROPERTY_MTP_EXT_RESPONSE_PARAMS) }.map_err(wpd_com::win_error)?.cast::<IPortableDevicePropVariantCollection>().map_err(wpd_com::win_error)?;
            let mut value = PROPVARIANT::default();
            unsafe { values.GetAt(0, &mut value as *mut _ as *const _) }.map_err(wpd_com::win_error)?;
            let number = unsafe { if value.Anonymous.Anonymous.vt == VT_UI4 { Some(value.Anonymous.Anonymous.Anonymous.ulVal as u64) } else { None } };
            let _ = unsafe { PropVariantClear(&mut value) };
            Ok(number)
        };
        let started = Instant::now();
        let (Some(all), Some(folders), Some(jpg), Some(raw)) = (count(0)?, count(0x3001)?, count(0x3801)?, count(0xb101)?) else { return Ok(None); };
        if all.checked_sub(folders) != jpg.checked_add(raw) { return Ok(None); }
        crate::devices::diagnostics::record(format!("WPD fast count: jpg={jpg} raw={raw}; elapsed={:?}", started.elapsed()));
        Ok(Some(std::collections::BTreeMap::from([(crate::events::AssetKind::Photo, jpg), (crate::events::AssetKind::Raw, raw)])))
    }

    /// 打开会话 + 全量探测：MTP GetDeviceInfo（透传）解析
    /// OperationsSupported/EventsSupported，WPD GetSupportedEvents 交叉验证
    /// OBJECT_ADDED。在 worker 线程执行。
    pub fn probe_device(pnp_id: &str) -> DeviceResult<CameraInfo> {
        let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
        let device = wpd_com::open_device(pnp_id)?;

        let get_info = mtp::get_device_info_command();
        let (resp, data) = execute_with_data_to_read(&device, get_info.code, &get_info.params)?;
        if resp != mtp::PTP_RESP_OK {
            return Err(DeviceError::Other(format!(
                "GetDeviceInfo 被拒绝（MTP 响应 0x{resp:04X}）"
            )));
        }
        let parsed =
            mtp::parse_device_info(&data).map_err(|e| DeviceError::Other(e.to_string()))?;
        let brand = ptp_control::detect_brand(
            &parsed.manufacturer,
            &parsed.model,
            &parsed.operations_supported,
        );
        // OpenSession 支持情况仅记录（WPD 驱动自管 PTP 会话，无需手动开）
        crate::devices::diagnostics::record(format!(
            "MTP 探测 {}: vendor={:?} standard=v{} vendor_ext=0x{:08X} ops={} 事件={} OpenSession 支持={}",
            crate::devices::normalize_device_id(pnp_id),
            brand,
            parsed.standard_version,
            parsed.vendor_extension_id,
            parsed.operations_supported.len(),
            parsed.events_supported.len(),
            parsed
                .operations_supported
                .contains(&mtp::PTP_OP_OPEN_SESSION),
        ));
        let wpd_object_added = supported_events_contains_object_added(&device)?;
        let mut caps = mtp::capabilities_from_probe(
            &parsed.operations_supported,
            &parsed.events_supported,
            wpd_object_added,
        );
        // LiveView 能力判定集中在 vendor_dispatch：Nikon 且声明完整
        // 9201/9202/9203 才启用（品牌来自 Manufacturer，不用 PnP VID 猜——
        // Sony 与 Nikon 复用同一数值区间且含义不同）。
        let vendor = vendor_dispatch::profile(brand);
        // Sony 等厂商的扩展操作码藏在遥控握手之后：能力判定前先展开操作码
        // 表（幂等握手）；失败退回原始表，按保守能力报告，不阻断连接。
        let mut session_transport = control::WpdTransport {
            device: device.clone(),
        };
        let operations = match vendor.expand_operations(
            &mut session_transport,
            &parsed.operations_supported,
        ) {
            Ok(operations) => operations,
            Err(error) => {
                crate::devices::diagnostics::record(format!(
                    "Probe vendor expansion failed; using raw operations: {error}"
                ));
                parsed.operations_supported.clone()
            }
        };
        if vendor.live_view_supported(&operations) {
            caps = caps.union(super::super::backend::Capabilities::LIVE_VIEW);
        }
        let (vid, pid) = super::super::backend::usb_vid_pid_from_pnp(pnp_id);
        Ok(CameraInfo {
            pnp_id: pnp_id.to_string(),
            name: device_display_name(&device)
                .unwrap_or_else(|_| crate::devices::hotplug::pnp_display_name(pnp_id)),
            capabilities: caps,
            serial: device_serial_number(&device),
            vid,
            pid,
        })
    }

    /// 机身序列号（WPD 设备属性；只读探测，缺省/不可读 → None）。
    fn device_serial_number(device: &IPortableDevice) -> Option<String> {
        use windows::Win32::Devices::PortableDevices::{
            WPD_DEVICE_OBJECT_ID, WPD_DEVICE_SERIAL_NUMBER,
        };
        // SAFETY: the open device owns these COM interfaces; the root id is static.
        let content = unsafe { device.Content() }.ok()?;
        let properties = unsafe { content.Properties() }.ok()?;
        // SAFETY: read-only property query on the open device.
        let values = unsafe { properties.GetValues(WPD_DEVICE_OBJECT_ID, None) }.ok()?;
        wpd_com::string_prop(&values, &WPD_DEVICE_SERIAL_NUMBER)
            .map(|serial| serial.trim().to_string())
            .filter(|serial| !serial.is_empty())
    }

    fn device_display_name(device: &IPortableDevice) -> DeviceResult<String> {
        use windows::Win32::Devices::PortableDevices::{
            WPD_DEVICE_FRIENDLY_NAME, WPD_DEVICE_MODEL, WPD_DEVICE_OBJECT_ID,
        };
        // SAFETY: the open device owns these COM interfaces; the root id is static.
        let content = unsafe { device.Content() }.map_err(wpd_com::win_error)?;
        let properties = unsafe { content.Properties() }.map_err(wpd_com::win_error)?;
        let values = unsafe { properties.GetValues(WPD_DEVICE_OBJECT_ID, None) }
            .map_err(wpd_com::win_error)?;
        wpd_com::string_prop(&values, &WPD_DEVICE_FRIENDLY_NAME)
            .or_else(|| wpd_com::string_prop(&values, &WPD_DEVICE_MODEL))
            .filter(|name| !name.is_empty())
            .ok_or_else(|| DeviceError::Other("相机缺少名称".into()))
    }

    /// WPD 能力查询：GetSupportedEvents 是否含 WPD_EVENT_OBJECT_ADDED
    /// （wpd_probe 同款写法）。
    fn supported_events_contains_object_added(device: &IPortableDevice) -> DeviceResult<bool> {
        // SAFETY: 设备已 Open；Capabilities 为只读查询
        let capabilities = unsafe { device.Capabilities() }.map_err(wpd_com::win_error)?;
        // SAFETY: 只读查询
        let events = unsafe { capabilities.GetSupportedEvents() }.map_err(wpd_com::win_error)?;
        let mut count = 0u32;
        // SAFETY: 出参指针（crate 元数据声明为 *const，API 实际写入）
        unsafe { events.GetCount(&mut count as *mut u32 as *const u32) }
            .map_err(wpd_com::win_error)?;
        let mut found = false;
        for index in 0..count {
            let mut prop = PROPVARIANT::default();
            // SAFETY: 索引在范围内；出参为零初始化 PROPVARIANT（*const 出参同上）
            unsafe { events.GetAt(index, &mut prop as *mut PROPVARIANT as *const PROPVARIANT) }
                .map_err(wpd_com::win_error)?;
            // SAFETY: 读 union 前先经 vt 判别（ManuallyDrop Deref）
            let vt = unsafe { prop.Anonymous.Anonymous.vt.0 };
            if vt == windows::Win32::System::Variant::VT_CLSID.0 {
                // SAFETY: vt == VT_CLSID 保证 puuid 指向合法 GUID
                let guid = unsafe { *prop.Anonymous.Anonymous.Anonymous.puuid };
                if guid == WPD_EVENT_OBJECT_ADDED {
                    found = true;
                }
            }
            // SAFETY: GetAt 产出的 PROPVARIANT 可能持有堆内存，必须清理
            let _ = unsafe { PropVariantClear(&mut prop) };
            if found {
                break;
            }
        }
        Ok(found)
    }

    // -----------------------------------------------------------------------
    // 拍摄会话
    // -----------------------------------------------------------------------

    /// 一次拍摄：Advise → 发码 → 限时等 OBJECT_ADDED → 富化 → Unadvise。
    /// 返回业务结果（含超时语义），调度层错误经 DeviceResult 外传。
    /// 新对象富化：批量读文件名/大小（对象 ID 来自事件参数）。
    fn enrich_object(device: &IPortableDevice, object_id: &str) -> DeviceResult<CapturedObject> {
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(wpd_com::win_error)?;
        // SAFETY: 设备已 Open
        let properties = unsafe { content.Properties() }.map_err(wpd_com::win_error)?;
        // SAFETY: CLSID 为静态常量；无外部聚合
        let keys: IPortableDeviceKeyCollection = unsafe {
            CoCreateInstance(
                &wpd_com::CLSID_PORTABLE_DEVICE_KEY_COLLECTION,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(wpd_com::win_error)?;
        for key in [
            &WPD_OBJECT_ORIGINAL_FILE_NAME,
            &WPD_OBJECT_NAME,
            &WPD_OBJECT_SIZE,
        ] {
            // SAFETY: key 指向模块级静态常量
            unsafe { keys.Add(key) }.map_err(wpd_com::win_error)?;
        }
        let wide = wpd_com::to_wide(object_id);
        // SAFETY: 批量属性查询；keys 为合法集合；wide 以 NUL 结尾
        let values =
            unsafe { properties.GetValues(windows::core::PCWSTR(wide.as_ptr()), Some(&keys)) }
                .map_err(wpd_com::win_error)?;
        let name = wpd_com::string_prop(&values, &WPD_OBJECT_ORIGINAL_FILE_NAME)
            .or_else(|| wpd_com::string_prop(&values, &WPD_OBJECT_NAME))
            .unwrap_or_else(|| object_id.to_string());
        // SAFETY: 键为静态常量
        let size = unsafe { values.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE) }.unwrap_or(0);
        Ok(CapturedObject {
            object_id: object_id.to_string(),
            object_name: name,
            object_size: size,
        })
    }

    // -----------------------------------------------------------------------
    // OBJECT_ADDED 回调（驱动线程 → 等待器）
    // -----------------------------------------------------------------------

    #[cfg(test)]
    mod transfer_tests {
        use super::*;
        #[test]
        #[ignore = "requires exclusive Sony MTP camera; reads protocol capabilities and property list only"]
        fn live_mtp_bulk_operation_probe() {
            use windows::core::Interface;
            let _com = wpd_com::ComApartment::init().unwrap();
            let (id, _) = wpd_com::enumerate().unwrap().into_iter().find(|(_, name)| name.contains("ILCE")).unwrap();
            let device = wpd_com::open_device(&id).unwrap();
            let (response, bytes) = execute_with_data_to_read(&device, 0x1001, &[]).unwrap();
            assert_eq!(response, 0x2001);
            let info = mtp::parse_device_info(&bytes).unwrap();
            log::warn!("Native MTP GetObjectPropList supported: {}; operations={:x?}", info.operations_supported.contains(&0x9805), info.operations_supported);
            for format in [0u32, 0x3001, 0x3801, 0x3000, 0xb101] {
                let now = Instant::now();
                let results = send_command(&device, &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITHOUT_DATA_PHASE, 0x1006, &[u32::MAX, format, 0]).unwrap();
                let code = response_code(&results).unwrap();
                let params = unsafe { results.GetIUnknownValue(&windows::Win32::Devices::PortableDevices::WPD_PROPERTY_MTP_EXT_RESPONSE_PARAMS) }.unwrap().cast::<IPortableDevicePropVariantCollection>().unwrap();
                let mut value = PROPVARIANT::default();
                unsafe { params.GetAt(0, &mut value as *mut _ as *const _) }.unwrap();
                let number = unsafe { value.Anonymous.Anonymous.Anonymous.ulVal };
                let _ = unsafe { PropVariantClear(&mut value) };
                log::warn!("GetNumObjects format={format:x} response={code:x} count={number} elapsed={:?}", now.elapsed());
            }
            let now = Instant::now();
            let (response, handles) = execute_with_data_limit(&device, 0x1007, &[u32::MAX, 0x3801, 0], 128*1024).unwrap();
            log::warn!("GetObjectHandles response={response:x} bytes={} elapsed={:?}", handles.len(), now.elapsed());
            assert_eq!(response, 0x2001);
            if handles.len() >= 8 {
                let handle = u32::from_le_bytes(handles[4..8].try_into().unwrap());
                let now = Instant::now();
                let (response, data) = execute_with_data_limit(&device, 0x1008, &[handle], 4096).unwrap();
                log::warn!("GetObjectInfo handle={handle:x} response={response:x} bytes={} elapsed={:?}", data.len(), now.elapsed());
                assert_eq!(response, 0x2001);
            }
        }
        #[test]
        fn known_size_finishes_without_extra_read() {
            let mut calls = 0;
            let data = read_payload(Some(5), 4, |size| {
                calls += 1;
                Ok(vec![1; size])
            })
            .unwrap();
            assert_eq!(data.len(), 5);
            assert_eq!(calls, 2);
        }
        #[test]
        fn unknown_size_finishes_on_short_chunk() {
            let mut calls = 0;
            let data = read_payload(Some(u32::MAX as u64), 4, |_| {
                calls += 1;
                Ok(vec![1; if calls == 1 { 4 } else { 2 }])
            })
            .unwrap();
            assert_eq!(data.len(), 6);
            assert_eq!(calls, 2);
        }
        #[test]
        fn truncated_and_oversized_payloads_fail() {
            assert!(read_payload(Some(5), 4, |_| Ok(vec![])).is_err());
            assert!(
                read_payload(Some(DEVICE_INFO_MAX_BYTES as u64 + 1), 4, |_| panic!(
                    "must not read"
                ))
                .is_err()
            );
        }
        #[test]
        fn chunk_cap_widens_only_for_large_reads() {
            // 小读取维持 64KB 原钳制（描述符 64KB / DeviceInfo 256KB 各自不变）
            assert_eq!(chunk_cap(64 * 1024), 64 * 1024);
            assert_eq!(chunk_cap(256 * 1024), 256 * 1024);
            // 取景帧一类的 16MB 上限读取放宽到 1MB 封顶
            assert_eq!(chunk_cap(16 * 1024 * 1024), 1024 * 1024);
            assert_eq!(chunk_cap(usize::MAX), 1024 * 1024);
        }
        #[test]
        #[ignore = "takes one photo on an explicitly selected camera"]
        fn live_wpd_capture_and_read() {
            use super::super::super::backend::CameraBackend;
            use super::super::WpdMtpBackend;
            let id =
                std::env::var("PHOTOGRAPHER_TEST_CAMERA").expect("set PHOTOGRAPHER_TEST_CAMERA");
            let backend = WpdMtpBackend::new();
            backend.connect(&id).expect("connect");
            let object = backend
                .capture_still(&id, Duration::from_secs(20))
                .expect("trigger shutter and receive object");
            println!("Captured: {object:?}");
            let mut input = backend
                .open_captured(&id, &object)
                .expect("open captured photo");
            let size =
                std::io::copy(&mut input, &mut std::io::sink()).expect("read captured photo");
            assert!(size > 0);
            println!("Transferred {size} bytes");
            backend.acknowledge_captured(&id, &object).unwrap();
            backend.disconnect(&id);
        }
        #[test]
        #[ignore = "requires an explicitly selected connected camera"]
        fn live_wpd_camera_probe() {
            let id =
                std::env::var("PHOTOGRAPHER_TEST_CAMERA").expect("set PHOTOGRAPHER_TEST_CAMERA");
            let info = probe_device(&id).expect("WPD camera probe");
            println!("Camera: {info:?}");
            assert!(
                info.capabilities.standard_capture() || info.capabilities.vendor_capture_nikon()
            );
        }
    }
}
