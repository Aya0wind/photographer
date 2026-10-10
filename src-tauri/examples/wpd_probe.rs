//! WPD 能力探针（阶段 E「联机拍摄」评估工具）。
//!
//! 列出已连接 MTP 相机声明的 WPD 命令 / 功能类别 / 事件，并给出三个关键
//! 判定：能否零驱动触发拍摄（WPD_COMMAND_STILL_IMAGE_CAPTURE_INITIATE，
//! fmtid=4FCD6982… pid=2）、有无拍摄功能类别（WPD_FUNCTIONAL_CATEGORY_
//! STILL_IMAGE_CAPTURE）、有无对象新增事件（WPD_EVENT_OBJECT_ADDED，拍后
//! 自动收片的依据）。
//!
//! 用法：`cargo run --example wpd_probe [-- 名称过滤子串]`

use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Devices::PortableDevices::{
    IPortableDevice, IPortableDeviceCapabilities, IPortableDeviceKeyCollection,
    IPortableDeviceManager, IPortableDevicePropVariantCollection, IPortableDeviceValues,
    WPD_CLIENT_MAJOR_VERSION, WPD_CLIENT_NAME,
};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Variant::{VT_CLSID, VT_LPWSTR};

// coclass GUID（windows crate 未收录 CLSID 常量，照抄 SDK 头数值——与
// src/devices/wpd.rs 保持一致）。
const CLSID_PORTABLE_DEVICE_MANAGER: GUID =
    GUID::from_u128(0x0af10cec_2ecd_4b92_9581_34f6ae0637f3);
const CLSID_PORTABLE_DEVICE: GUID = GUID::from_u128(0x728a21c5_3d9e_48d7_9810_864848f0f404);
const CLSID_PORTABLE_DEVICE_VALUES: GUID =
    GUID::from_u128(0x0c15d503_d017_47ce_9016_7b3f978721cc);

/// WPD_CATEGORY_STILL_IMAGE_CAPTURE（拍摄命令组；INITIATE 为 pid 2）。
const STILL_IMAGE_CAPTURE_CATEGORY: GUID =
    GUID::from_u128(0x4fcd6982_22a2_4b05_a48b_62d38bf27b32);
const STILL_IMAGE_CAPTURE_INITIATE_PID: u32 = 2;
/// WPD_FUNCTIONAL_CATEGORY_STILL_IMAGE_CAPTURE。
const FUNCTIONAL_STILL_CAPTURE: GUID =
    GUID::from_u128(0x613ca327_ab93_4900_b4fa_895bb5874b79);
/// WPD_EVENT_OBJECT_ADDED。
const EVENT_OBJECT_ADDED: GUID = GUID::from_u128(0xa726da95_e207_4b02_8d44_bef2e86cbffc);

fn main() {
    let filter = std::env::args().nth(1);
    // SAFETY: 进程级 COM 初始化，主线程独占，退出前配对 CoUninitialize
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().expect("CoInitializeEx");
    let exit = std::panic::catch_unwind(|| run(filter));
    // SAFETY: 与 CoInitializeEx 配对
    unsafe { CoUninitialize() };
    if exit.is_err() {
        std::process::exit(1);
    }
}

fn run(filter: Option<String>) {
    // SAFETY: CLSID 为静态常量；无外部聚合
    let manager: IPortableDeviceManager = unsafe {
        CoCreateInstance(
            &CLSID_PORTABLE_DEVICE_MANAGER,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }
    .expect("CoCreateInstance(PortableDeviceManager)");

    let mut count = 0u32;
    // SAFETY: 两段式第一段：仅查询设备数（数组指针为 NULL）
    unsafe { manager.GetDevices(std::ptr::null_mut(), &mut count) }.expect("GetDevices(count)");
    if count == 0 {
        println!("无 WPD 设备");
        return;
    }
    let mut ids = vec![PWSTR::null(); count as usize];
    // SAFETY: 缓冲容量与第一段一致
    unsafe { manager.GetDevices(ids.as_mut_ptr(), &mut count) }.expect("GetDevices(fill)");

    for &id_ptr in &ids[..count as usize] {
        // SAFETY: id_ptr 由 GetDevices 填充（可能为 NULL）
        let pnp = unsafe { pwstr_to_string(id_ptr) };
        // SAFETY: GetDevices 分配的 PnP 字符串由调用方释放
        unsafe { CoTaskMemFree(Some(id_ptr.as_ptr() as _)) };
        if pnp.is_empty() {
            continue;
        }
        let name = friendly_name(&manager, &pnp);
        if let Some(f) = &filter {
            if !pnp.to_lowercase().contains(&f.to_lowercase())
                && !name.to_lowercase().contains(&f.to_lowercase())
            {
                continue;
            }
        }
        println!("=== {name} ({pnp})");
        probe_device(&pnp);
    }
}

fn probe_device(pnp: &str) {
    let device = match open_device(pnp) {
        Ok(d) => d,
        Err(e) => {
            println!("  打开失败: {e}");
            return;
        }
    };
    // SAFETY: device 已成功 Open；Capabilities 为只读查询
    let caps: IPortableDeviceCapabilities = unsafe { device.Capabilities() }.expect("Capabilities");

    // SAFETY: 均为只读查询
    let commands: IPortableDeviceKeyCollection =
        unsafe { caps.GetSupportedCommands() }.expect("GetSupportedCommands");
    let events: IPortableDevicePropVariantCollection =
        unsafe { caps.GetSupportedEvents() }.expect("GetSupportedEvents");
    let categories: IPortableDevicePropVariantCollection =
        unsafe { caps.GetFunctionalCategories() }.expect("GetFunctionalCategories");

    // SAFETY: GetCount 只读
    let mut n_cmds = 0u32;
    // SAFETY: 出参指针指向合法 u32
    unsafe { commands.GetCount(&mut n_cmds) }.expect("GetCount");
    let mut initiate = false;
    let mut category_cmds = 0usize;
    for i in 0..n_cmds {
        // SAFETY: 索引在范围内
        let mut key = PROPERTYKEY::default();
        // SAFETY: 出参指针指向合法 PROPERTYKEY
        unsafe { commands.GetAt(i, &mut key) }.expect("GetAt");
        if key.fmtid == STILL_IMAGE_CAPTURE_CATEGORY {
            category_cmds += 1;
            println!("  命令: {:?}/{}", key.fmtid, key.pid);
            if key.pid == STILL_IMAGE_CAPTURE_INITIATE_PID {
                initiate = true;
            }
        } else if i < 12 {
            // 全量打印太吵：只打印前 12 个 + 拍摄组的全部
            println!("  命令: {:?}/{}", key.fmtid, key.pid);
        }
    }

    let events = guid_collection(&events, "事件");
    let categories = guid_collection(&categories, "功能类别");

    let added = events.contains(&EVENT_OBJECT_ADDED);
    let func = categories.contains(&FUNCTIONAL_STILL_CAPTURE);
    println!(
        "  判定：零驱动触发拍摄(WPD_COMMAND_STILL_IMAGE_CAPTURE_INITIATE)={initiate} | \
         OBJECT_ADDED 事件={added} | 拍摄功能类别={func}"
    );
    if !initiate {
        println!(
            "  （命令总数 {n_cmds}，拍摄组命令 {category_cmds} 项——WPD 通道大概率只支持文件传输）"
        );
    }
}

/// 枚举 VT_CLSID 集合并打印；返回 GUID 列表供判定。
fn guid_collection(collection: &IPortableDevicePropVariantCollection, label: &str) -> Vec<GUID> {
    let mut out = Vec::new();
    // SAFETY: GetCount 只读
    let mut total = 0u32;
    // SAFETY: 出参指针指向合法 u32
    unsafe { collection.GetCount(&mut total) }.expect("GetCount");
    for i in 0..total {
        // SAFETY: 索引在范围内
        let mut prop = PROPVARIANT::default();
        // SAFETY: 出参指针指向合法 PROPVARIANT
        unsafe { collection.GetAt(i, &mut prop) }.expect("GetAt");
        // SAFETY: 读 union 前先经 vt 判别
        let vt = unsafe { prop.Anonymous.Anonymous.vt.0 };
        if vt == VT_CLSID.0 {
            // SAFETY: vt == VT_CLSID 保证 puuid 指向合法 GUID
            let guid = unsafe { *prop.Anonymous.Anonymous.Anonymous.puuid };
            println!("  {label}: {guid:?}");
            out.push(guid);
        } else if vt == VT_LPWSTR.0 {
            // SAFETY: vt == VT_LPWSTR 保证 pwszVal 指向 NUL 结尾字符串
            let s = unsafe { pwstr_to_string(prop.Anonymous.Anonymous.Anonymous.pwszVal) };
            println!("  {label}(str): {s}");
        } else {
            println!("  {label}: vt={vt}（非 GUID，跳过）");
        }
        // SAFETY: prop 可能持有堆内存（VT_LPWSTR 等），必须清理
        let _ = unsafe { PropVariantClear(&mut prop) };
    }
    out
}

fn open_device(pnp_id: &str) -> windows::core::Result<IPortableDevice> {
    // SAFETY: CLSID 为静态常量；无外部聚合
    let client_info: IPortableDeviceValues = unsafe {
        CoCreateInstance(
            &CLSID_PORTABLE_DEVICE_VALUES,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    // SAFETY: key 为静态常量；部分设备要求非空客户端信息才允许 Open
    unsafe {
        client_info.SetStringValue(&WPD_CLIENT_NAME, windows::core::w!("Photographer Probe"))?;
        client_info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1)?;
    }
    // SAFETY: CLSID 为静态常量；无外部聚合
    let device: IPortableDevice = unsafe {
        CoCreateInstance(
            &CLSID_PORTABLE_DEVICE,
            None::<&windows::core::IUnknown>,
            CLSCTX_INPROC_SERVER,
        )
    }?;
    let wide: Vec<u16> = pnp_id.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: wide 以 NUL 结尾且在本调用内存活
    unsafe { device.Open(PCWSTR(wide.as_ptr()), &client_info)? };
    Ok(device)
}

/// SAFETY: pwszVal 必须指向 NUL 结尾的 UTF-16 字符串（或 NULL）。
unsafe fn pwstr_to_string(pwstr: PWSTR) -> String {
    if pwstr.is_null() {
        return String::new();
    }
    unsafe {
        let mut len = 0usize;
        while *pwstr.as_ptr().add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(pwstr.as_ptr(), len))
    }
}

fn friendly_name(manager: &IPortableDeviceManager, pnp: &str) -> String {
    let wide: Vec<u16> = pnp.encode_utf16().chain(std::iter::once(0)).collect();
    let mut len = 0u32;
    // SAFETY: 第一段以空缓冲取所需长度（预期返回失败）
    let _ = unsafe {
        manager.GetDeviceFriendlyName(PCWSTR(wide.as_ptr()), PWSTR::null(), &mut len)
    };
    if len == 0 {
        return pnp.to_string();
    }
    let mut buf = vec![0u16; len as usize];
    // SAFETY: buf 容量满足第一段报告的长度
    let hr = unsafe {
        manager.GetDeviceFriendlyName(PCWSTR(wide.as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len)
    };
    if hr.is_err() {
        return pnp.to_string();
    }
    String::from_utf16_lossy(&buf[..(len as usize).min(buf.len())])
        .trim_end_matches('\0')
        .to_string()
}
