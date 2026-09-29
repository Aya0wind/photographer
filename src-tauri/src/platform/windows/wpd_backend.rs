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
    apply_cached_capabilities, CameraBackend, CameraInfo, Capabilities, CapturedObject, TetherError,
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

    /// 取设备已探测能力（未探测返回 None）。
    pub(super) fn capabilities(pnp: &str) -> Option<Capabilities> {
        map()
            .lock()
            .expect("tethering cache mutex poisoned")
            .get(&crate::devices::normalize_device_id(pnp))
            .map(|info| info.capabilities)
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
}

impl CameraBackend for WpdMtpBackend {
    fn id(&self) -> &'static str {
        BACKEND_ID
    }

    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    /// 类型上限（设备级以探测为准）：文件通道 + 两种拍摄码 + OBJECT_ADDED
    /// 均为 WPD 通道潜在能力；LiveView v1 不支持。
    fn capabilities(&self) -> Capabilities {
        Capabilities::FILE_TRANSFER
            .union(Capabilities::STANDARD_CAPTURE)
            .union(Capabilities::VENDOR_CAPTURE_NIKON)
            .union(Capabilities::OBJECT_ADDED_EVENTS)
    }

    /// 枚举 WPD 设备（复用既有 worker 枚举，不逐台 Open）；已探测设备
    /// 以缓存能力位填充，未探测设备只给默认位（fileTransfer）。
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        let devices = crate::devices::wpd::enumerate_mtp_devices().map_err(TetherError::from)?;
        let infos = devices
            .into_iter()
            .map(|(pnp, name)| CameraInfo {
                pnp_id: pnp,
                name,
                capabilities: Capabilities::FILE_TRANSFER,
            })
            .collect();
        Ok(apply_cached_capabilities(infos, &cache::snapshot()))
    }

    /// 打开会话 + 全量探测（worker 线程执行；probe 通道与数据通道分离）。
    fn connect(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        {
            let pnp = pnp_id.to_string();
            let info = crate::devices::wpd::schedule_com_operation(
                &format!("probe:{pnp}"),
                Duration::from_secs(30),
                move || com::probe_device(&pnp),
            )
            .map_err(TetherError::from)?;
            cache::insert(info.clone());
            Ok(info)
        }
    }

    /// 断开：丢弃会话态缓存并隔离旧连接（复用既有 invalidate）。
    fn disconnect(&self, pnp_id: &str) {
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
                move || com::capture_session(&pnp, timeout),
            ) {
                // 拍摄业务结果（含 Timeout/CaptureNotSupported 等）
                Ok(inner) => inner,
                // 调度/COM 层失败（会话打不开、拔线等）
                Err(device_err) => Err(TetherError::from(device_err)),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Windows COM 实现（全部调用带 SAFETY 注释；对象生命周期归宿 worker 线程）
// ---------------------------------------------------------------------------

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
        WPD_EVENT_OBJECT_ADDED, WPD_EVENT_PARAMETER_EVENT_ID, WPD_OBJECT_ID, WPD_OBJECT_NAME,
        WPD_OBJECT_ORIGINAL_FILE_NAME, WPD_OBJECT_SIZE, WPD_PROPERTY_COMMON_COMMAND_CATEGORY,
        WPD_PROPERTY_COMMON_COMMAND_ID, WPD_PROPERTY_MTP_EXT_OPERATION_CODE,
        WPD_PROPERTY_MTP_EXT_OPERATION_PARAMS, WPD_PROPERTY_MTP_EXT_OPTIMAL_TRANSFER_BUFFER_SIZE,
        WPD_PROPERTY_MTP_EXT_RESPONSE_CODE, WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT,
        WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_READ,
        WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_READ,
    };
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
    use windows::Win32::System::Variant::VT_UI4;

    use crate::devices::wpd::com as wpd_com;
    use crate::devices::{DeviceError, DeviceResult};

    use super::super::backend::{CameraInfo, CapturedObject, TetherError};
    use super::super::mtp;

    /// 默认读块（驱动建议值缺省时的回退）。
    const READ_CHUNK_FALLBACK: u32 = 64 * 1024;
    /// DeviceInfo 数据集大小上限（正常 ≤ 数 KB；防御异常设备）。
    const DEVICE_INFO_MAX_BYTES: usize = 256 * 1024;

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
        unsafe { device.SendCommand(0, &values) }.map_err(wpd_com::win_error)
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
        let results = send_command(
            device,
            &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_READ,
            code,
            params,
        )?;
        let resp = response_code(&results)?;
        if resp != mtp::PTP_RESP_OK {
            return Ok((resp, Vec::new())); // 设备层拒绝：由调用方按响应码报错
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
        // 驱动建议块大小（尽力而为；缺省 64KB）
        // SAFETY: 键为静态常量
        let chunk = unsafe {
            results.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_OPTIMAL_TRANSFER_BUFFER_SIZE)
        }
        .unwrap_or(READ_CHUNK_FALLBACK)
        .clamp(1024, READ_CHUNK_FALLBACK) as usize;

        let mut data = Vec::new();
        loop {
            let read = read_data_chunk(device, &context, chunk)?;
            if read.is_empty() {
                break; // 驱动报告 0 字节：数据相位结束
            }
            data.extend_from_slice(&read);
            if data.len() > DEVICE_INFO_MAX_BYTES {
                return Err(DeviceError::Other("MTP DeviceInfo 数据集异常超限".into()));
            }
        }
        // 结束传输（携带上下文；失败尽力而为——上下文随会话回收）
        let end = (|| {
            let values = base_command_values(&WPD_COMMAND_MTP_EXT_END_DATA_TRANSFER)?;
            let wide = wpd_com::to_wide(&context);
            // SAFETY: 键为静态常量；wide 以 NUL 结尾且在本调用内存活
            unsafe {
                values.SetStringValue(
                    &WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT,
                    windows::core::PCWSTR(wide.as_ptr()),
                )
            }
            .map_err(wpd_com::win_error)?;
            // SAFETY: 设备已 Open
            unsafe { device.SendCommand(0, &values) }.map_err(wpd_com::win_error)
        })();
        if let Err(err) = end {
            crate::devices::diagnostics::record(format!(
                "MTP END_DATA_TRANSFER 失败（忽略）: {err}"
            ));
        }
        Ok((resp, data))
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
        // SAFETY: 设备已 Open
        let results = unsafe { device.SendCommand(0, &values) }.map_err(wpd_com::win_error)?;
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
        if ptr.is_null() || count == 0 {
            return Ok(Vec::new());
        }
        // SAFETY: ptr 指向驱动分配的 count 字节缓冲（本块生命周期至下方 free）
        let chunk_data = unsafe { std::slice::from_raw_parts(ptr, count as usize) }.to_vec();
        // SAFETY: GetBufferValue 缓冲由 API 以 CoTaskMem 分配
        unsafe { CoTaskMemFree(Some(ptr as *const core::ffi::c_void)) };
        Ok(chunk_data)
    }

    // -----------------------------------------------------------------------
    // 能力探测（camera_probe / capture 前置）
    // -----------------------------------------------------------------------

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
        // OpenSession 支持情况仅记录（WPD 驱动自管 PTP 会话，无需手动开）
        crate::devices::diagnostics::record(format!(
            "MTP 探测 {}: standard=v{} vendor_ext=0x{:08X} ops={} 事件={} OpenSession 支持={}",
            crate::devices::normalize_device_id(pnp_id),
            parsed.standard_version,
            parsed.vendor_extension_id,
            parsed.operations_supported.len(),
            parsed.events_supported.len(),
            parsed
                .operations_supported
                .contains(&mtp::PTP_OP_OPEN_SESSION),
        ));
        let wpd_object_added = supported_events_contains_object_added(&device)?;
        let caps = mtp::capabilities_from_probe(
            &parsed.operations_supported,
            &parsed.events_supported,
            wpd_object_added,
        );
        Ok(CameraInfo {
            pnp_id: pnp_id.to_string(),
            name: crate::devices::hotplug::pnp_display_name(pnp_id),
            capabilities: caps,
        })
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
    pub fn capture_session(
        pnp_id: &str,
        timeout: Duration,
    ) -> DeviceResult<Result<CapturedObject, TetherError>> {
        (|| {
            let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
            let device = wpd_com::open_device(pnp_id)?;

            // 能力选码：缓存命中直接用；否则本会话现场探测（GetDeviceInfo）
            let caps = match super::cache::capabilities(pnp_id) {
                Some(caps) => caps,
                None => {
                    let get_info = mtp::get_device_info_command();
                    let (resp, data) =
                        execute_with_data_to_read(&device, get_info.code, &get_info.params)?;
                    if resp != mtp::PTP_RESP_OK {
                        return Err(DeviceError::Other(format!(
                            "GetDeviceInfo 被拒绝（MTP 响应 0x{resp:04X}）"
                        )));
                    }
                    let parsed = mtp::parse_device_info(&data)
                        .map_err(|e| DeviceError::Other(e.to_string()))?;
                    let wpd_object_added = supported_events_contains_object_added(&device)?;
                    mtp::capabilities_from_probe(
                        &parsed.operations_supported,
                        &parsed.events_supported,
                        wpd_object_added,
                    )
                }
            };
            let command = mtp::capture_command(caps).map_err(|err| {
                // 选不出拍摄码：设备级 Other 文案（经 From 转回 TetherError）
                DeviceError::Other(err.to_string())
            })?;

            // 订阅 OBJECT_ADDED（回调仅提取对象 ID，经 Condvar 唤醒本线程）
            let waiter = Arc::new(ObjectAddedWait::default());
            // SAFETY: implement 宏生成的 COM 包装；Advise 期间驱动持有引用
            let callback: IPortableDeviceEventCallback = ObjectAddedCallback {
                waiter: Arc::clone(&waiter),
            }
            .into();
            // SAFETY: CLSID 为静态常量；无外部聚合（空过滤参数 = 全部事件）
            let advise_params: IPortableDeviceValues = unsafe {
                CoCreateInstance(
                    &wpd_com::CLSID_PORTABLE_DEVICE_VALUES,
                    None::<&windows::core::IUnknown>,
                    CLSCTX_INPROC_SERVER,
                )
            }
            .map_err(wpd_com::win_error)?;
            // SAFETY: 设备已 Open；回调接口已实现 IPortableDeviceEventCallback
            let cookie_ptr = unsafe { device.Advise(0, &callback, &advise_params) }
                .map_err(wpd_com::win_error)?;
            // SAFETY: Advise 出参为 NUL 结尾宽字符串
            let cookie = unsafe { wpd_com::pwstr_to_string(cookie_ptr) };
            // SAFETY: Advise 分配的 cookie 字符串由调用方释放
            unsafe { wpd_com::free_pwstr(cookie_ptr) };

            let outcome = (|| {
                let resp = execute_without_data(&device, command.code, &command.params)?;
                if resp != mtp::PTP_RESP_OK {
                    return Err(DeviceError::Other(format!(
                        "相机拒绝拍摄命令（MTP 响应 0x{resp:04X}）"
                    )));
                }
                // 等待新对象（回调线程唤醒；超时 → Timeout 语义）
                let Some(object_id) = waiter.wait(timeout) else {
                    return Ok(Err(TetherError::Timeout));
                };
                let captured = enrich_object(&device, &object_id)?;
                Ok(Ok(captured))
            })();

            // 清理（成功/失败/超时任何路径都 Unadvise；失败尽力而为）
            let cookie_wide = wpd_com::to_wide(&cookie);
            // SAFETY: cookie_wide 以 NUL 结尾且在本调用内存活
            if let Err(err) =
                unsafe { device.Unadvise(windows::core::PCWSTR(cookie_wide.as_ptr())) }
            {
                crate::devices::diagnostics::record(format!("Unadvise 失败（忽略）: {err}"));
            }
            outcome
        })()
    }

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

    /// 回调线程写 / worker 线程限时读 的单值等待器（Mutex+Condvar 天然
    /// Send+Sync，跨 COM 回调线程安全）。
    #[derive(Default)]
    struct ObjectAddedWait {
        object_id: Mutex<Option<String>>,
        cvar: Condvar,
    }

    impl ObjectAddedWait {
        fn set(&self, object_id: String) {
            let mut guard = self.object_id.lock().expect("object-added mutex poisoned");
            if guard.is_none() {
                *guard = Some(object_id);
            }
            self.cvar.notify_all();
        }

        /// 限时等待（首值即返回；到期返回 None）。
        fn wait(&self, timeout: Duration) -> Option<String> {
            let deadline = Instant::now() + timeout;
            let mut guard = self.object_id.lock().expect("object-added mutex poisoned");
            loop {
                if let Some(id) = guard.take() {
                    return Some(id);
                }
                let now = Instant::now();
                if now >= deadline {
                    return None;
                }
                let (next, _) = self
                    .cvar
                    .wait_timeout(guard, deadline - now)
                    .expect("object-added mutex poisoned");
                guard = next;
            }
        }
    }

    /// OBJECT_ADDED 事件回调：仅提取事件 GUID 与新对象 ID（不读属性——
    /// 富化回 worker 线程做，回调保持最小工作量）。
    #[implement(IPortableDeviceEventCallback)]
    struct ObjectAddedCallback {
        waiter: Arc<ObjectAddedWait>,
    }

    impl IPortableDeviceEventCallback_Impl for ObjectAddedCallback_Impl {
        fn OnEvent(&self, params: Ref<'_, IPortableDeviceValues>) -> windows::core::Result<()> {
            // Ref 是 ABI 包装：ok() 取回借用（NULL 参数不可能出现于驱动回调）
            let Ok(values) = params.ok() else {
                return Ok(());
            };
            // SAFETY: values 为驱动传入的事件参数集合（回调存活期内有效）
            unsafe {
                if let Ok(event_id) = values.GetGuidValue(&WPD_EVENT_PARAMETER_EVENT_ID) {
                    if event_id == WPD_EVENT_OBJECT_ADDED {
                        // SAFETY: WPD_OBJECT_ID 出参（若存在）为 API 分配宽字符串
                        if let Ok(ptr) = values.GetStringValue(&WPD_OBJECT_ID) {
                            // SAFETY: GetStringValue 出参为 NUL 结尾宽字符串
                            let id = wpd_com::pwstr_to_string(ptr);
                            // SAFETY: 出参缓冲由 API 以 CoTaskMem 分配
                            wpd_com::free_pwstr(ptr);
                            if !id.is_empty() {
                                self.waiter.set(id);
                            }
                        }
                    }
                }
            }
            Ok(())
        }
    }
}
