//! macOS ImageCaptureCore（IC）PTP 传输骨架。
//! 计划：docs/plans/2026-10-11-native-tethering.md §D（macOS 与打包）。
//!
//! # 定位
//!
//! 实现 [`crate::tethering::ptp_control::PtpTransport`]（Windows 侧
//! `WpdTransport` 的对应物），让品牌协议层（`ptp_control`/`mtp`）可以原样
//! 跑在 macOS 上。PTP 属性描述解析、写入编码、快门/光圈标签、Nikon 取景
//! 状态机全部复用 `tethering` 里的纯函数，本文件只做 OS 传输。
//!
//! # 技术路线
//!
//! IC 没有稳定的社区 Rust 绑定（crates.io 的 `image-capture-core` 停更于
//! 2019），因此用 objc2 0.6 运行时消息（`extern_class!` 自写最小类声明 +
//! `msg_send!`），objc2-foundation 提供基础类型，block2 提供 completion 块。
//! 所有 IC 选择器都对照 Apple 官方文档核对过；无法核实的语义在下面
//! TODO-verify 清单里逐条列出，运行时对不上时返回 `Err`，绝不 panic、
//! 绝不假装成功。
//!
//! # 线程 / run loop 模型
//!
//! ImageCaptureCore 的对象不是线程安全的：浏览器、设备、completion 回调
//! 都必须留在创建它们的线程的 run loop 上。因此：
//!
//! - 全部 IC 对象只存在于一个专用线程（`ic-camera-runloop`）里，
//!   线程启动即 bootstrap：取当前 run loop、`ICDeviceBrowser` new、
//!   设置相机掩码、`start`，然后进入“泵 run loop + 处理命令”循环。
//! - 其他线程通过 mpsc 通道投递命令（枚举/开会话/关会话/发 PTP 命令），
//!   并阻塞等待回执；真正的 IC 调用都在工作线程上同步执行。
//! - 泵用 `runMode:beforeDate:` 限时长切片（20ms）：既是空闲等待，也让
//!   IC 投递到本 run loop 的 completion 块得以执行。停止 = 设置
//!   `AtomicBool`，主循环在下个切片边界退出，先 `stop` 浏览器再泵 200ms
//!   收尾，最后线程自然结束（不需要 CFRunLoopStop）。
//! - 停止逻辑入口：[`shutdown_worker`]（测试/进程收尾用；应用运行期
//!   共享线程存活即可）。
//!
//! # TODO-verify 清单（未经 macOS 真机验证，接机后逐项核对）
//!
//! 1. **命令容器格式**：`requestSendPTPCommand:outData:completion:` 的
//!    `ptpCommand` 是否包含 12 字节 USB 容器头（长度/类型/码/事务号）。
//!    Apple 文档只写“PTP command data”。本实现按完整容器发送（与响应侧
//!    对称，USB PTP 标准布局）；若相机/IC 拒绝，试改为裸 opcode+参数。
//! 2. **completion 两个 NSData 的次序**：文档命名为 responseData /
//!    ptpResponseData，语义未写死。[`completion_payloads_to_result`]
//!    防御性识别“哪一个像 type-3 响应容器”，识别不出即 Err。
//! 3. **outData 是否要数据容器头**：本实现发送裸 payload；若
//!    SetDevicePropValue 写入失败且响应码怪异，试包 type-2 容器。
//! 4. **completion 回调线程/模式**：假设投递到本线程 run loop 的
//!    default 模式；若实测在别的队列，等待循环会超时报错而不是挂死。
//! 5. **无 delegate 的设备发现**：本骨架未实现 `ICDeviceBrowserDelegate`
//!    （需要 define_class!，风险高，留待真机验证后补），靠轮询 `devices`
//!    属性。若浏览器要求 delegate 才填充 devices，枚举只会得到空列表。
//! 6. **会话错误报告**：`requestOpenSession`/`requestCloseSession`（老式
//!    无 completion 版本）把错误发给 device delegate；无 delegate 时错误
//!    只表现为 `hasOpenSession` 不变 + 超时。
//! 7. **ICDeviceTypeMaskCamera = 0x1**：取自 SDK 头文件镜像（enum {
//!    ICDeviceTypeMaskCamera = 0x00000001, ICDeviceTypeMaskScanner =
//!    0x00000002 }），未在真机上验证掩码行为。
//! 8. **ptpEventHandler**：PTP 事件包（ObjectAdded 等）通道未接；收片
//!    receipt 管线接通前，macOS 侧不能宣称支持连拍收片。
//! 9. **TCC 授权**：相机控制权限（requestControlAuthorization，macOS 14+）
//!    未接；旧系统行为未验证。
//! 10. **超时泄漏**：PTP 等待超时后故意 `mem::forget` completion 块，
//!     防止晚到的回调触碰已释放内存（错误路径，量级可忽略）。
//! 11. **会话独占/多相机**：IC 会话的独占语义、同机双端点互斥未验证。
//!
//! # 本模块编译边界
//!
//! 仅在 macOS 构建（挂在 `platform/macos/mod.rs` 下）；Windows/Linux
//! 构建不含本文件。纯函数（容器编解码 + completion 载荷识别）有单元
//! 测试，但测试同样只在 macOS 上运行——Windows 侧无法执行。
#![cfg(target_os = "macos")]

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{extern_class, msg_send, ClassType};
use objc2_foundation::{NSData, NSDate, NSError, NSString, NSRunLoop};

use crate::tethering::ptp_control::PtpTransport;

/// ICDevice.h：`enum { ICDeviceTypeMaskCamera = 0x00000001, ... }`。
const IC_DEVICE_TYPE_MASK_CAMERA: usize = 0x1;

/// 空闲时每次泵 run loop 的切片。太短会空转烧 CPU，太长会拖慢命令响应。
const POLL_SLICE: Duration = Duration::from_millis(20);
/// 枚举等待设备出现的宽限期（发现是异步的）。
const DISCOVERY_GRACE: Duration = Duration::from_secs(2);
/// 会话开/关等待 `hasOpenSession` 翻转的期限。
const SESSION_TIMEOUT: Duration = Duration::from_secs(10);
/// 单条 PTP 命令等待 completion 的期限（含大图读取；TODO-verify 调优）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// 纯函数：PTP 容器编解码（可单测；ISO 15740 / USB still image 容器布局）
// ---------------------------------------------------------------------------

/// USB PTP 容器类型：1 = Command。
pub(crate) const CONTAINER_COMMAND: u16 = 1;
/// USB PTP 容器类型：2 = Data。
pub(crate) const CONTAINER_DATA: u16 = 2;
/// USB PTP 容器类型：3 = Response。
pub(crate) const CONTAINER_RESPONSE: u16 = 3;

/// 构造完整命令容器：u32 长度 / u16 类型 / u16 操作码 / u32 事务号 / 参数。
/// PTP 标准最多 5 个参数。
pub(crate) fn build_command_container(
    operation: u16,
    params: &[u32],
    transaction: u32,
) -> Result<Vec<u8>, String> {
    if params.len() > 5 {
        return Err("PTP 命令参数过多（最多 5 个）".into());
    }
    let mut bytes = Vec::with_capacity(12 + params.len() * 4);
    bytes.extend_from_slice(&((12 + params.len() * 4) as u32).to_le_bytes());
    bytes.extend_from_slice(&CONTAINER_COMMAND.to_le_bytes());
    bytes.extend_from_slice(&operation.to_le_bytes());
    bytes.extend_from_slice(&transaction.to_le_bytes());
    for param in params {
        bytes.extend_from_slice(&param.to_le_bytes());
    }
    Ok(bytes)
}

/// 解析响应容器，返回 (响应码, 事务号)。类型不是 3、长度不足或长度字段
/// 与实际不符都视为无效——用于防御性识别 completion 里的响应包。
pub(crate) fn parse_response_container(data: &[u8]) -> Result<(u32, u32), String> {
    if data.len() < 12 {
        return Err("响应容器不足 12 字节".into());
    }
    let length = u32::from_le_bytes(data[0..4].try_into().expect("4 字节"));
    let container_type = u16::from_le_bytes(data[4..6].try_into().expect("2 字节"));
    let code = u16::from_le_bytes(data[6..8].try_into().expect("2 字节"));
    let transaction = u32::from_le_bytes(data[8..12].try_into().expect("4 字节"));
    if container_type != CONTAINER_RESPONSE {
        return Err(format!("容器类型 0x{container_type:04X} 不是响应容器"));
    }
    if length as usize != data.len() {
        return Err("响应容器长度字段与实际不符".into());
    }
    Ok((code as u32, transaction))
}

/// 把 completion 给出的两段字节解释成 `PtpTransport` 结果。
///
/// TODO-verify（见模块清单 #2）：Apple 文档没有写死两个 NSData 参数的
/// 内容次序。防御性策略：哪一段能解析成 type-3 响应容器，哪段就是响应，
/// 另一段就是数据阶段字节；两段都识别不出 → 报“未核对”错误。
pub(crate) fn completion_payloads_to_result(
    first: &[u8],
    second: &[u8],
) -> Result<(u32, Vec<u8>), String> {
    if let Ok((code, _)) = parse_response_container(first) {
        return Ok((code, second.to_vec()));
    }
    if let Ok((code, _)) = parse_response_container(second) {
        return Ok((code, first.to_vec()));
    }
    Err(format!(
        "macOS IC PTP 响应容器未核对（两段 {} / {} 字节，均不符合 type-3 容器布局）",
        first.len(),
        second.len()
    ))
}

// ---------------------------------------------------------------------------
// extern 类声明（objc2 0.6）：只声明用到名字的最小集合
// ---------------------------------------------------------------------------

extern_class!(
    /// 系统成像设备基类（UUIDString/name/serialNumberString/会话）。
    #[unsafe(super(NSObject))]
    #[name = "ICDevice"]
    pub struct ICDevice;
);

extern_class!(
    /// 相机设备；`requestSendPTPCommand:outData:completion:` 所在类。
    #[unsafe(super(ICDevice, NSObject))]
    #[name = "ICCameraDevice"]
    pub struct ICCameraDevice;
);

extern_class!(
    /// 设备浏览器；持有浏览 run loop 的生命周期。
    #[unsafe(super(NSObject))]
    #[name = "ICDeviceBrowser"]
    pub struct ICDeviceBrowser;
);

// objc2 0.6 的 `ClassType::class()` 在运行时用 objc_getClass 按名查类，
// 框架没链入时会直接 panic。引用 ImageCaptureCore 的类符号把框架链进
// 二进制（与 objc2 自身 `__statics_class!` 使用的符号一致）。
#[link(name = "ImageCaptureCore", kind = "framework")]
extern "C" {
    #[link_name = "OBJC_CLASS_$_ICCameraDevice"]
    static OBJC_CLASS_ICCAMERA_DEVICE: AnyClass;
}

/// 强制链接 ImageCaptureCore。必须在第一次 `::class()` 调用前执行
/// （bootstrap 里第一行就是它）。
fn link_image_capture_core() -> &'static AnyClass {
    // SAFETY：只读取 dyld 已完成绑定的外部类符号；引用本身即让静态链接器
    // 解析该符号，从而把 ImageCaptureCore 拖入进程。
    unsafe { &OBJC_CLASS_ICCAMERA_DEVICE }
}

// ---------------------------------------------------------------------------
// 专用 run loop 线程上的 IC 工作器
// ---------------------------------------------------------------------------

/// 枚举结果：跨线程传递的纯数据快照（不含 ObjC 引用）。
#[derive(Debug, Clone)]
pub(crate) struct IcCameraInfo {
    pub(crate) uuid: String,
    pub(crate) name: String,
    pub(crate) serial: String,
    pub(crate) usb_vendor_id: i32,
    pub(crate) usb_product_id: i32,
    pub(crate) session_open: bool,
}

enum WorkerCommand {
    Enumerate {
        reply: Sender<Result<Vec<IcCameraInfo>, String>>,
    },
    OpenSession {
        uuid: String,
        reply: Sender<Result<(), String>>,
    },
    CloseSession {
        uuid: String,
        reply: Sender<Result<(), String>>,
    },
    SendPtp {
        uuid: String,
        operation: u16,
        params: Vec<u32>,
        out_data: Option<Vec<u8>>,
        limit: usize,
        reply: Sender<Result<(u32, Vec<u8>), String>>,
    },
}

/// 只活在工作线程上的全部 IC 状态。字段类型都非 `Send`，构造后永不跨线程。
struct IcWorker {
    browser: Retained<ICDeviceBrowser>,
    run_loop: Retained<NSRunLoop>,
    /// NSDefaultRunLoopMode 的底层字符串值就是 kCFRunLoopDefaultMode。
    mode: Retained<NSString>,
}

impl IcWorker {
    /// 在当前（工作）线程上创建 run loop 引用与浏览器并开始浏览。
    fn bootstrap() -> Result<Self, String> {
        // 先强制链接，否则下面按名查类会 panic。
        let _ = link_image_capture_core();
        // SAFETY: currentRunLoop 返回当前线程的 run loop（线程本地，无竞争）。
        let run_loop: Retained<NSRunLoop> =
            unsafe { msg_send![NSRunLoop::class(), currentRunLoop] };
        let mode = NSString::from_str("kCFRunLoopDefaultMode");
        // SAFETY: new = alloc + init；ICDeviceBrowser 非主线程独占类。
        let browser: Retained<ICDeviceBrowser> = unsafe { msg_send![ICDeviceBrowser::class(), new] };
        // SAFETY: setBrowsedDeviceTypeMask: 接 NSUInteger；值为
        // ICDeviceTypeMaskCamera（见模块 TODO-verify #7）。
        let _: () = unsafe { msg_send![&browser, setBrowsedDeviceTypeMask: IC_DEVICE_TYPE_MASK_CAMERA] };
        // SAFETY: start 幂等；浏览在本线程 run loop 上进行。
        let _: () = unsafe { msg_send![&browser, start] };
        Ok(Self {
            browser,
            run_loop,
            mode,
        })
    }

    /// 泵一小段 run loop：既等待 IC 回调，也充当空闲 sleep。
    fn pump(&self, slice: Duration) {
        // SAFETY: dateWithTimeIntervalSinceNow: 返回 autorelease 对象，
        // msg_send! 按 Cocoa 约定自动 retain；runMode:beforeDate: 在本线程
        // run loop 上运行 default 模式直到日期到期。返回值（是否有源）忽略。
        let date: Retained<NSDate> = unsafe {
            msg_send![
                NSDate::class(),
                dateWithTimeIntervalSinceNow: slice.as_secs_f64()
            ]
        };
        let _: bool = unsafe { msg_send![&self.run_loop, runMode: &*self.mode, beforeDate: &*date] };
    }

    /// 浏览器当前设备里的相机，保持为 `Retained<AnyObject>`（不静态下转，
    /// 类型判断用 isKindOfClass: 完成）。
    fn camera_devices(&self) -> Vec<Retained<AnyObject>> {
        // SAFETY: devices 是 ICDeviceBrowser 的 NSArray 属性；count /
        // objectAtIndex: / isKindOfClass: 均为 Foundation 标准选择器。
        let devices: Option<Retained<AnyObject>> = unsafe { msg_send![&self.browser, devices] };
        let Some(devices) = devices else {
            return Vec::new();
        };
        let count: usize = unsafe { msg_send![&devices, count] };
        let mut cameras = Vec::new();
        for index in 0..count {
            let device: Retained<AnyObject> = unsafe { msg_send![&devices, objectAtIndex: index] };
            let is_camera: bool =
                unsafe { msg_send![&device, isKindOfClass: ICCameraDevice::class()] };
            if is_camera {
                cameras.push(device);
            }
        }
        cameras
    }

    /// 每条命令都按 uuid 重新查找设备，避免持有可能已被拔掉的旧引用。
    fn find_camera(&self, uuid: &str) -> Result<Retained<AnyObject>, String> {
        self.camera_devices()
            .into_iter()
            .find(|device| device_uuid(device).as_deref() == Some(uuid))
            .ok_or_else(|| format!("macOS IC 未发现相机 {uuid}"))
    }

    fn handle(&self, command: WorkerCommand) {
        match command {
            WorkerCommand::Enumerate { reply } => {
                let _ = reply.send(self.enumerate());
            }
            WorkerCommand::OpenSession { uuid, reply } => {
                let _ = reply.send(self.session(&uuid, true));
            }
            WorkerCommand::CloseSession { uuid, reply } => {
                let _ = reply.send(self.session(&uuid, false));
            }
            WorkerCommand::SendPtp {
                uuid,
                operation,
                params,
                out_data,
                limit,
                reply,
            } => {
                let _ = reply.send((|| {
                    let device = self.find_camera(&uuid)?;
                    self.send_ptp(&device, operation, &params, out_data.as_deref(), limit)
                })());
            }
        }
    }

    /// 枚举相机：先给异步发现留一段宽限期，读到即返回。
    fn enumerate(&self) -> Result<Vec<IcCameraInfo>, String> {
        let deadline = Instant::now() + DISCOVERY_GRACE;
        loop {
            let cameras = self.camera_devices();
            if !cameras.is_empty() || Instant::now() >= deadline {
                let list = cameras
                    .iter()
                    .map(|device| {
                        // SAFETY: 只读属性选择器（Apple 文档核对），见 camera_info。
                        let open: bool = unsafe { msg_send![device, hasOpenSession] };
                        camera_info(device, open)
                    })
                    .collect();
                return Ok(list);
            }
            self.pump(POLL_SLICE);
        }
    }

    /// 打开/关闭会话：老式无 completion 的 API + 轮询 hasOpenSession。
    /// TODO-verify（模块清单 #6）：无 delegate 时错误只能表现为超时。
    fn session(&self, uuid: &str, open: bool) -> Result<(), String> {
        let device = self.find_camera(uuid)?;
        // SAFETY: requestOpenSession / requestCloseSession 是 ICDevice 的
        // void 方法（Apple 文档核对），异步生效。
        let _: () = if open {
            unsafe { msg_send![&device, requestOpenSession] }
        } else {
            unsafe { msg_send![&device, requestCloseSession] }
        };
        let deadline = Instant::now() + SESSION_TIMEOUT;
        loop {
            // SAFETY: hasOpenSession 是 BOOL 属性（msg_send! 自动转换）。
            let state: bool = unsafe { msg_send![&device, hasOpenSession] };
            if state == open {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(format!("macOS IC 会话操作超时（open={open}）"));
            }
            self.pump(POLL_SLICE);
        }
    }

    /// 单条 PTP 命令的完整生命周期：建容器 → 发送 → 泵 run loop 等
    /// completion → 防御性解析响应。
    fn send_ptp(
        &self,
        device: &Retained<AnyObject>,
        operation: u16,
        params: &[u32],
        out_data: Option<&[u8]>,
        limit: usize,
    ) -> Result<(u32, Vec<u8>), String> {
        // TODO-verify（模块清单 #1/#11）：事务号是进程级递增计数器；IC 是否
        // 要求按设备连续编号未核对。
        static NEXT_TRANSACTION: AtomicU32 = AtomicU32::new(1);
        let transaction = NEXT_TRANSACTION.fetch_add(1, Ordering::Relaxed);

        let command = build_command_container(operation, params, transaction)?;
        let command_data = NSData::with_bytes(&command);
        let out_payload = out_data.map(NSData::with_bytes);
        // outData 参数：None → nil 指针（无数据写出阶段）。
        let out_ptr: *const NSData = match &out_payload {
            Some(data) => &**data as *const NSData,
            None => std::ptr::null(),
        };

        type Completion = Result<(Vec<u8>, Vec<u8>, Option<String>), String>;
        let slot: Arc<Mutex<Option<Completion>>> = Arc::new(Mutex::new(None));
        let filler = slot.clone();
        // completion 签名（Apple 文档核对）：
        //   void (^)(NSData *responseData, NSData *ptpResponseData, NSError *error)
        // TODO-verify（模块清单 #2/#4）：两段数据的内容次序与回调线程。
        let block = RcBlock::new(
            move |response: *mut NSData, data: *mut NSData, error: *mut NSError| {
                // SAFETY: completion 参数由 IC 传入；只在块内立即拷贝成
                // Rust 数据，不保留任何 ObjC 引用。
                let outcome: Completion = (|| {
                    let response_bytes = unsafe { copy_nsdata(response) };
                    let data_bytes = unsafe { copy_nsdata(data) };
                    let error_text = unsafe { nserror_message(error) };
                    Ok((response_bytes, data_bytes, error_text))
                })();
                let mut guard = filler.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                *guard = Some(outcome);
            },
        );

        // SAFETY: 选择器与参数类型对照 Apple 文档：ptpCommand/outData 为
        // NSData*（可空），completion 为块指针；块存活到本函数结束。
        let _: () = unsafe {
            msg_send![
                device,
                requestSendPTPCommand: &*command_data,
                outData: out_ptr,
                completion: &*block
            ]
        };

        // completion 可能在上面的调用里同步触发；否则靠泵本线程 run loop
        // 驱动（TODO-verify #4：若回调落在他处，这里会超时报错而非挂死）。
        let deadline = Instant::now() + REQUEST_TIMEOUT;
        loop {
            {
                let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                if let Some(outcome) = guard.take() {
                    let (response_bytes, data_bytes, error_text) = outcome?;
                    if let Some(message) = error_text {
                        return Err(format!("macOS IC PTP 命令失败：{message}"));
                    }
                    let (code, payload) = completion_payloads_to_result(&response_bytes, &data_bytes)?;
                    // 与 Windows WpdTransport 相同的上限语义：IC 没有按需
                    // 限制返回长度的参数，只能事后校验（数据会先整体进内存）。
                    if payload.len() > limit {
                        return Err("PTP 数据超过请求上限".into());
                    }
                    return Ok((code, payload));
                }
            }
            if Instant::now() >= deadline {
                // TODO-verify（模块清单 #10）：超时后 IC 仍可能持有块；故意
                // 泄漏 RcBlock 防止晚到的回调触碰已释放内存。
                std::mem::forget(block);
                return Err(format!("macOS IC PTP 响应超时（操作 0x{operation:04X}）"));
            }
            self.pump(POLL_SLICE);
        }
    }
}

/// ICDevice 的 UUIDString 属性 → String（空串表示设备未提供）。
fn device_uuid(device: &Retained<AnyObject>) -> Option<String> {
    // SAFETY: UUIDString 是 ICDevice 的 NSString* 属性（Apple 文档核对）。
    let uuid: Option<Retained<NSString>> = unsafe { msg_send![device, UUIDString] };
    uuid.map(|value| value.to_string())
}

/// 读取一台相机的描述性快照。所有属性见 Apple 文档 ICDevice 页。
fn camera_info(device: &Retained<AnyObject>, session_open: bool) -> IcCameraInfo {
    // SAFETY: 只读属性：UUIDString/name/serialNumberString → NSString*；
    // usbVendorID/usbProductID → int32_t。可能返回 nil，用 Option 接住。
    let uuid: Option<Retained<NSString>> = unsafe { msg_send![device, UUIDString] };
    let name: Option<Retained<NSString>> = unsafe { msg_send![device, name] };
    let serial: Option<Retained<NSString>> = unsafe { msg_send![device, serialNumberString] };
    let vendor: i32 = unsafe { msg_send![device, usbVendorID] };
    let product: i32 = unsafe { msg_send![device, usbProductID] };
    let to_string = |value: Option<Retained<NSString>>| {
        value.map(|s| s.to_string()).unwrap_or_default()
    };
    IcCameraInfo {
        uuid: to_string(uuid),
        name: to_string(name),
        serial: to_string(serial),
        usb_vendor_id: vendor,
        usb_product_id: product,
        session_open,
    }
}

/// completion 块里立即拷贝 NSData → Vec<u8>。空指针按空数据处理。
unsafe fn copy_nsdata(data: *mut NSData) -> Vec<u8> {
    if data.is_null() {
        return Vec::new();
    }
    // SAFETY: length/bytes 是 NSData 标准方法；bytes 返回 const void*，
    // 用 c_void 指针接收以匹配编码，随后转 u8。数据在块返回前拷贝完毕，
    // 不保留指针。
    let len: usize = unsafe { msg_send![data, length] };
    if len == 0 {
        return Vec::new();
    }
    let bytes: *const std::ffi::c_void = unsafe { msg_send![data, bytes] };
    unsafe { std::slice::from_raw_parts(bytes.cast::<u8>(), len) }.to_vec()
}

/// NSError → 便于日志的文案。空指针（无错误）返回 None。
unsafe fn nserror_message(error: *mut NSError) -> Option<String> {
    if error.is_null() {
        return None;
    }
    // SAFETY: localizedDescription 是 NSError 的 NSString* 属性；code 是
    // NSInteger（64 位平台为 i64）。只读且立即拷贝。
    let description: Option<Retained<NSString>> = unsafe { msg_send![error, localizedDescription] };
    let code: i64 = unsafe { msg_send![error, code] };
    Some(match description {
        Some(text) => format!("{}（code {}）", text, code),
        None => format!("code {code}"),
    })
}

// ---------------------------------------------------------------------------
// 共享工作线程的引导与停止
// ---------------------------------------------------------------------------

struct WorkerHandle {
    tx: Sender<WorkerCommand>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

static IC_WORKER: OnceLock<Mutex<Option<WorkerHandle>>> = OnceLock::new();

/// 取共享 IC 工作线程的命令通道；首次调用时引导线程。
fn shared_worker() -> Result<Sender<WorkerCommand>, String> {
    let cell = IC_WORKER.get_or_init(|| Mutex::new(None));
    let mut guard = cell.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(spawn_worker()?);
    }
    Ok(guard.as_ref().expect("上面刚填入").tx.clone())
}

fn spawn_worker() -> Result<WorkerHandle, String> {
    let (tx, rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    let join = thread::Builder::new()
        .name("ic-camera-runloop".into())
        .spawn(move || run_worker(rx, stop_for_thread, ready_tx))
        .map_err(|error| format!("无法启动 macOS IC run loop 线程：{error}"))?;
    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(WorkerHandle {
            tx,
            stop,
            join: Some(join),
        }),
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            let _ = join.join();
            Err("macOS IC 工作线程初始化超时".into())
        }
    }
}

/// 工作线程主体：bootstrap → （处理命令 + 泵 run loop）→ 收尾。
fn run_worker(rx: Receiver<WorkerCommand>, stop: Arc<AtomicBool>, ready: Sender<Result<(), String>>) {
    let worker = match IcWorker::bootstrap() {
        Ok(worker) => {
            let _ = ready.send(Ok(()));
            worker
        }
        Err(error) => {
            let _ = ready.send(Err(error));
            return;
        }
    };
    while !stop.load(Ordering::Acquire) {
        match rx.try_recv() {
            Ok(command) => worker.handle(command),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => break,
        }
        worker.pump(POLL_SLICE);
    }
    // 收尾：停止浏览（浏览器属于本线程 run loop），再泵一小段让它清理。
    // SAFETY: stop 幂等。
    let _: () = unsafe { msg_send![&worker.browser, stop] };
    let deadline = Instant::now() + Duration::from_millis(200);
    while Instant::now() < deadline {
        worker.pump(POLL_SLICE);
    }
}

/// 停止共享 IC 工作线程（测试/进程收尾用）。若线程正卡在长请求上会等它
/// 超时返回——应用运行期不需要调用。
pub(crate) fn shutdown_worker() {
    let Some(cell) = IC_WORKER.get() else {
        return;
    };
    let mut guard = match cell.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    if let Some(handle) = guard.take() {
        handle.stop.store(true, Ordering::Release);
        if let Some(join) = handle.join {
            let _ = join.join();
        }
    }
}

// ---------------------------------------------------------------------------
// 对外入口：枚举 / 连接（PtpTransport）
// ---------------------------------------------------------------------------

/// 枚举 IC 可见相机。发现为空只说明“此刻没看到”，能力边界见模块
/// TODO-verify 清单（尤其 #5：无 delegate 的 devices 填充行为）。
pub(crate) fn enumerate_cameras() -> Result<Vec<IcCameraInfo>, String> {
    let tx = shared_worker()?;
    let (reply, receive) = mpsc::channel();
    tx.send(WorkerCommand::Enumerate { reply })
        .map_err(|_| "macOS IC 工作线程已退出".to_string())?;
    receive
        .recv()
        .map_err(|_| "macOS IC 工作线程处理请求时退出".to_string())?
}

/// ImageCaptureCore PTP 传输：所有请求经共享 run loop 线串行执行。
pub(crate) struct IcPtpTransport {
    tx: Sender<WorkerCommand>,
    uuid: String,
}

impl IcPtpTransport {
    /// 连接相机（按 [`IcCameraInfo::uuid`]）并打开会话。无法核实的边界
    /// 一律返回 Err（见模块 TODO-verify 清单），绝不假装成功。
    pub(crate) fn connect(uuid: &str) -> Result<Self, String> {
        let tx = shared_worker()?;
        let (reply, receive) = mpsc::channel();
        tx.send(WorkerCommand::OpenSession {
            uuid: uuid.to_string(),
            reply,
        })
        .map_err(|_| "macOS IC 工作线程已退出".to_string())?;
        receive
            .recv()
            .map_err(|_| "macOS IC 工作线程处理请求时退出".to_string())??;
        Ok(Self {
            tx,
            uuid: uuid.to_string(),
        })
    }

    fn request(
        &self,
        operation: u16,
        params: &[u32],
        out_data: Option<&[u8]>,
        limit: usize,
    ) -> Result<(u32, Vec<u8>), String> {
        let (reply, receive) = mpsc::channel();
        self.tx
            .send(WorkerCommand::SendPtp {
                uuid: self.uuid.clone(),
                operation,
                params: params.to_vec(),
                out_data: out_data.map(|data| data.to_vec()),
                limit,
                reply,
            })
            .map_err(|_| "macOS IC 工作线程已退出".to_string())?;
        receive
            .recv()
            .map_err(|_| "macOS IC 工作线程处理请求时退出".to_string())?
    }
}

impl Drop for IcPtpTransport {
    fn drop(&mut self) {
        // 只投递关闭命令，不阻塞等待：Drop 不能长时间卡住调用方。回执通道
        // 立即销毁，工作线程发送回执失败会被静默忽略。
        let (reply, _) = mpsc::channel();
        let _ = self.tx.send(WorkerCommand::CloseSession {
            uuid: self.uuid.clone(),
            reply,
        });
    }
}

impl PtpTransport for IcPtpTransport {
    fn read(
        &mut self,
        operation: u16,
        params: &[u32],
        limit: usize,
    ) -> Result<(u32, Vec<u8>), String> {
        self.request(operation, params, None, limit)
    }

    fn write(&mut self, operation: u16, params: &[u32], data: &[u8]) -> Result<u32, String> {
        // 写入后的响应通常无数据阶段；上限取 len+1 只为复用同一检查路径。
        self.request(operation, params, Some(data), data.len().saturating_add(1))
            .map(|(code, _)| code)
    }

    fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
        self.request(operation, params, None, 0).map(|(code, _)| code)
    }
}

// ---------------------------------------------------------------------------
// 单元测试：纯函数部分（只在 macOS 上编译/运行；ObjC 层不在此断言）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn response_container(code: u16, transaction: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(12u32.to_le_bytes());
        bytes.extend(CONTAINER_RESPONSE.to_le_bytes());
        bytes.extend(code.to_le_bytes());
        bytes.extend(transaction.to_le_bytes());
        bytes
    }

    #[test]
    fn command_container_matches_ptp_usb_layout() {
        let bytes = build_command_container(0x1001, &[1, 0xdead_beef], 7).unwrap();
        assert_eq!(bytes.len(), 20);
        assert_eq!(u32::from_le_bytes(bytes[0..4].try_into().unwrap()), 20);
        assert_eq!(u16::from_le_bytes(bytes[4..6].try_into().unwrap()), CONTAINER_COMMAND);
        assert_eq!(u16::from_le_bytes(bytes[6..8].try_into().unwrap()), 0x1001);
        assert_eq!(u32::from_le_bytes(bytes[8..12].try_into().unwrap()), 7);
        assert_eq!(u32::from_le_bytes(bytes[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 0xdead_beef);
        let empty = build_command_container(0x1002, &[], 0).unwrap();
        assert_eq!(empty.len(), 12);
        assert_eq!(u32::from_le_bytes(empty[0..4].try_into().unwrap()), 12);
    }

    #[test]
    fn command_container_rejects_more_than_five_params() {
        assert!(build_command_container(1, &[0; 5], 0).is_ok());
        assert!(build_command_container(1, &[0; 6], 0).is_err());
    }

    #[test]
    fn response_container_parses_code_and_transaction() {
        let bytes = response_container(0x2001, 9);
        assert_eq!(parse_response_container(&bytes).unwrap(), (0x2001, 9));
        for len in 0..bytes.len() {
            assert!(parse_response_container(&bytes[..len]).is_err());
        }
        // 类型不是 3：按数据容器处理必须失败。
        let mut wrong_type = response_container(0x2001, 9);
        wrong_type[4..6].copy_from_slice(&CONTAINER_DATA.to_le_bytes());
        assert!(parse_response_container(&wrong_type).is_err());
        // 长度字段与实际不符必须失败。
        let mut wrong_length = response_container(0x2001, 9);
        wrong_length[0..4].copy_from_slice(&20u32.to_le_bytes());
        assert!(parse_response_container(&wrong_length).is_err());
    }

    #[test]
    fn completion_payloads_identify_response_container_in_either_position() {
        let response = response_container(0x2019, 3);
        let payload = vec![0xff, 0xd8, 0xff, 3, 0xff, 0xd9];
        let (code, data) = completion_payloads_to_result(&response, &payload).unwrap();
        assert_eq!(code, 0x2019);
        assert_eq!(data, payload);
        // 次序相反（文档未写死两个参数的内容）也能防御性识别。
        let (code, data) = completion_payloads_to_result(&payload, &response).unwrap();
        assert_eq!(code, 0x2019);
        assert_eq!(data, payload);
        // 两段都不是响应容器：必须报“未核对”，不能假装成功。
        let error = completion_payloads_to_result(&payload, &payload).unwrap_err();
        assert!(error.contains("未核对"), "unexpected error: {error}");
    }
}
