//! WPD/MTP 相机源（Windows Portable Devices COM）。
//!
//! 结构：跨平台纯函数（路径拼接 / OLE DATE / ISO 日期 / FILETIME 解析，
//! 有单测）+ Windows COM 实现 + 非 Windows 桩。
//!
//! COM 约定：每个对外入口（list/open_head/stream/enumerate）都在调用线程
//! `CoInitializeEx(COINIT_MULTITHREADED)` 并由 guard 配对 `CoUninitialize`；
//! COM 接口指针全部使用 windows crate 封装，禁止手写 AddRef/Release。
//!
//! 降级点（真机 ILCE-7RM5 验证通过 list/open_head/stream）：
//! - 单对象属性读取失败（受限目录等）跳过该对象，不中断整卷枚举；
//! - 设备缺失 ORIGINAL_FILE_NAME 时回退 WPD_OBJECT_NAME，再回退对象 ID；
//! - PERSISTENT_UNIQUE_ID 缺失时以会话对象 ID 充当（跨会话可能变化，
//!   影响“已导入”识别的稳定性）；
//! - stream() 为满足“接口 Release 先于 CoUninitialize”的 COM 契约，
//!   有意泄漏一次线程 COM 初始化计数（进程退出回收）；
//! - 目录递归深度上限 32；每对象一次属性批量查询（2349 文件约 1.5s
//!   热态，冷态约 160s——如需提速可后续改 IPortableDevicePropertiesBulk）。

use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};

use super::{DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

// ---------------------------------------------------------------------------
// 纯函数（跨平台，单测见 tests/devices_test.rs）
// ---------------------------------------------------------------------------

/// MTP 层级路径拼接（统一 `/` 分隔）。
pub fn join_rel_path(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// OLE 自动化日期（自 1899-12-30T00:00:00Z 的天数，含小数）→ UTC 时间。
/// OLE 序列 25569.0 恰为 Unix 纪元（1970-01-01）。
pub fn ole_date_to_utc(days: f64) -> Option<DateTime<Utc>> {
    if !days.is_finite() || !(-3_652_059.0..=3_652_059.0).contains(&days) {
        return None; // NaN/Inf/超 ±10000 年（chrono 表示范围余量）
    }
    let base = NaiveDate::from_ymd_opt(1899, 12, 30)?
        .and_hms_opt(0, 0, 0)?
        .and_utc();
    // 秒级精度足够（文件 mtime）；f64 在 4.6 万天尺度下分辨率远优于 1s
    let seconds = chrono::Duration::try_seconds((days * 86_400.0).round() as i64)?;
    base.checked_add_signed(seconds)
}

/// WPD 日期字符串解析：RFC3339 / ISO 无时区 / 空格分隔 / 基本格式
/// （`20240102T030405`）。无时区一律按 UTC。
pub fn parse_wpd_date_string(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S", "%Y%m%dT%H%M%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(naive.and_utc());
        }
    }
    None
}

/// Windows FILETIME（自 1601-01-01T00:00:00Z 的 100ns 计数）→ UTC 时间。
pub fn filetime_to_utc(low: u32, high: u32) -> Option<DateTime<Utc>> {
    let ticks = ((high as u64) << 32) | low as u64;
    let seconds = (ticks / 10_000_000) as i64;
    let nanos = ((ticks % 10_000_000) * 100) as u32;
    let unix = seconds.checked_sub(11_644_473_600)?; // 1601→1970 纪元差
    DateTime::from_timestamp(unix, nanos)
}

// ---------------------------------------------------------------------------
// 设备源
// ---------------------------------------------------------------------------

/// WPD/MTP 设备源：`pnp_id` 为 PnP 设备路径（enumerate_mtp_devices 的产出），
/// `friendly_name` 为展示名。
pub struct WpdSource {
    pnp_id: String,
    friendly_name: String,
}

impl WpdSource {
    pub fn new(pnp_id: impl Into<String>, friendly_name: impl Into<String>) -> Self {
        Self {
            pnp_id: pnp_id.into(),
            friendly_name: friendly_name.into(),
        }
    }
}

impl DeviceSource for WpdSource {
    fn id(&self) -> String {
        self.pnp_id.clone()
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Mtp
    }

    fn name(&self) -> String {
        self.friendly_name.clone()
    }

    #[cfg(windows)]
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        com::list(&self.pnp_id)
    }

    #[cfg(not(windows))]
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        Err(super::DeviceError::Other("WPD 仅在 Windows 可用".into()))
    }

    #[cfg(windows)]
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        com::open_head(&self.pnp_id, id, max)
    }

    #[cfg(not(windows))]
    fn open_head(&self, _id: &str, _max: u64) -> DeviceResult<Vec<u8>> {
        Err(super::DeviceError::Other("WPD 仅在 Windows 可用".into()))
    }

    #[cfg(windows)]
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        com::stream(&self.pnp_id, id)
    }

    #[cfg(not(windows))]
    fn stream(&self, _id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        Err(super::DeviceError::Other("WPD 仅在 Windows 可用".into()))
    }
}

/// 枚举全部 WPD/MTP 设备：`(pnp_id, friendly_name)` 列表。
#[cfg(windows)]
pub fn enumerate_mtp_devices() -> DeviceResult<Vec<(String, String)>> {
    com::enumerate()
}

#[cfg(not(windows))]
pub fn enumerate_mtp_devices() -> DeviceResult<Vec<(String, String)>> {
    Ok(Vec::new())
}

// ---------------------------------------------------------------------------
// Windows COM 实现
// ---------------------------------------------------------------------------

#[cfg(windows)]
mod com {
    use chrono::{DateTime, Utc};

    use super::super::{is_media_ext, DeviceError, DeviceResult, FileEntry};
    use super::{filetime_to_utc, join_rel_path, ole_date_to_utc, parse_wpd_date_string};
    use crate::devices::hotplug::pnp_display_name;

    use windows::core::{Error as WinError, GUID, HRESULT, PCWSTR, PWSTR};
    use windows::Win32::Devices::PortableDevices::{
        IPortableDevice, IPortableDeviceContent, IPortableDeviceKeyCollection,
        IPortableDeviceManager, IPortableDevicePropVariantCollection, IPortableDeviceValues,
        WPD_CLIENT_MAJOR_VERSION, WPD_CLIENT_NAME, WPD_CONTENT_TYPE_FOLDER,
        WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_DEVICE_OBJECT_ID, WPD_OBJECT_CONTENT_TYPE,
        WPD_OBJECT_DATE_MODIFIED, WPD_OBJECT_NAME, WPD_OBJECT_ORIGINAL_FILE_NAME,
        WPD_OBJECT_PERSISTENT_UNIQUE_ID, WPD_OBJECT_SIZE, WPD_RESOURCE_DEFAULT,
    };
    use windows::Win32::Foundation::PROPERTYKEY;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemAlloc, CoTaskMemFree, CoUninitialize, IStream,
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::Variant::{VT_DATE, VT_FILETIME, VT_LPWSTR};

    /// coclass GUID（SDK 头 PortableDeviceApi.h / PortableDeviceTypes.h 的
    /// class DECLSPEC_UUID；windows crate 未收录 CLSID 常量，此处照抄数值）。
    const CLSID_PORTABLE_DEVICE_MANAGER: GUID =
        GUID::from_u128(0x0af10cec_2ecd_4b92_9581_34f6ae0637f3);
    const CLSID_PORTABLE_DEVICE: GUID = GUID::from_u128(0x728a21c5_3d9e_48d7_9810_864848f0f404);
    const CLSID_PORTABLE_DEVICE_VALUES: GUID =
        GUID::from_u128(0x0c15d503_d017_47ce_9016_7b3f978721cc);
    const CLSID_PORTABLE_DEVICE_KEY_COLLECTION: GUID =
        GUID::from_u128(0xde2d022d_2480_43be_97f0_d1fa2cf98f4f);
    const CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION: GUID =
        GUID::from_u128(0x08a99e2f_6d6d_4b80_af5a_baf2bcbe4cb9);

    /// HRESULT → 设备错误语义（spec §5.1：UI 需区分“可提示操作设备”与“拔线”）。
    fn hr_error(hr: HRESULT) -> DeviceError {
        match hr.0 as u32 {
            0x8007_0005 => DeviceError::AccessDenied, // E_ACCESSDENIED：相机未切 PC 模式 / 手机锁定
            0x8007_0015 | 0x8007_048F | 0x8007_04C7 => DeviceError::Disconnected, // NOT_READY / DEVICE_NOT_CONNECTED / CANCELLED：传输中拔线
            _ => DeviceError::Other(format!("WPD error 0x{:08X}", hr.0 as u32)),
        }
    }

    fn win_error(e: WinError) -> DeviceError {
        hr_error(e.code())
    }

    /// 每线程 COM 初始化 guard。
    ///
    /// S_OK（首次初始化）与 S_FALSE（该线程已初始化）都必须配对一次
    /// `CoUninitialize`；RPC_E_CHANGED_MODE（他线程模型已存在）视为失败。
    struct ComApartment {
        active: bool,
    }

    impl ComApartment {
        fn init() -> Result<Self, WinError> {
            // SAFETY: 无指针参数，仅改变当前线程 COM 状态
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if hr.is_ok() {
                Ok(Self { active: true })
            } else {
                Err(WinError::from(hr))
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.active {
                // SAFETY: 与本 guard 的成功 CoInitializeEx（含 S_FALSE）配对
                unsafe { CoUninitialize() };
            }
        }
    }

    impl ComApartment {
        /// 放弃注销（见 stream() 的 COM 契约说明）。
        fn leak(mut self) {
            self.active = false;
        }
    }

    fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 拷贝 PWSTR → String（不取得所有权；释放由调用方负责）。
    ///
    /// SAFETY: p 必须为 NULL 或指向 NUL 结尾的合法宽字符串。
    unsafe fn pwstr_to_string(p: PWSTR) -> String {
        if p.is_null() {
            return String::new();
        }
        // SAFETY: p 非空且 NUL 结尾
        match unsafe { p.to_string() } {
            Ok(s) => s,
            // 代理对损坏时降级为替换字符，不使枚举整体失败
            Err(_) => String::from_utf16_lossy(unsafe { p.as_wide() }),
        }
    }

    /// 释放 WPD API 以 CoTaskMem 分配并转移给调用方的字符串。
    ///
    /// SAFETY: p 必须来自 WPD API 的出参且未被重复释放。
    unsafe fn free_pwstr(p: PWSTR) {
        if !p.is_null() {
            // SAFETY: 见函数级注释
            unsafe { CoTaskMemFree(Some(p.as_ptr() as *const core::ffi::c_void)) };
        }
    }

    // -----------------------------------------------------------------------
    // 设备枚举
    // -----------------------------------------------------------------------

    pub fn enumerate() -> DeviceResult<Vec<(String, String)>> {
        let _com = ComApartment::init().map_err(win_error)?;
        // SAFETY: CLSID 为静态常量；无外部聚合
        let manager: IPortableDeviceManager = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE_MANAGER,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(win_error)?;

        let mut count = 0u32;
        // SAFETY: 两段式调用第一段：仅查询设备数（数组指针为 NULL）
        unsafe { manager.GetDevices(std::ptr::null_mut(), &mut count) }.map_err(win_error)?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut ids = vec![PWSTR::null(); count as usize];
        // SAFETY: 缓冲区容量与第一段查询一致
        unsafe { manager.GetDevices(ids.as_mut_ptr(), &mut count) }.map_err(win_error)?;

        let mut out = Vec::new();
        for &id_ptr in &ids[..count as usize] {
            // SAFETY: id_ptr 由 GetDevices 填充（可能为 NULL）
            let pnp = unsafe { pwstr_to_string(id_ptr) };
            if !pnp.is_empty() {
                let name = friendly_name(&manager, &pnp).unwrap_or_else(|| pnp_display_name(&pnp));
                out.push((pnp, name));
            }
            // SAFETY: GetDevices 分配的 PnP 字符串由调用方释放
            unsafe { free_pwstr(id_ptr) };
        }
        Ok(out)
    }

    /// 设备友好名（两段式 GetDeviceFriendlyName）；失败返回 None 由调用方降级。
    fn friendly_name(manager: &IPortableDeviceManager, pnp: &str) -> Option<String> {
        let wide = to_wide(pnp);
        let mut len = 0u32;
        // SAFETY: 第一段以空缓冲取所需长度（预期返回失败）
        let _ = unsafe {
            manager.GetDeviceFriendlyName(PCWSTR(wide.as_ptr()), PWSTR::null(), &mut len)
        };
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize];
        // SAFETY: buf 容量满足第一段报告的长度
        let hr = unsafe {
            manager.GetDeviceFriendlyName(PCWSTR(wide.as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len)
        };
        if hr.is_err() {
            return None;
        }
        let end = (len as usize).min(buf.len());
        let name = String::from_utf16_lossy(&buf[..end]);
        let name = name.trim_end_matches('\0');
        (!name.is_empty()).then(|| name.to_string())
    }

    // -----------------------------------------------------------------------
    // 对象树枚举（list）
    // -----------------------------------------------------------------------

    /// 每批 EnumObjects/Next 的对象数。
    const BATCH: usize = 64;
    /// 目录递归深度上限（防御异常设备；正常 DCIM 树深 3~5 层）。
    const MAX_DEPTH: u32 = 32;
    /// open_head 的读块大小。
    const HEAD_CHUNK: usize = 64 * 1024;

    pub fn list(pnp_id: &str) -> DeviceResult<Vec<FileEntry>> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let keys = build_property_keys()?;
        let mut out = Vec::new();
        walk_folder(&content, &keys, WPD_DEVICE_OBJECT_ID, "", 0, &mut out)?;
        out.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
    }

    fn open_device(pnp_id: &str) -> DeviceResult<IPortableDevice> {
        // SAFETY: CLSID 为静态常量；无外部聚合
        let client_info: IPortableDeviceValues = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE_VALUES,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(win_error)?;
        // 客户端信息尽力而为：部分设备要求非空客户端名/版本才允许 Open
        // SAFETY: key 为静态常量
        let _ = unsafe {
            client_info.SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Smart Photo"))
        };
        // SAFETY: key 为静态常量
        let _ = unsafe { client_info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1) };

        // SAFETY: CLSID 为静态常量；无外部聚合
        let device: IPortableDevice = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(win_error)?;
        let wide = to_wide(pnp_id);
        // SAFETY: wide 以 NUL 结尾且在本调用内存活
        unsafe { device.Open(PCWSTR(wide.as_ptr()), &client_info) }.map_err(win_error)?;
        Ok(device)
    }

    /// 批量属性键：类型 / 大小 / 文件名 / 名称 / 持久 ID / 修改时间。
    fn build_property_keys() -> DeviceResult<IPortableDeviceKeyCollection> {
        // SAFETY: CLSID 为静态常量；无外部聚合
        let keys: IPortableDeviceKeyCollection = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE_KEY_COLLECTION,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(win_error)?;
        for key in [
            &WPD_OBJECT_CONTENT_TYPE,
            &WPD_OBJECT_SIZE,
            &WPD_OBJECT_ORIGINAL_FILE_NAME,
            &WPD_OBJECT_NAME,
            &WPD_OBJECT_PERSISTENT_UNIQUE_ID,
            &WPD_OBJECT_DATE_MODIFIED,
        ] {
            // SAFETY: key 指向模块级静态常量
            unsafe { keys.Add(key) }.map_err(win_error)?;
        }
        Ok(keys)
    }

    /// 递归枚举目录：folder 为对象 ID（WPD_DEVICE_OBJECT_ID 起），
    /// prefix 为已积累的 `/` 分隔相对路径。
    fn walk_folder(
        content: &IPortableDeviceContent,
        keys: &IPortableDeviceKeyCollection,
        folder: PCWSTR,
        prefix: &str,
        depth: u32,
        out: &mut Vec<FileEntry>,
    ) -> DeviceResult<()> {
        if depth > MAX_DEPTH {
            return Ok(());
        }
        // SAFETY: folder 为有效对象 ID；无过滤条件（None = 全部子对象）
        let enumerator = unsafe { content.EnumObjects(0, folder, None::<&IPortableDeviceValues>) }
            .map_err(win_error)?;
        loop {
            let mut batch = vec![PWSTR::null(); BATCH];
            let mut fetched = 0u32;
            // SAFETY: batch 容量与请求条数一致
            let hr = unsafe { enumerator.Next(&mut batch, &mut fetched) };
            if hr.is_err() {
                return Err(hr_error(hr));
            }
            if fetched == 0 {
                break;
            }
            for &id_ptr in &batch[..fetched as usize] {
                // SAFETY: id_ptr 由 Next 填充（可能为 NULL）
                let obj_id = unsafe { pwstr_to_string(id_ptr) };
                // SAFETY: Next 分配的对象 ID 字符串由调用方释放
                unsafe { free_pwstr(id_ptr) };
                if obj_id.is_empty() {
                    continue;
                }
                // 单对象属性失败（受限目录等）跳过，不中断整卷枚举
                let _ = process_object(content, keys, &obj_id, prefix, depth, out);
            }
            if fetched < BATCH as u32 {
                break; // S_FALSE：本次不足一批，枚举已尽
            }
        }
        Ok(())
    }

    /// 处理单个对象：目录则递归；媒体文件则产出 FileEntry。
    fn process_object(
        content: &IPortableDeviceContent,
        keys: &IPortableDeviceKeyCollection,
        obj_id: &str,
        prefix: &str,
        depth: u32,
        out: &mut Vec<FileEntry>,
    ) -> DeviceResult<()> {
        // SAFETY: 设备已 Open
        let properties = unsafe { content.Properties() }.map_err(win_error)?;
        let wide = to_wide(obj_id);
        // SAFETY: 批量属性查询；keys 为合法集合
        let values = unsafe { properties.GetValues(PCWSTR(wide.as_ptr()), Some(keys)) }
            .map_err(win_error)?;

        // SAFETY: 属性键为静态常量
        let content_type =
            unsafe { values.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE) }.unwrap_or(GUID::zeroed());

        // 目录判定：标准文件夹 + 功能对象（MTP 设备的存储根是
        // WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT，须下钻才能到达 DCIM）。
        // 存储根名（如 "Storage Media"）不进入 rel_path——保持
        // `DCIM/100CANON/IMG_0001.CR3` 形态。
        if content_type == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT {
            let child = to_wide(obj_id);
            // SAFETY: child 以 NUL 结尾且在递归调用期间存活
            walk_folder(
                content,
                keys,
                PCWSTR(child.as_ptr()),
                prefix,
                depth + 1,
                out,
            )?;
        } else if content_type == WPD_CONTENT_TYPE_FOLDER {
            let name = string_prop(&values, &WPD_OBJECT_ORIGINAL_FILE_NAME)
                .or_else(|| string_prop(&values, &WPD_OBJECT_NAME))
                .unwrap_or_else(|| obj_id.to_string());
            let child_prefix = join_rel_path(prefix, &name);
            let child = to_wide(obj_id);
            // SAFETY: child 以 NUL 结尾且在递归调用期间存活
            walk_folder(
                content,
                keys,
                PCWSTR(child.as_ptr()),
                &child_prefix,
                depth + 1,
                out,
            )?;
        } else {
            let file_name = string_prop(&values, &WPD_OBJECT_ORIGINAL_FILE_NAME)
                .or_else(|| string_prop(&values, &WPD_OBJECT_NAME))
                .ok_or_else(|| DeviceError::Other(format!("对象缺文件名: {obj_id}")))?;
            let ext = file_name.rsplit('.').next().unwrap_or_default();
            if !is_media_ext(ext) {
                return Ok(());
            }
            // SAFETY: 属性键为静态常量
            let size =
                unsafe { values.GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE) }.unwrap_or(0);
            let mtime = date_prop(&values, &WPD_OBJECT_DATE_MODIFIED).unwrap_or_else(Utc::now);
            let id = string_prop(&values, &WPD_OBJECT_PERSISTENT_UNIQUE_ID)
                .unwrap_or_else(|| obj_id.to_string());
            out.push(FileEntry {
                id,
                rel_path: join_rel_path(prefix, &file_name),
                size,
                mtime,
            });
        }
        Ok(())
    }

    /// 读取字符串属性（GetStringValue；返回缓冲由 API 分配，须 CoTaskMemFree）。
    fn string_prop(values: &IPortableDeviceValues, key: *const PROPERTYKEY) -> Option<String> {
        // SAFETY: key 为静态常量
        let p = unsafe { values.GetStringValue(key) }.ok()?;
        // SAFETY: GetStringValue 的出参为 NUL 结尾宽字符串
        let s = unsafe { pwstr_to_string(p) };
        // SAFETY: 出参缓冲由 API 以 CoTaskMem 分配
        unsafe { free_pwstr(p) };
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    }

    /// 读取日期属性（PROPVARIANT 按 vt 判别：VT_DATE / VT_LPWSTR / VT_FILETIME）。
    fn date_prop(values: &IPortableDeviceValues, key: *const PROPERTYKEY) -> Option<DateTime<Utc>> {
        // SAFETY: key 为静态常量
        let mut prop = unsafe { values.GetValue(key) }.ok()?;
        // SAFETY: 函数内部先读 vt 再按判别读 union
        let parsed = unsafe { propvariant_date(&prop) };
        // SAFETY: prop 可能持有堆内存（VT_LPWSTR 等），必须清理
        let _ = unsafe { PropVariantClear(&mut prop) };
        parsed
    }

    /// SAFETY: 仅在按 prop.vt 判别后的分支读取 union 对应成员。
    unsafe fn propvariant_date(prop: &PROPVARIANT) -> Option<DateTime<Utc>> {
        // SAFETY: 读 vt 与 union 均为 union 的合法字段访问
        let inner = unsafe { &prop.Anonymous.Anonymous };
        let vt = inner.vt.0;
        if vt == VT_DATE.0 {
            // SAFETY: vt == VT_DATE 保证 date 成员有效
            ole_date_to_utc(unsafe { inner.Anonymous.date })
        } else if vt == VT_LPWSTR.0 {
            // SAFETY: vt == VT_LPWSTR 保证 pwszVal 指向 NUL 结尾字符串
            parse_wpd_date_string(&unsafe { pwstr_to_string(inner.Anonymous.pwszVal) })
        } else if vt == VT_FILETIME.0 {
            // SAFETY: vt == VT_FILETIME 保证 filetime 成员有效
            let ft = unsafe { inner.Anonymous.filetime };
            filetime_to_utc(ft.dwLowDateTime, ft.dwHighDateTime)
        } else {
            None
        }
    }

    // -----------------------------------------------------------------------
    // 资源读取（open_head / stream）
    // -----------------------------------------------------------------------

    /// 持久唯一 ID → 当前会话对象 ID。
    ///
    /// FileEntry.id 采用 WPD 持久 ID（跨会话稳定，供“已导入”去重）；而
    /// Resources::GetStream 等对象操作只接受当前会话的临时对象 ID，须用
    /// GetObjectIDsFromPersistentUniqueIDs 翻译。翻译失败（设备不支持或
    /// 传入的本就是会话 ID）时回退原值。
    fn resolve_object_id(content: &IPortableDeviceContent, id: &str) -> String {
        translate_persistent_id(content, id).unwrap_or_else(|| id.to_string())
    }

    fn translate_persistent_id(
        content: &IPortableDeviceContent,
        persistent_id: &str,
    ) -> Option<String> {
        // SAFETY: CLSID 为静态常量；无外部聚合
        let request: IPortableDevicePropVariantCollection = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .ok()?;
        let mut prop = lpstr_propvariant(persistent_id)?;
        // SAFETY: prop 为本地合法 VT_LPWSTR PROPVARIANT；Add 拷贝值入集合
        let added = unsafe { request.Add(&prop) }.is_ok();
        // SAFETY: prop 持有 CoTaskMem 分配的字符串，无论 Add 成败都须清理
        let _ = unsafe { PropVariantClear(&mut prop) };
        if !added {
            return None;
        }
        // SAFETY: request 已装载 1 个持久 ID
        let results = unsafe { content.GetObjectIDsFromPersistentUniqueIDs(&request) }.ok()?;

        let mut count = 0u32;
        // SAFETY: count 为本帧出参（crate 元数据将出参声明为 *const，API 实际写入）
        unsafe { results.GetCount(&mut count as *mut u32 as *const u32) }.ok()?;
        if count == 0 {
            return None;
        }
        let mut out = PROPVARIANT::default();
        // SAFETY: out 为零初始化 PROPVARIANT；GetAt 写入它（*const 出参同上）
        unsafe { results.GetAt(0, &mut out as *mut PROPVARIANT as *const PROPVARIANT) }.ok()?;
        // SAFETY: 读 union 前先取判别字段 vt 的引用（ManuallyDrop Deref）
        let inner = unsafe { &out.Anonymous.Anonymous };
        let translated = if inner.vt.0 == VT_LPWSTR.0 {
            // SAFETY: vt == VT_LPWSTR 保证 pwszVal 指向 NUL 结尾字符串
            Some(unsafe { pwstr_to_string(inner.Anonymous.pwszVal) })
        } else {
            None
        };
        // SAFETY: GetAt 产出的 PROPVARIANT 可能持有堆内存，必须清理
        let _ = unsafe { PropVariantClear(&mut out) };
        let translated = translated?;
        (!translated.is_empty()).then_some(translated)
    }

    /// 构造持有 CoTaskMem 宽字符串的 VT_LPWSTR PROPVARIANT。
    /// 调用方负责 PropVariantClear。
    fn lpstr_propvariant(s: &str) -> Option<PROPVARIANT> {
        let wide = to_wide(s);
        let bytes = wide.len() * size_of::<u16>();
        // SAFETY: 常规 COM 任务堆分配
        let buf = unsafe { CoTaskMemAlloc(bytes) };
        if buf.is_null() {
            return None;
        }
        // SAFETY: buf 可写长度 ≥ bytes
        unsafe { std::ptr::copy_nonoverlapping(wide.as_ptr() as *const u8, buf as *mut u8, bytes) };
        let mut prop = PROPVARIANT::default();
        // SAFETY: 写本地零初始化 PROPVARIANT：vt 为具名字段；pwszVal 为
        // union 成员且随后以 VT_LPWSTR 解释，二者一致。外层 Anonymous 是
        // ManuallyDrop<PROPVARIANT_0_0>，写入须显式 DerefMut
        unsafe {
            let inner = &mut *prop.Anonymous.Anonymous;
            inner.vt = VT_LPWSTR;
            inner.Anonymous.pwszVal = PWSTR(buf as *mut u16);
        }
        Some(prop)
    }

    /// 打开对象默认资源（文件本体）的数据流。
    fn open_resource(
        content: &IPortableDeviceContent,
        obj_id: &str,
    ) -> Result<(IStream, u32), WinError> {
        // SAFETY: 设备已 Open
        let resources = unsafe { content.Transfer() }?;
        let wide = to_wide(obj_id);
        let mut stream: Option<IStream> = None;
        let mut optimal_buffer = 0u32;
        // SAFETY: WPD_RESOURCE_DEFAULT 为默认资源；宽字符串以 NUL 结尾
        unsafe {
            resources.GetStream(
                PCWSTR(wide.as_ptr()),
                &WPD_RESOURCE_DEFAULT,
                STGM_READ.0,
                &mut optimal_buffer,
                &mut stream,
            )
        }?;
        match stream {
            Some(stream) => Ok((stream, optimal_buffer)),
            None => Err(WinError::from_hresult(HRESULT(0x8000_4005u32 as i32))), // E_FAIL
        }
    }

    pub fn open_head(pnp_id: &str, obj_id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);
        let (stream, _) = open_resource(&content, &resolved).map_err(win_error)?;

        let mut out = Vec::new();
        let mut chunk = vec![0u8; HEAD_CHUNK];
        let mut remaining = max;
        while remaining > 0 {
            let want = (chunk.len() as u64).min(remaining) as u32;
            let mut got = 0u32;
            // SAFETY: chunk 可写长度 ≥ want
            let hr = unsafe {
                stream.Read(
                    chunk.as_mut_ptr() as *mut core::ffi::c_void,
                    want,
                    Some(&mut got),
                )
            };
            if hr.is_err() {
                return Err(hr_error(hr)); // 含传输中断 → Disconnected
            }
            if got == 0 {
                break; // 设备端流结束
            }
            out.extend_from_slice(&chunk[..got as usize]);
            remaining -= got as u64;
        }
        Ok(out)
    }

    pub fn stream(pnp_id: &str, obj_id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        let com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);
        let (stream, _) = open_resource(&content, &resolved).map_err(win_error)?;
        // COM 契约：所有接口 Release 必须先于 CoUninitialize。流会逃逸到
        // 调用方作用域（可能存活到导入任务结束），因此本线程的 COM 初始化
        // 有意不注销（leak 一次计数，进程退出时回收）——这是 M1 的已知
        // 降级点，比违反 Release/CoUninitialize 顺序（实测导致进程退出时
        // ACCESS_VIOLATION）安全。
        com.leak();
        // _device：保持设备会话打开——流关闭前不 Close 设备（部分设备
        // 会随 Close 中断未完成的资源流）
        Ok(Box::new(MtpStream {
            stream,
            _device: device,
        }))
    }

    /// IStream → std::io::Read 适配。读取中断映射为 `ConnectionAborted`
    /// （调用方据此判定设备拔线并暂停任务）。
    struct MtpStream {
        stream: IStream,
        /// 设备会话保活（见 stream() 说明）。
        _device: IPortableDevice,
    }

    // SAFETY: WPD 的资源 IStream 是进程内 COM 对象（非跨套间代理），
    // 允许在任意 CoInitializeEx(MULTITHREADED) 初始化的线程上调用；
    // 导入引擎在读取线程按 MTA 初始化 COM 后顺序读（不并发共享）。
    unsafe impl Send for MtpStream {}

    impl std::io::Read for MtpStream {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let want = buf.len().min(u32::MAX as usize) as u32;
            let mut got = 0u32;
            // SAFETY: buf 可写长度 ≥ want
            let hr = unsafe {
                self.stream.Read(
                    buf.as_mut_ptr() as *mut core::ffi::c_void,
                    want,
                    Some(&mut got),
                )
            };
            if hr.is_err() {
                let kind = match hr_error(hr) {
                    DeviceError::AccessDenied => std::io::ErrorKind::PermissionDenied,
                    DeviceError::Disconnected => std::io::ErrorKind::ConnectionAborted,
                    _ => std::io::ErrorKind::Other,
                };
                return Err(std::io::Error::new(
                    kind,
                    format!("MTP stream error 0x{:08X}", hr.0 as u32),
                ));
            }
            Ok(got as usize)
        }
    }
}
