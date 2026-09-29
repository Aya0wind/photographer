use crate::events::EventBus;
use std::thread::JoinHandle;
/// 启动热插拔监视线程（幂等性由调用方保证：应用生命周期内启动一次）。
pub fn spawn_hotplug_thread(bus: EventBus) -> JoinHandle<()> {
    win::spawn_hotplug_thread(bus)
}

/// 请求线程退出（向消息窗口投递 WM_QUIT）并等待收尾。
#[allow(dead_code)] // 预留给应用退出收尾
pub fn stop(handle: JoinHandle<()>) {
    win::stop(handle)
}

mod win {
    use std::cell::RefCell;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::thread::JoinHandle;

    use crate::devices::hotplug::{pnp_display_name, unitmask_to_drives};
    use crate::events::{AppEvent, EventBus, SourceKind};

    use windows::core::{w, GUID, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
        PostMessageW, RegisterClassW, RegisterDeviceNotificationW, TranslateMessage,
        UnregisterDeviceNotification, DBT_DEVICEARRIVAL, DBT_DEVICEREMOVECOMPLETE,
        DBT_DEVNODES_CHANGED, DBT_DEVTYP_DEVICEINTERFACE, DBT_DEVTYP_VOLUME,
        DEVICE_NOTIFY_WINDOW_HANDLE, DEV_BROADCAST_DEVICEINTERFACE_W, DEV_BROADCAST_HDR,
        DEV_BROADCAST_VOLUME, HDEVNOTIFY, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DEVICECHANGE,
        WM_QUIT, WNDCLASSW,
    };

    /// WPD 设备接口 GUID（DEVINTERFACE_WPD）。
    pub const GUID_DEV_INTERFACE_WPD: GUID =
        GUID::from_u128(0x6ac27878_a6fa_4155_ba85_f98f491d4f33);

    /// DBTF_NET：网络卷不是可移动媒体，忽略。
    const DBTF_NET: u16 = 0x0001;

    /// 消息窗口 HWND（0 = 未创建）；stop() 据此投递 WM_QUIT。
    static WINDOW: AtomicIsize = AtomicIsize::new(0);

    thread_local! {
        /// 消息泵线程内的总线。窗口过程只在该线程被调用（message-only
        /// 窗口的消息只能由创建线程的 GetMessage 取到），无跨线程访问。
        static BUS: RefCell<Option<EventBus>> = const { RefCell::new(None) };
    }

    const CLASS_NAME: &str = "SmartPhotoHotplugWnd";

    pub fn spawn_hotplug_thread(bus: EventBus) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("hotplug".into())
            .spawn(move || run(bus))
            .expect("failed to spawn hotplug thread")
    }

    /// 停止热插拔线程：向消息窗口投递 WM_QUIT 使 GetMessageW 返回 0，再 join。
    /// 窗口尚未创建时（启动初期）仅 join——该窗口在 spawn 后毫秒级完成创建。
    #[allow(dead_code)] // 预留给应用退出收尾（外层 stop 转发）
    pub fn stop(handle: JoinHandle<()>) {
        let hwnd = WINDOW.swap(0, Ordering::SeqCst);
        if hwnd != 0 {
            // SAFETY: hwnd 由本模块创建且尚未销毁；失败仅意味着窗口已退出
            let _ =
                unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        let _ = handle.join();
    }

    fn run(bus: EventBus) {
        BUS.with(|b| *b.borrow_mut() = Some(bus));
        if let Some((hwnd, notifies)) = setup() {
            eprintln!("热插拔窗口已启动（卷广播 + WPD 接口通知/轮询）");
            let mut msg = MSG::default();
            loop {
                // SAFETY: msg 为本帧合法栈变量
                let ret = unsafe { GetMessageW(&mut msg, None, 0, 0) };
                if ret.0 == 0 || ret.0 == -1 {
                    break; // WM_QUIT / GetMessage 错误
                }
                // SAFETY: msg 已由 GetMessageW 填充
                unsafe {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            cleanup(hwnd, notifies);
        } else {
            eprintln!("热插拔监听启动失败：窗口创建/设备通知注册失败，插拔事件将不可用");
        }
        BUS.with(|b| *b.borrow_mut() = None);
    }

    /// 注册窗口类、创建 隐藏顶层窗口、注册两类设备通知。
    fn setup() -> Option<(HWND, Vec<HDEVNOTIFY>)> {
        // SAFETY: 模块名传 NULL 取当前进程可执行模块句柄
        let module = unsafe { GetModuleHandleW(None::<&PCWSTR>) }.ok()?;
        let hinstance = HINSTANCE(module.0);
        let class_name: Vec<u16> = CLASS_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance,
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: wc 在本帧存活，RegisterClassW 同步拷贝所需字段
        if unsafe { RegisterClassW(&wc) } == 0 {
            return None;
        }
        // SAFETY: 类已注册；无父窗口、未设置 WS_VISIBLE，接收卷广播但不显示。
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class_name.as_ptr()),
                w!("smart-photo-hotplug"),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinstance),
                None,
            )
        }
        .ok()?;
        WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);

        let mut notifies = Vec::new();

        // 隐藏顶层窗口接收系统卷广播；DBT_DEVTYP_VOLUME 不能显式注册。
        // WPD 设备接口（相机/手机 MTP）
        let iface_filter = DEV_BROADCAST_DEVICEINTERFACE_W {
            dbcc_size: std::mem::size_of::<DEV_BROADCAST_DEVICEINTERFACE_W>() as u32,
            dbcc_devicetype: DBT_DEVTYP_DEVICEINTERFACE.0,
            dbcc_reserved: 0,
            dbcc_classguid: GUID_DEV_INTERFACE_WPD,
            dbcc_name: [0],
        };
        // SAFETY: hwnd 合法；filter 在调用期间存活
        if let Ok(h) = unsafe {
            RegisterDeviceNotificationW(
                HANDLE(hwnd.0),
                &iface_filter as *const _ as *const core::ffi::c_void,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
        } {
            notifies.push(h);
        }

        if notifies.is_empty() {
            eprintln!("WPD 接口通知注册失败，将使用周期枚举；卷广播仍可用");
        }
        Some((hwnd, notifies))
    }

    fn cleanup(hwnd: HWND, notifies: Vec<HDEVNOTIFY>) {
        for handle in notifies {
            // SAFETY: 句柄来自成功的 RegisterDeviceNotificationW
            let _ = unsafe { UnregisterDeviceNotification(handle) };
        }
        // SAFETY: hwnd 属于当前（泵）线程
        let _ = unsafe { DestroyWindow(hwnd) };
        WINDOW.store(0, Ordering::SeqCst);
    }

    /// SAFETY: 系统按 WNDCLASSW.lpfnWndProc 约定调用；hwnd/msg/wparam/lparam
    /// 均由系统传入且在本调用期间有效。
    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_DEVICECHANGE {
            handle_device_change(wparam.0 as u32, lparam.0 as *const core::ffi::c_void);
            return LRESULT(1); // 已处理
        }
        // SAFETY: 常规默认处理
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    fn handle_device_change(event: u32, lparam: *const core::ffi::c_void) {
        if event == DBT_DEVNODES_CHANGED {
            publish(
                DBT_DEVICEARRIVAL,
                SourceKind::Mtp,
                String::new(),
                String::new(),
            );
            return;
        }
        let arrival = event == DBT_DEVICEARRIVAL;
        if !arrival && event != DBT_DEVICEREMOVECOMPLETE {
            return;
        }
        if lparam.is_null() {
            return;
        }
        // SAFETY: WM_DEVICECHANGE 的 lParam 由系统指向按 dbch_devicetype
        // 判别的 DEV_BROADCAST_* 载荷，在本消息处理期内有效；先读公共头
        // 判别类型，再按对应结构解读
        let hdr = unsafe { &*(lparam as *const DEV_BROADCAST_HDR) };
        let ev_name = if arrival {
            "DBT_DEVICEARRIVAL"
        } else {
            "DBT_DEVICEREMOVECOMPLETE"
        };
        match hdr.dbch_devicetype {
            DBT_DEVTYP_VOLUME => {
                // SAFETY: dbch_devicetype 已确认为 DEV_BROADCAST_VOLUME
                let vol = unsafe { &*(lparam as *const DEV_BROADCAST_VOLUME) };
                let drives = unitmask_to_drives(vol.dbcv_unitmask);
                let is_net = vol.dbcv_flags.0 & DBTF_NET != 0;
                // 每个 DBT 事件一行日志（真机排查到达/移除倒置问题）
                eprintln!("DBT 事件: {ev_name} 卷 {drives:?}（net={is_net}）");
                if is_net {
                    return; // 网络卷忽略
                }
                // 信号语义（调和器架构，2026-09-18）：DBT 只发触发信号，
                // 到达/移除不做任何注册/摘除/过滤——「设备」语义全部收敛到
                // reconcile 的真值（有媒体可移动卷；网络盘/本地盘不在真值）。
                for drive in drives {
                    // 消息泵只搬运信号，卷标读取留给后台枚举。
                    let name = drive.clone();
                    publish(event, SourceKind::Volume, drive, name);
                }
            }
            t if t == DBT_DEVTYP_DEVICEINTERFACE => {
                // SAFETY: dbch_devicetype 已确认为 DEV_BROADCAST_DEVICEINTERFACE_W；
                // dbcc_name 为 NUL 结尾宽字符串（长度可超结构声明中的 1 元素）
                let iface = unsafe { &*(lparam as *const DEV_BROADCAST_DEVICEINTERFACE_W) };
                if iface.dbcc_classguid != GUID_DEV_INTERFACE_WPD {
                    return;
                }
                let path = unsafe { wide_ptr_to_string(iface.dbcc_name.as_ptr()) };
                if path.is_empty() {
                    return;
                }
                // 每个 DBT 事件一行日志（真机排查到达/移除倒置问题）
                eprintln!("DBT 事件: {ev_name} WPD 接口 {path}");
                let name = pnp_display_name(&path);
                publish(event, SourceKind::Mtp, path, name);
            }
            _ => {}
        }
    }

    fn publish(event: u32, kind: SourceKind, id: String, name: String) {
        BUS.with(|b| {
            if let Some(bus) = b.borrow().as_ref() {
                let _ = (kind, name);
                bus.publish(AppEvent::DeviceTopologyChanged {
                    id: crate::devices::normalize_device_id(&id),
                    arrived: event == DBT_DEVICEARRIVAL,
                });
            }
        });
    }

    /// 读 NUL 结尾宽字符串（4096 字符上限防御异常数据）。
    ///
    /// SAFETY: p 必须指向 NUL 结尾的合法 UTF-16 缓冲。
    unsafe fn wide_ptr_to_string(mut p: *const u16) -> String {
        let mut buf = Vec::new();
        while buf.len() < 4096 {
            // SAFETY: 调用方保证 NUL 结尾，循环在 NUL 处终止
            let c = unsafe { *p };
            if c == 0 {
                break;
            }
            buf.push(c);
            // SAFETY: p 指向缓冲内部且未越过后继 NUL
            p = unsafe { p.add(1) };
        }
        String::from_utf16_lossy(&buf)
    }
}

// ---------------------------------------------------------------------------
// 非 Windows 桩（保持模块树跨平台可编译；本产品仅面向 Windows）
// ---------------------------------------------------------------------------
