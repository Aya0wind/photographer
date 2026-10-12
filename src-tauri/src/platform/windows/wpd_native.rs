// Included in wpd_backend::com::native. COM objects remain on their owning worker.
use super::super::super::backend::{CameraInfo, CameraSetting, CapturedObject, LiveViewFrame};
use super::super::super::ptp_control as ptp;
use super::super::super::receipts::Receipts;
use super::super::super::vendor_dispatch::{self, LiveViewSession};
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

#[derive(Default)]
struct Notice {
    receipts: Mutex<Receipts>,
    changed: Condvar,
    /// 设置失效信号（§B7）：设备属性变更事件在回调里只递增此值并唤醒，
    /// 不做任何相机读取；会话层对比 revision 后按需拉取快照。
    settings_invalidated: AtomicU64,
}
fn notices() -> &'static Mutex<HashMap<String, Arc<Notice>>> {
    static NOTICES: OnceLock<Mutex<HashMap<String, Arc<Notice>>>> = OnceLock::new();
    NOTICES.get_or_init(Default::default)
}
fn notice(id: &str) -> Arc<Notice> {
    notices()
        .lock()
        .unwrap()
        .entry(crate::devices::normalize_device_id(id))
        .or_default()
        .clone()
}

struct Connection {
    device: IPortableDevice,
    /// Vendor handshake 后的完整操作码表（Sony 扩展码 0x9202 之前不可见）。
    operations: Vec<u16>,
    brand: ptp::Brand,
    info: CameraInfo,
    notify: Arc<Notice>,
    cookie: String,
    /// 厂商实时取景会话（None = 该相机不支持取景）。
    preview: Option<Box<dyn LiveViewSession>>,
    /// 最近一帧的取景头部（画面尺寸/AF 框；点击对焦坐标换算依据）。
    live_header: Option<ptp::LiveViewHeader>,
    // Extra COM initialization keeps the apartment alive through TLS destruction.
    _com: wpd_com::ComApartment,
}
thread_local! { static CONNECTIONS: RefCell<HashMap<String, Connection>> = RefCell::new(HashMap::new()); }
impl Drop for Connection {
    fn drop(&mut self) {
        let mut transport = super::control::WpdTransport {
            device: self.device.clone(),
        };
        if let Some(preview) = self.preview.as_mut() {
            if let Err(error) = preview.stop(&mut transport) {
                crate::devices::diagnostics::record(format!("Native LiveView cleanup: {error}"));
            }
        }
        let cookie = wpd_com::to_wide(&self.cookie);
        // SAFETY: the device and cookie belong to this worker; COM is still initialized.
        if let Err(error) = unsafe { self.device.Unadvise(windows::core::PCWSTR(cookie.as_ptr())) }
        {
            crate::devices::diagnostics::record(format!("Native event cleanup: {error}"));
        }
        let _ = unsafe { self.device.Close() };
    }
}

#[implement(IPortableDeviceEventCallback)]
struct NativeEvents {
    notify: Arc<Notice>,
}
impl IPortableDeviceEventCallback_Impl for NativeEvents_Impl {
    fn OnEvent(&self, values: Ref<'_, IPortableDeviceValues>) -> windows::core::Result<()> {
        let Some(values) = values.as_ref() else {
            return Ok(());
        };
        // SAFETY: values is the driver-owned event payload for this callback invocation.
        let event = unsafe { values.GetGuidValue(&WPD_EVENT_PARAMETER_EVENT_ID) }.ok();
        if event == Some(WPD_EVENT_OBJECT_ADDED) {
            if let Some(id) =
                wpd_com::string_prop(values, &WPD_OBJECT_ID).filter(|id| !id.is_empty())
            {
                self.notify
                    .receipts
                    .lock()
                    .unwrap()
                    .enqueue(id, Instant::now());
                self.notify.changed.notify_all();
            }
        } else if is_settings_invalidated_event(event) {
            // §B7 设置收敛：设备属性变更（MTP DevicePropChanged 由类驱动以
            // 通用通知事件上报）只投「有变化」信号进 FIFO——回调内零读取；
            // 具体哪个参数变了由会话层拉快照时按失效集合合并。
            self.notify
                .settings_invalidated
                .fetch_add(1, Ordering::AcqRel);
        }
        Ok(())
    }
}

/// 判定事件是否应标记设置失效。WPD 没有专用的「设备属性已变更」事件
/// GUID；类驱动把 PTP DevicePropChanged 一类的属性通知以通用
/// WPD_EVENT_NOTIFICATION 上报，能力表更新事件同样视为失效信号（保守：
/// 多失效一次只多一次合并读取，不丢变更）。
fn is_settings_invalidated_event(event: Option<windows::core::GUID>) -> bool {
    matches!(
        event,
        Some(WPD_EVENT_NOTIFICATION) | Some(WPD_EVENT_DEVICE_CAPABILITIES_UPDATED)
    )
}

/// 参数事件序号（会话层轮询对比；无原生连接时恒 0）。
pub(crate) fn settings_revision(id: &str) -> u64 {
    notice(id).settings_invalidated.load(Ordering::Acquire)
}

pub(crate) fn connect(id: &str) -> DeviceResult<CameraInfo> {
    CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        if connections.contains_key(id) {
            return Err(DeviceError::Other("相机已有原生连接".into()));
        }
        let apartment = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
        let info = probe_device(id)?;
        let device = wpd_com::open_device(id)?;
        let (response, data) =
            execute_with_data_to_read(&device, mtp::PTP_OP_GET_DEVICE_INFO, &[])?;
        ptp::require_ok(response, mtp::PTP_OP_GET_DEVICE_INFO).map_err(DeviceError::Other)?;
        let parsed =
            mtp::parse_device_info(&data).map_err(|e| DeviceError::Other(e.to_string()))?;
        let brand = ptp::detect_brand(
            &parsed.manufacturer,
            &parsed.model,
            &parsed.operations_supported,
        );
        let vendor = vendor_dispatch::profile(brand);
        // 厂商握手展开隐藏操作码表（Sony）；失败退回原始表——能力按保守
        // 值报告，取景/参数路径使用时仍会幂等重握手，不因探测失败锁死。
        let mut session_transport = super::control::WpdTransport {
            device: device.clone(),
        };
        let operations =
            match vendor.expand_operations(&mut session_transport, &parsed.operations_supported) {
                Ok(operations) => operations,
                Err(error) => {
                    crate::devices::diagnostics::record(format!(
                        "Vendor session expansion failed; using raw operations: {error}"
                    ));
                    parsed.operations_supported.clone()
                }
            };
        let preview = vendor.live_view_session(&operations);
        let notify = notice(id);
        let callback: IPortableDeviceEventCallback = NativeEvents {
            notify: notify.clone(),
        }
        .into();
        // SAFETY: initialized COM, local values collection; callback contains no COM handles.
        let parameters: IPortableDeviceValues = unsafe {
            CoCreateInstance(
                &wpd_com::CLSID_PORTABLE_DEVICE_VALUES,
                None::<&windows::core::IUnknown>,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(wpd_com::win_error)?;
        let pointer =
            unsafe { device.Advise(0, &callback, &parameters) }.map_err(wpd_com::win_error)?;
        let cookie = unsafe { wpd_com::pwstr_to_string(pointer) };
        unsafe { wpd_com::free_pwstr(pointer) };
        connections.insert(
            id.into(),
            Connection {
                device,
                operations,
                brand,
                info: info.clone(),
                notify,
                cookie,
                preview,
                live_header: None,
                _com: apartment,
            },
        );
        Ok(info)
    })
}
fn with<T>(
    id: &str,
    operation: impl FnOnce(&mut Connection) -> DeviceResult<T>,
) -> DeviceResult<T> {
    CONNECTIONS.with(|connections| {
        let mut connections = connections.borrow_mut();
        let connection = connections.get_mut(id).ok_or(DeviceError::Disconnected)?;
        operation(connection)
    })
}

impl Connection {
    fn poll(&mut self) -> DeviceResult<Vec<CapturedObject>> {
        loop {
            let Some(id) = self.notify.receipts.lock().unwrap().next(Instant::now()) else {
                return Ok(Vec::new());
            };
            let object = enrich_object(&self.device, &id)?;
            if !crate::devices::is_media_ext(object.object_name.rsplit('.').next().unwrap_or("")) {
                if !is_container(&self.device, &id)?
                    && (object.object_name == id || !object.object_name.contains('.'))
                {
                    return Ok(Vec::new()); // Filename metadata is not ready; retain the receipt.
                }
                // Directory/new storage events must not swallow the following image event.
                self.notify.receipts.lock().unwrap().acknowledge(&id);
                continue;
            }
            if object.object_size == 0 {
                return Ok(Vec::new());
            } // not fully written yet; keep receipt
            return Ok(vec![object]);
        }
    }
}
pub(crate) fn poll(id: &str) -> DeviceResult<Vec<CapturedObject>> {
    with(id, Connection::poll)
}
pub(crate) fn acknowledge(id: &str, object: &str) {
    notice(id).receipts.lock().unwrap().acknowledge(object);
}

/// 待接收对象数（会话快照「N 待收」真值；进程级 FIFO 直读，无需 COM）。
pub(crate) fn pending_receipts(id: &str) -> usize {
    notice(id).receipts.lock().unwrap().pending_count()
}

/// 崩溃/重连恢复（§B）：按对象 id 重新入收片队列。校验沿用 poll 的对象
/// 读取路径（enrich_object 批量属性查询）：对象仍在设备上、是媒体文件、
/// 已写完整才入队；对象已消失/不可读的 id 不返回（调用方退回事件驱动
/// 等待）。WPD 对象 id 为会话瞬态——进程重启后失效是预期形态。
pub(crate) fn request_objects(id: &str, ids: &[String]) -> DeviceResult<Vec<String>> {
    with(id, |connection| {
        let mut requeued = Vec::new();
        for object_id in ids {
            let Ok(object) = enrich_object(&connection.device, object_id) else {
                continue; // 对象已不在（瞬态 id / 已删）：跳过，不阻断其余恢复
            };
            if object.object_size == 0 {
                continue; // 尚未写完整：留给事件驱动路径
            }
            if !crate::devices::is_media_ext(object.object_name.rsplit('.').next().unwrap_or("")) {
                continue;
            }
            connection
                .notify
                .receipts
                .lock()
                .unwrap()
                .enqueue(object_id.clone(), Instant::now());
            requeued.push(object_id.clone());
        }
        Ok(requeued)
    })
}
pub(crate) fn close(id: &str) -> DeviceResult<()> {
    CONNECTIONS.with(|connections| {
        connections.borrow_mut().remove(id);
    });
    // Pending receipts are intentionally retained across a session close/reopen.
    Ok(())
}

pub(crate) fn capture(id: &str, timeout: Duration) -> DeviceResult<Vec<CapturedObject>> {
    with(id, |connection| {
        let sequence = connection.notify.receipts.lock().unwrap().sequence();
        // 厂商拍摄序列（Sony 半按→全按→等状态→释放）；对象仍统一经
        // Advise→receipt FIFO 到达，与机身快门同路径。无厂商序列时走标准 MTP 拍摄。
        let vendor = vendor_dispatch::profile(connection.brand);
        if vendor.vendor_capture(&connection.operations) {
            let mut transport = super::control::WpdTransport {
                device: connection.device.clone(),
            };
            vendor
                .capture(&mut transport)
                .map_err(DeviceError::Other)?;
        } else {
            let command = mtp::capture_command_for_operations(&connection.operations)
                .map_err(|e| DeviceError::Other(e.to_string()))?;
            let response = execute_without_data(&connection.device, command.code, &command.params)?;
            ptp::require_ok(response, command.code).map_err(DeviceError::Other)?;
        }
        let deadline = Instant::now() + timeout;
        let mut receipts = connection.notify.receipts.lock().unwrap();
        while receipts.sequence() == sequence {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(DeviceError::Other("相机未响应拍摄命令".into()));
            }
            receipts = connection
                .notify
                .changed
                .wait_timeout(receipts, remaining)
                .unwrap()
                .0;
        }
        drop(receipts);
        // Return one receipt; the persistent subscription keeps the rest (RAW+JPG/bursts).
        connection.poll()
    })
}

/// 参数刷新（原生连接路径）：复用连接上已打开的设备与握手时缓存的
/// 操作码表——省去每次重开设备 + 全量 GetDeviceInfo（取景期间属性变更
/// 频繁触发刷新，这两步是通道占用的主要部分）。描述符仍逐次读取：
/// 可写性随机身模式（A/S ↔ M）变化，缓存会吞掉模式切换的显示变化。
pub(crate) fn settings(id: &str) -> DeviceResult<Vec<CameraSetting>> {
    with(id, |connection| {
        let profile = super::control::DeviceProfile::from_parts(
            connection.brand,
            connection.operations.clone(),
        );
        let mut transport = super::control::WpdTransport {
            device: connection.device.clone(),
        };
        super::control::camera_settings(&mut transport, &profile).map_err(DeviceError::Other)
    })
}

pub(crate) fn frame(id: &str) -> DeviceResult<LiveViewFrame> {    with(id, |connection| {
        if !connection.info.capabilities.live_view() {
            return Err(DeviceError::NotSupported("实时取景".into()));
        }
        let Some(preview) = connection.preview.as_mut() else {
            return Err(DeviceError::NotSupported("实时取景".into()));
        };
        let mut transport = super::control::WpdTransport {
            device: connection.device.clone(),
        };
        let jpeg = preview.frame(&mut transport).map_err(DeviceError::Other)?;
        // 帧头（画面尺寸/AF 框）随帧缓存——点击对焦按最新一帧的坐标系换算，
        // AF 框随帧带给前端叠加显示（相机回报的当前对焦区域）。
        connection.live_header = preview.last_header();
        Ok(LiveViewFrame {
            af_region: connection
                .live_header
                .as_ref()
                .and_then(|header| header.af_regions.first().copied()),
            jpeg,
        })
    })
}

/// 点击对焦（原生连接路径）：会话缓存的最近帧头部把归一化坐标换算成
/// 厂商协议坐标（Nikon = 头部 4/6 全图像素空间），选区后再驱动 AF。
pub(crate) fn focus_at(id: &str, x: f64, y: f64) -> DeviceResult<()> {
    with(id, |connection| {
        let vendor = vendor_dispatch::profile(connection.brand);
        let mut transport = super::control::WpdTransport {
            device: connection.device.clone(),
        };
        vendor
            .focus_at(
                &mut transport,
                &connection.operations,
                x,
                y,
                connection.live_header.as_ref(),
            )
            .map_err(DeviceError::Other)
    })
}

pub(crate) fn capture_once(id: &str, timeout: Duration) -> DeviceResult<CapturedObject> {
    let temporary = CONNECTIONS.with(|connections| !connections.borrow().contains_key(id));
    if temporary {
        connect(id)?;
    }
    let result = capture(id, timeout).and_then(|objects| {
        objects
            .into_iter()
            .next()
            .ok_or_else(|| DeviceError::Other("拍摄已触发，照片仍在接收队列中".into()))
    });
    if temporary {
        close(id)?;
    }
    result
}

fn is_container(device: &IPortableDevice, id: &str) -> DeviceResult<bool> {
    use windows::Win32::Devices::PortableDevices::{
        WPD_CONTENT_TYPE_FOLDER, WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT, WPD_OBJECT_CONTENT_TYPE,
    };
    let content = unsafe { device.Content() }.map_err(wpd_com::win_error)?;
    let properties = unsafe { content.Properties() }.map_err(wpd_com::win_error)?;
    let wide = wpd_com::to_wide(id);
    // SAFETY: device is open; id buffer remains alive through this synchronous call.
    let values = unsafe { properties.GetValues(windows::core::PCWSTR(wide.as_ptr()), None) }
        .map_err(wpd_com::win_error)?;
    Ok(unsafe { values.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE) }
        .ok()
        .is_some_and(|kind| {
            kind == WPD_CONTENT_TYPE_FOLDER || kind == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT
        }))
}

#[cfg(test)]
mod hardware_tests {
    #[test]
    #[ignore = "starts/stops LiveView on an explicitly selected camera; does not shoot"]
    fn live_native_preview() {
        use super::super::super::super::backend::CameraBackend;
        use super::super::super::WpdMtpBackend;
        let id = std::env::var("PHOTOGRAPHER_TEST_CAMERA").expect("set PHOTOGRAPHER_TEST_CAMERA");
        let backend = WpdMtpBackend::new();
        backend.connect(&id).unwrap();
        let result = backend.live_view_frame(&id);
        backend.disconnect(&id); // Restore owned state even if preview validation fails.
        let frame = result.unwrap();
        let dimensions = image::ImageReader::new(std::io::Cursor::new(frame.jpeg))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert!(dimensions.0 > 0 && dimensions.1 > 0);
        println!("Native preview dimensions: {dimensions:?}");
    }
}
