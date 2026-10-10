//! WPD/MTP 相机源（Windows Portable Devices COM）。
//!
//! 结构：跨平台纯函数（路径拼接 / OLE DATE / ISO 日期 / FILETIME 解析，
//! 有单测）+ Windows COM 实现 + 非 Windows 桩。
//!
//! COM 对象在拥有它的 worker 线程内创建、调用与释放。WpdSource 是
//! 无 COM 状态的代理；设备枚举、每台设备的数据操作与探活分别排队。
//! 等待有超时，旧连接调用被隔离，恢复和线程数量限制见 timed_worker。

use super::{DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

// ---------------------------------------------------------------------------
// 纯函数（跨平台，单测见 tests/devices_test.rs）
// ---------------------------------------------------------------------------

pub use super::wpd_helpers::*;

// ---------------------------------------------------------------------------
// 设备源
// ---------------------------------------------------------------------------

/// WPD/MTP 设备源——**轻量代理**（统一架构 2026-09-18）：`pnp_id` 为 PnP
/// 设备路径（enumerate_mtp_devices 的产出），`friendly_name` 为展示名。
/// 不持有任何 COM 资源；list/open_head/stream/delete 全部转发给全局
/// wpd-worker 线程（见 `worker` 模块），任意线程持有/drop 均安全。
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

/// 流读取适配：worker 泵线程 → 有界通道 → `std::io::Read`。reader 在任意
/// 线程 drop 即关通道 → 泵线程退出并释放全部 COM（`WPD_RELEASE_COUNT`
/// 递增可观测）。
pub(crate) struct MtpChannelReader {
    rx: tokio::sync::mpsc::Receiver<StreamChunk>,
    buf: Vec<u8>,
    pos: usize,
}

impl std::io::Read for MtpChannelReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }
        if self.pos >= self.buf.len() {
            match self.rx.blocking_recv() {
                // 通道关闭（EOF 或泵线程结束）
                None => return Ok(0),
                Some(Err(e)) => return Err(e),
                Some(Ok(chunk)) => {
                    self.buf = chunk;
                    self.pos = 0;
                }
            }
        }
        let n = (self.buf.len() - self.pos).min(out.len());
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// 流分块（worker 泵线程 → reader）。
pub type StreamChunk = Result<Vec<u8>, std::io::Error>;

/// COM 释放计数（流泵线程释放会话时递增；测试/运维观测用）。
pub static WPD_RELEASE_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl DeviceSource for WpdSource {
    fn thumbnail(&self, id: &str) -> DeviceResult<Option<Vec<u8>>> {
        worker::thumbnail(&self.pnp_id, id)
    }
    /// 规范化（小写）的 PnP id：启动枚举（小写）与热插到达（大写）两种
    /// 形式收敛为同一标识（注册表 key / jobs.device_id 单一事实源）。
    /// COM 调用仍用原始 `pnp_id` 串（Windows 设备路径大小写不敏感）。
    fn id(&self) -> String {
        super::normalize_device_id(&self.pnp_id)
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Mtp
    }

    fn name(&self) -> String {
        self.friendly_name.clone()
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        worker::list(&self.pnp_id, None)
    }

    fn list_with_progress(
        &self,
        on_batch: super::FileBatchCallback,
    ) -> DeviceResult<Vec<FileEntry>> {
        worker::list(&self.pnp_id, Some(on_batch))
    }

    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        worker::open_head(&self.pnp_id, id, max)
    }

    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        let rx = worker::stream(&self.pnp_id, id)?;
        Ok(Box::new(MtpChannelReader {
            rx,
            buf: Vec::new(),
            pos: 0,
        }))
    }

    /// MTP 删源（move 模式）：经 WPD Delete（worker 线程执行）。
    fn delete(&self, id: &str) -> DeviceResult<()> {
        worker::delete(&self.pnp_id, id)
    }
}

// ---------------------------------------------------------------------------
// Windows COM 实现
// ---------------------------------------------------------------------------

pub fn enumerate_mtp_devices() -> DeviceResult<Vec<(String, String)>> {
    worker::enumerate()
}

/// 探活（健康监控接线）：worker 打开会话+列举顶层对象，5s 超时。
pub fn worker_ping(pnp: &str) -> Option<bool> {
    worker::ping(pnp, std::time::Duration::from_secs(5))
}

/// 断开时隔离旧连接的排队调用；重连不等待旧驱动调用返回。
pub fn invalidate_device(pnp: &str) {
    worker::invalidate(pnp);
}

/// 联拍（tethering）COM 操作调度：复用同一 worker 池（COM 生命周期归宿
/// worker 线程、超时与旧连接隔离同套语义）。`data:{pnp}` 通道与导入互斥
/// （WPD 会话独占），`probe:{pnp}` 通道与数据操作分离。
pub(crate) fn schedule_com_operation<T: Send + 'static>(
    lane: &str,
    timeout: std::time::Duration,
    operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
) -> DeviceResult<T> {
    worker::schedule(lane, timeout, operation)
}

/// 测试注入：验证 worker 命令 panic 被捕获（进程存活、后续命令可用）。
/// lib 目标内无调用点（集成测试经 #[path] 模块树引用）——豁免 dead_code。
#[doc(hidden)]
#[allow(dead_code)]
pub fn panic_probe() -> DeviceResult<()> {
    worker::panic_probe()
}

// ---------------------------------------------------------------------------
// WPD worker：单一常驻线程，全部 COM 对象的生命周期归宿
// ---------------------------------------------------------------------------

mod worker {
    use super::super::{DeviceResult, FileEntry};
    use super::{com, StreamChunk};
    use std::sync::OnceLock;
    use std::time::Duration;
    fn pool() -> &'static super::super::timed_worker::WorkerPool {
        static POOL: OnceLock<super::super::timed_worker::WorkerPool> = OnceLock::new();
        POOL.get_or_init(|| {
            super::super::timed_worker::WorkerPool::with_initializer(|| {
                Box::new(com::ComApartment::init())
            })
        })
    }
    pub(super) fn invalidate(pnp: &str) {
        pool().invalidate(&format!("data:{pnp}"));
        pool().invalidate(&format!("probe:{pnp}"));
    }
    pub(super) fn enumerate() -> DeviceResult<Vec<(String, String)>> {
        pool().call("enumerate", Duration::from_secs(5), com::enumerate)
    }
    pub(super) fn list(
        pnp: &str,
        on_batch: Option<super::super::FileBatchCallback>,
    ) -> DeviceResult<Vec<FileEntry>> {
        let id = pnp.to_owned();
        pool().call_with_idle_timeout(&format!("data:{pnp}"), Duration::from_secs(30), move || {
            com::list(&id, on_batch)
        })
    }
    pub(super) fn open_head(pnp: &str, obj: &str, max: u64) -> DeviceResult<Vec<u8>> {
        let (id, obj) = (pnp.to_owned(), obj.to_owned());
        pool().call(&format!("data:{pnp}"), Duration::from_secs(30), move || {
            com::open_head(&id, &obj, max)
        })
    }
    pub(super) fn thumbnail(pnp: &str, obj: &str) -> DeviceResult<Option<Vec<u8>>> {
        let (id, obj) = (pnp.to_owned(), obj.to_owned());
        pool().call(&format!("data:{pnp}"), Duration::from_secs(30), move || {
            com::thumbnail(&id, &obj)
        })
    }
    pub(super) fn stream(
        pnp: &str,
        obj: &str,
    ) -> DeviceResult<tokio::sync::mpsc::Receiver<StreamChunk>> {
        let (id, obj) = (pnp.to_owned(), obj.to_owned());
        pool().call(&format!("data:{pnp}"), Duration::from_secs(30), move || {
            com::stream_channel(&id, &obj)
        })
    }
    pub(super) fn delete(pnp: &str, obj: &str) -> DeviceResult<()> {
        let (id, obj) = (pnp.to_owned(), obj.to_owned());
        pool().call(&format!("data:{pnp}"), Duration::from_secs(30), move || {
            com::delete(&id, &obj)
        })
    }
    pub(super) fn ping(pnp: &str, timeout: Duration) -> Option<bool> {
        let id = pnp.to_owned();
        match pool().call_if_idle(&format!("data:{pnp}"), timeout, move || com::ping(&id)) {
            Ok(Some(())) => Some(true),
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }
    #[allow(dead_code)]
    pub(super) fn panic_probe() -> DeviceResult<()> {
        pool().call("panic-test", Duration::from_secs(1), || {
            panic!("注入测试 panic")
        })
    }
    /// 通用调度（tethering 等新调用方复用 worker 池；lane 命名由调用方
    /// 决定，与既有 data:/probe: 通道语义一致）。
    pub(super) fn schedule<T: Send + 'static>(
        lane: &str,
        timeout: Duration,
        operation: impl FnOnce() -> DeviceResult<T> + Send + 'static,
    ) -> DeviceResult<T> {
        pool().call(lane, timeout, operation)
    }
}

pub(crate) mod com {
    use std::sync::atomic::Ordering;

    use chrono::{DateTime, Utc};

    use super::super::{is_media_ext, DeviceError, DeviceResult, FileEntry};
    use super::{
        filetime_to_utc, is_object_level_skip, join_rel_path, ole_date_to_utc,
        parse_wpd_date_string,
    };
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
    /// tethering（SendCommand 参数组装）复用。
    pub(crate) const CLSID_PORTABLE_DEVICE_VALUES: GUID =
        GUID::from_u128(0x0c15d503_d017_47ce_9016_7b3f978721cc);
    /// tethering（新对象属性批量读）复用。
    pub(crate) const CLSID_PORTABLE_DEVICE_KEY_COLLECTION: GUID =
        GUID::from_u128(0xde2d022d_2480_43be_97f0_d1fa2cf98f4f);
    /// tethering（MTP 操作码参数表）复用。
    pub(crate) const CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION: GUID =
        GUID::from_u128(0x08a99e2f_6d6d_4b80_af5a_baf2bcbe4cb9);

    /// HRESULT → 设备错误语义（spec §5.1：UI 需区分“可提示操作设备”与“拔线”）。
    pub(crate) fn hr_error(hr: HRESULT) -> DeviceError {
        match hr.0 as u32 {
            0x8007_0005 => DeviceError::AccessDenied, // E_ACCESSDENIED：相机未切 PC 模式 / 手机锁定
            0x8007_0015 | 0x8007_048F | 0x8007_04C7 => DeviceError::Disconnected, // NOT_READY / DEVICE_NOT_CONNECTED / CANCELLED：传输中拔线
            _ => DeviceError::Other(format!("WPD error 0x{:08X}", hr.0 as u32)),
        }
    }

    pub(crate) fn win_error(e: WinError) -> DeviceError {
        hr_error(e.code())
    }

    /// 每线程 COM apartment guard（三态，决策逻辑见
    /// [`super::com_apartment_owned`]，纯函数单测覆盖）。
    ///
    /// - `owned=true`：本线程 CoInitializeEx 首次初始化成功（S_OK）——
    ///   drop 时配对一次 `CoUninitialize`；
    /// - `owned=false`：S_FALSE（线程已初始化，他人持有引用）或
    ///   RPC_E_CHANGED_MODE（线程为 STA——tao 事件循环把 Tauri IPC 命令
    ///   线程/主线程初始化为 STA；WPD 在 STA 上调用合法，Windows
    ///   Explorer 同款用法）——沿用现有 apartment，**绝不
    ///   CoUninitialize**（不注销他人/宿主的初始化，泄漏的引用至多延长
    ///   apartment 生命周期到进程退出，无害）。
    pub(crate) struct ComApartment {
        owned: bool,
    }

    impl ComApartment {
        pub(crate) fn init() -> Result<Self, WinError> {
            // SAFETY: 无指针参数，仅改变当前线程 COM 状态
            let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            match super::com_apartment_owned(hr.0) {
                Ok(owned) => Ok(Self { owned }),
                Err(hr) => Err(WinError::from(HRESULT(hr))),
            }
        }
    }

    impl Drop for ComApartment {
        fn drop(&mut self) {
            if self.owned {
                // SAFETY: 仅与本 guard 自己的 S_OK CoInitializeEx 配对
                unsafe { CoUninitialize() };
            }
        }
    }

    pub(crate) fn to_wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// 拷贝 PWSTR → String（不取得所有权；释放由调用方负责）。
    ///
    /// SAFETY: p 必须为 NULL 或指向 NUL 结尾的合法宽字符串。
    pub(crate) unsafe fn pwstr_to_string(p: PWSTR) -> String {
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
    pub(crate) unsafe fn free_pwstr(p: PWSTR) {
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
                if !super::is_volume_alias(&name) {
                    out.push((pnp, name));
                }
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
    /// 流泵分块大小（1MB：内存可控，导入侧 8MB 读循环多次接收）。
    const STREAM_CHUNK_BYTES: usize = 1024 * 1024;
    /// 流泵有界通道缓冲块数（8MB 在途；背压阻塞泵线程而非 worker）。
    const STREAM_BUFFER_CHUNKS: usize = 8;

    struct ScanProgress {
        callback: Option<super::super::FileBatchCallback>,
        sent: usize,
        last_emit: std::time::Instant,
    }
    impl ScanProgress {
        fn flush(&mut self, files: &[FileEntry], force: bool) {
            // 首个文件立即显示；之后按时间合并，避免高速枚举按文件数触发大量 UI 更新。
            // 扫描结束强制送出最后一批，不丢尾部文件。
            if files.len() > self.sent
                && (force
                    || self.sent == 0
                    || self.last_emit.elapsed() >= std::time::Duration::from_millis(150))
            {
                if let Some(callback) = &self.callback {
                    callback(files[self.sent..].to_vec());
                }
                self.sent = files.len();
                self.last_emit = std::time::Instant::now();
            }
        }
    }
    pub fn list(
        pnp_id: &str,
        on_batch: Option<super::super::FileBatchCallback>,
    ) -> DeviceResult<Vec<FileEntry>> {
        let _com = ComApartment::init().map_err(win_error)?;
        super::super::diagnostics::record(format!("WPD open begin: {pnp_id}"));
        let device = open_device(pnp_id)?;
        super::super::diagnostics::record("WPD open complete; enumerating media");
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let keys = build_property_keys()?;
        let mut out = Vec::new();
        let mut skipped = 0u32;
        let mut progress = ScanProgress {
            callback: on_batch,
            sent: 0,
            last_emit: std::time::Instant::now(),
        };
        walk_folder(
            &content,
            &keys,
            WPD_DEVICE_OBJECT_ID,
            "",
            0,
            &mut out,
            &mut skipped,
            &mut progress,
        )?;
        progress.flush(&out, true);
        if out.is_empty() {
            if let Some(callback) = &progress.callback {
                callback(Vec::new());
            }
        }
        // 「要么完整要么报错」：中途错误已整体上抛；此处只剩受限对象计数
        if skipped > 0 {
            eprintln!(
                "WPD 枚举跳过 {skipped} 个受限/无名对象（访问被拒或缺基本属性，                 如播放列表/系统对象），其余完整返回"
            );
        }
        super::super::diagnostics::record(format!("WPD list complete: {} files", out.len()));
        out.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
    }

    pub(crate) fn open_device(pnp_id: &str) -> DeviceResult<IPortableDevice> {
        super::super::timed_worker::checkpoint()?;
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
            client_info.SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Photographer"))
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
    #[allow(clippy::too_many_arguments)]
    fn walk_folder(
        content: &IPortableDeviceContent,
        keys: &IPortableDeviceKeyCollection,
        folder: PCWSTR,
        prefix: &str,
        depth: u32,
        out: &mut Vec<FileEntry>,
        skipped: &mut u32,
        progress: &mut ScanProgress,
    ) -> DeviceResult<()> {
        super::super::timed_worker::checkpoint()?;
        if depth > MAX_DEPTH {
            eprintln!("WPD 枚举达到深度上限 {MAX_DEPTH}，截断该子树（防御异常设备）");
            return Ok(());
        }
        // SAFETY: folder 为有效对象 ID；无过滤条件（None = 全部子对象）
        let enumerator = unsafe { content.EnumObjects(0, folder, None::<&IPortableDeviceValues>) }
            .map_err(win_error)?;
        loop {
            super::super::timed_worker::checkpoint()?;
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
            // 先释放整批系统字符串，防止子树报错/取消时遗留后半批。
            let ids: Vec<_> = batch[..fetched as usize]
                .iter()
                .map(|&id_ptr| {
                    let id = unsafe { pwstr_to_string(id_ptr) };
                    unsafe { free_pwstr(id_ptr) };
                    id
                })
                .collect();
            for obj_id in ids {
                if obj_id.is_empty() {
                    continue;
                }
                // 「要么完整要么报错」（2026-09-18 真机：清单随机截断 562/1161/2349
                // 的根因——此处曾把一切对象级错误静默吞掉，枚举中途失败即返回
                // 部分结果）。现在：仅受限对象/子树（AccessDenied，如相机受保护
                // 目录、并发会话被拒）跳过并计数（收尾日志可见）；其余错误
                // （传输中断/会话失效等）整体上抛，绝不出半份清单。
                if let Err(err) = process_object(
                    content, keys, &obj_id, prefix, depth, out, skipped, progress,
                ) {
                    if is_object_level_skip(&err) {
                        *skipped += 1;
                    } else {
                        return Err(err);
                    }
                }
                progress.flush(out, false);
            }
            if fetched < BATCH as u32 {
                break; // S_FALSE：本次不足一批，枚举已尽
            }
        }
        Ok(())
    }

    /// 处理单个对象：目录则递归；媒体文件则产出 FileEntry。
    #[allow(clippy::too_many_arguments)]
    fn process_object(
        content: &IPortableDeviceContent,
        keys: &IPortableDeviceKeyCollection,
        obj_id: &str,
        prefix: &str,
        depth: u32,
        out: &mut Vec<FileEntry>,
        skipped: &mut u32,
        progress: &mut ScanProgress,
    ) -> DeviceResult<()> {
        // SAFETY: 设备已 Open
        super::super::timed_worker::checkpoint()?;
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
                skipped,
                progress,
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
                skipped,
                progress,
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
    pub(crate) fn string_prop(
        values: &IPortableDeviceValues,
        key: *const PROPERTYKEY,
    ) -> Option<String> {
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

    /// 轻量探活：打开会话 + 列举顶层 1 个对象（能走到 EnumObjects/Next
    /// 即视为可达；不关心对象本身）。
    pub fn ping(pnp_id: &str) -> DeviceResult<()> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        // SAFETY: 设备根为合法对象 ID；无过滤条件
        let enumerator =
            unsafe { content.EnumObjects(0, WPD_DEVICE_OBJECT_ID, None::<&IPortableDeviceValues>) }
                .map_err(win_error)?;
        let mut one = [PWSTR::null(); 1];
        let mut fetched = 0u32;
        // SAFETY: 缓冲与请求条数一致
        let result = unsafe { enumerator.Next(&mut one, &mut fetched) };
        // SAFETY: Next 分配的字符串（若有）由调用方释放
        if fetched > 0 {
            unsafe { free_pwstr(one[0]) };
        }
        result.ok().map_err(win_error)?;
        Ok(())
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

    /// 只读 WPD 缩略资源。部分相机不提供该资源；此时明确返回无预览。
    pub fn thumbnail(pnp_id: &str, obj_id: &str) -> DeviceResult<Option<Vec<u8>>> {
        use windows::Win32::Devices::PortableDevices::WPD_RESOURCE_THUMBNAIL;
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);
        let resources = unsafe { content.Transfer() }.map_err(win_error)?;
        let wide = to_wide(&resolved);
        let mut stream = None;
        let mut optimal = 0u32;
        if let Err(error) = unsafe {
            resources.GetStream(
                PCWSTR(wide.as_ptr()),
                &WPD_RESOURCE_THUMBNAIL,
                STGM_READ.0,
                &mut optimal,
                &mut stream,
            )
        } {
            let error = win_error(error);
            return if matches!(error, DeviceError::Disconnected | DeviceError::AccessDenied) {
                Err(error)
            } else {
                Ok(None)
            };
        }
        let Some(stream) = stream else {
            return Ok(None);
        };
        const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
        let mut bytes = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            super::super::timed_worker::checkpoint()?;
            let mut got = 0u32;
            let hr = unsafe {
                stream.Read(
                    chunk.as_mut_ptr().cast(),
                    chunk.len() as u32,
                    Some(&mut got),
                )
            };
            if hr.is_err() {
                return Err(hr_error(hr));
            }
            if got == 0 {
                break;
            }
            if bytes.len() + got as usize > MAX_PREVIEW_BYTES {
                return Ok(None);
            }
            bytes.extend_from_slice(&chunk[..got as usize]);
        }
        Ok((!bytes.is_empty()).then_some(bytes))
    }

    pub fn stream_channel(
        pnp_id: &str,
        obj_id: &str,
    ) -> DeviceResult<tokio::sync::mpsc::Receiver<super::StreamChunk>> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);
        let (stream, _) = open_resource(&content, &resolved).map_err(win_error)?;

        /// COM 接口跨线程移动包装（worker → 泵线程，均为 MTA 初始化）。
        ///
        /// SAFETY: WPD 进程内 COM 对象（非跨套间代理），移动后仅在泵线程
        /// 上读取与 Release，不做其他调用（与 M1 期间 MtpStream 的 Send
        /// 论证一致）。
        struct ComMove<T>(T);
        unsafe impl<T> Send for ComMove<T> {}

        let coms = ComMove((device, stream));
        let operation_lease = super::super::timed_worker::lease_current_operation();
        let (tx, rx) = tokio::sync::mpsc::channel::<super::StreamChunk>(STREAM_BUFFER_CHUNKS);
        // 泵线程：会话与流移交本线程（MTA 初始化；in-proc 对象跨 MTA 线程
        // 合法）——分块泵入有界通道（背压：reader 消费慢/暂停时阻塞在此，
        // 不占 worker）；reader drop → 通道关闭 → 循环退出 → 在本线程释放
        // 全部 COM（WPD_RELEASE_COUNT 递增）。COM 生命周期永不落在调用方
        // 线程（断开闪退整类消灭）。
        std::thread::Builder::new()
            .name("wpd-stream".into())
            .spawn(move || {
                let _com = ComApartment::init();
                let _operation_lease = operation_lease;
                let coms = coms;
                let (_device, stream) = coms.0; // _device 保活：流关闭前不 Close
                let mut chunk = vec![0u8; STREAM_CHUNK_BYTES];
                loop {
                    let mut got = 0u32;
                    // SAFETY: chunk 可写长度 ≥ 请求长度
                    let hr = unsafe {
                        stream.Read(
                            chunk.as_mut_ptr() as *mut core::ffi::c_void,
                            chunk.len() as u32,
                            Some(&mut got),
                        )
                    };
                    if hr.is_err() {
                        let _ = tx.blocking_send(Err(hr_to_io(hr)));
                        break;
                    }
                    if got == 0 {
                        break; // EOF：tx 随作用域 drop → reader 见 Ok(0)
                    }
                    if tx
                        .blocking_send(Ok(chunk[..got as usize].to_vec()))
                        .is_err()
                    {
                        break; // reader 已 drop（取消/完成）：就此收尾释放
                    }
                }
                super::WPD_RELEASE_COUNT.fetch_add(1, Ordering::SeqCst);
            })
            .map_err(|e| DeviceError::Other(format!("流泵线程启动失败: {e}")))?;
        Ok(rx)
    }

    /// HRESULT → io::Error（读取中断映射 ConnectionAborted：调用方据此
    /// 判定拔线并暂停任务）。
    fn hr_to_io(hr: HRESULT) -> std::io::Error {
        let kind = match hr_error(hr) {
            DeviceError::AccessDenied => std::io::ErrorKind::PermissionDenied,
            DeviceError::Disconnected => std::io::ErrorKind::ConnectionAborted,
            _ => std::io::ErrorKind::Other,
        };
        std::io::Error::new(kind, format!("MTP stream error 0x{:08X}", hr.0 as u32))
    }

    /// 删除对象（move 模式删源）：持久 ID → 会话 ID 翻译后经
    /// `IPortableDeviceContent::Delete`（flags=0 非递归——文件对象无子层级，
    /// 带子对象的意外删除直接报错而非连带清除）。
    pub fn delete(pnp_id: &str, obj_id: &str) -> DeviceResult<()> {
        let _com = ComApartment::init().map_err(win_error)?;
        let device = open_device(pnp_id)?;
        // SAFETY: 设备已 Open
        let content = unsafe { device.Content() }.map_err(win_error)?;
        let resolved = resolve_object_id(&content, obj_id);

        // SAFETY: CLSID 为静态常量；无外部聚合
        let request: IPortableDevicePropVariantCollection = unsafe {
            CoCreateInstance(
                &CLSID_PORTABLE_DEVICE_PROPVARIANT_COLLECTION,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(win_error)?;
        let mut prop = lpstr_propvariant(&resolved)
            .ok_or_else(|| DeviceError::Other("构造删除请求失败".into()))?;
        // SAFETY: prop 为本地合法 VT_LPWSTR PROPVARIANT；Add 拷贝值入集合
        let added = unsafe { request.Add(&prop) }.is_ok();
        // SAFETY: prop 持有 CoTaskMem 分配的字符串，无论 Add 成败都须清理
        let _ = unsafe { PropVariantClear(&mut prop) };
        if !added {
            return Err(DeviceError::Other("构造删除请求失败".into()));
        }
        // SAFETY: request 已装载 1 个对象 ID；结果集合出参传空（整体成败
        // 由 HRESULT 表达）
        unsafe { content.Delete(0, &request, std::ptr::null_mut()) }.map_err(win_error)?;
        Ok(())
    }
}
