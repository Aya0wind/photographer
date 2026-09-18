//! WPD/MTP 源纯函数：相对路径拼接、OLE/FILETIME/ISO 日期换算，
//! 外加需真机的手动验收用例（默认忽略）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings, tasks, thumbs};

use std::io::Read;

use common::utc;
use devices::wpd::{
    com_apartment_owned, filetime_to_utc, join_rel_path, ole_date_to_utc, parse_wpd_date_string,
    WpdSource,
};
use devices::{normalize_device_id, DeviceError, DeviceSource, SourceKind};

#[test]
fn object_level_skip_classification() {
    use devices::wpd::is_object_level_skip;
    // 受限对象（访问被拒）→ 跳过计数
    assert!(is_object_level_skip(&DeviceError::AccessDenied));
    // 无名内部对象（播放列表/系统对象，缺文件名/基本属性）→ 跳过计数
    //（真机：ILCE-7RM5 的 o18179B 对象导致整树失败、设备注册不上）
    assert!(is_object_level_skip(&DeviceError::Other(
        "对象缺文件名: o18179B".into()
    )));
    // 会话丢失/拔线/枚举接口失败仍为致命（要么完整要么报错）
    assert!(!is_object_level_skip(&DeviceError::Disconnected));
    assert!(!is_object_level_skip(&DeviceError::Other(
        "WPD error 0x800710D2".into()
    )));
    assert!(!is_object_level_skip(&DeviceError::Other(
        "读取失败".into()
    )));
    assert!(!is_object_level_skip(&DeviceError::NotSupported(
        "x".into()
    )));
}

#[test]
fn normalize_device_id_collapses_wpd_case_variants() {
    // 同一相机的三种大小写形式 → 同一规范化 key（注册/查找/移除单一事实源）
    let upper =
        r"\\?\USB#VID_054C&PID_0E0B#002166OKDGN00AYZR#{6AC27878-A6FA-4155-BA85-F98F491D4F33}";
    let lower = upper.to_ascii_lowercase();
    let mixed =
        r"\\?\usb#vid_054c&pid_0e0b#002166okdgn00ayzr#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";
    assert_eq!(normalize_device_id(upper), normalize_device_id(&lower));
    assert_eq!(normalize_device_id(upper), normalize_device_id(mixed));
    assert_eq!(normalize_device_id(upper), lower);

    // 非 PnP 形态原样保留：盘符（卷 id）、FOLDER: 源、文件系统 verbatim 路径
    //（NTFS 路径大小写敏感，文件夹路径含 # 也不得小写化）
    assert_eq!(normalize_device_id("E:"), "E:");
    assert_eq!(
        normalize_device_id(r"FOLDER:C:\Photos#a"),
        r"FOLDER:C:\Photos#a"
    );
    assert_eq!(
        normalize_device_id(r"\\?\C:\Path#With#Hash"),
        r"\\?\C:\Path#With#Hash"
    );
    assert_eq!(
        normalize_device_id(r"\\?\UNC\srv\Share#S"),
        r"\\?\UNC\srv\Share#S"
    );
}

#[test]
fn wpd_source_id_is_normalized() {
    // 以热插 DBT 的大写 pnp 构建 → id() 仍产出规范化（小写）形式，
    // 与启动枚举注册的 key 一致（COM 调用保留原串，仅标识归一）
    let upper = r"\\?\USB#VID_054C&PID_0E0B#SERIAL#{GUID}";
    let src = WpdSource::new(upper, "ILCE-7RM5");
    assert_eq!(src.id(), upper.to_ascii_lowercase());
    assert_eq!(src.kind(), SourceKind::Mtp);
    assert_eq!(src.name(), "ILCE-7RM5");
}

#[test]
fn com_apartment_guard_three_state_semantics() {
    // S_OK：本线程首次初始化成功 → owned（drop 时配对 CoUninitialize）
    assert_eq!(com_apartment_owned(0), Ok(true));
    // S_FALSE：线程已初始化（他人持有）→ 沿用现有 apartment，绝不 uninit
    assert_eq!(com_apartment_owned(1), Ok(false));
    // RPC_E_CHANGED_MODE（0x80010106）：线程为 STA（tao 事件循环的 IPC 主线程）
    // → 沿用现有 apartment 继续调用（WPD 在 STA 上合法），绝不 uninit
    assert_eq!(com_apartment_owned(0x8001_0106u32 as i32), Ok(false));
    // 其余失败码上抛（调用方转设备错误语义）
    assert_eq!(
        com_apartment_owned(0x8000_4005u32 as i32),
        Err(0x8000_4005u32 as i32)
    );
    assert!(com_apartment_owned(-1).is_err());
    // 负 HRESULT 表示（0x80010106 即 -2147417850）与 u32 判定一致
    assert_eq!(com_apartment_owned(-2147417850i32), Ok(false));
}

#[test]
fn wpd_join_rel_path() {
    assert_eq!(join_rel_path("", "IMG.CR3"), "IMG.CR3");
    assert_eq!(join_rel_path("DCIM", "100CANON"), "DCIM/100CANON");
    assert_eq!(
        join_rel_path("DCIM/100CANON", "IMG.CR3"),
        "DCIM/100CANON/IMG.CR3"
    );
}

#[test]
fn wpd_ole_date_conversion() {
    // OLE 纪元与锚点：25569.0 恰为 Unix 纪元（1970-01-01）
    assert_eq!(ole_date_to_utc(0.0).unwrap(), utc(1899, 12, 30, 0, 0, 0));
    assert_eq!(ole_date_to_utc(25569.0).unwrap(), utc(1970, 1, 1, 0, 0, 0));
    assert_eq!(ole_date_to_utc(25569.5).unwrap(), utc(1970, 1, 1, 12, 0, 0));
    // 负日期（1899-12-30 之前）
    assert_eq!(ole_date_to_utc(-1.0).unwrap(), utc(1899, 12, 29, 0, 0, 0));
    // 非法值
    assert!(ole_date_to_utc(f64::NAN).is_none());
    assert!(ole_date_to_utc(f64::INFINITY).is_none());
    assert!(ole_date_to_utc(1e12).is_none());
}

#[test]
fn wpd_iso_date_string_parsing() {
    assert_eq!(
        parse_wpd_date_string("2024-01-02T03:04:05Z").unwrap(),
        utc(2024, 1, 2, 3, 4, 5)
    );
    // 带时区偏移 → 换算 UTC
    assert_eq!(
        parse_wpd_date_string("2024-01-02T03:04:05+08:00").unwrap(),
        utc(2024, 1, 1, 19, 4, 5)
    );
    // 无时区（按 UTC）
    assert_eq!(
        parse_wpd_date_string("2024-01-02 03:04:05").unwrap(),
        utc(2024, 1, 2, 3, 4, 5)
    );
    assert_eq!(
        parse_wpd_date_string("20240102T030405").unwrap(),
        utc(2024, 1, 2, 3, 4, 5)
    );
    assert!(parse_wpd_date_string("not-a-date").is_none());
    assert!(parse_wpd_date_string("").is_none());
}

#[test]
fn wpd_filetime_conversion() {
    assert_eq!(filetime_to_utc(0, 0).unwrap(), utc(1601, 1, 1, 0, 0, 0));
    // Unix 纪元 = 116444736000000000（100ns 计数）
    let epoch_ticks: u64 = 116444736000000000;
    assert_eq!(
        filetime_to_utc(epoch_ticks as u32, (epoch_ticks >> 32) as u32).unwrap(),
        utc(1970, 1, 1, 0, 0, 0)
    );
    // 2024-01-01T00:00:00Z = 133485408000000000（Unix 1704067200 + 1601 纪元差）
    let ticks: u64 = 133485408000000000;
    assert_eq!(
        filetime_to_utc(ticks as u32, (ticks >> 32) as u32).unwrap(),
        utc(2024, 1, 1, 0, 0, 0)
    );
}

// ---------------------------------------------------------------------------
// 真机验收（手动，默认忽略）
// ---------------------------------------------------------------------------

#[test]
#[ignore = "需真机（MTP 相机/手机）手动验收"]
fn mtp_enumerate_real_devices() {
    let devices_list = devices::wpd::enumerate_mtp_devices().unwrap();
    for (id, name) in &devices_list {
        println!("{name}: {id}");
    }
    assert!(!devices_list.is_empty(), "应至少枚举到一台 MTP 设备");
}

#[test]
#[ignore = "需真机（MTP 相机/手机）手动验收"]
fn mtp_list_and_read_real_device() {
    let devices_list = devices::wpd::enumerate_mtp_devices().unwrap();
    let Some((id, name)) = devices_list.first().cloned() else {
        return;
    };
    let src = devices::wpd::WpdSource::new(&id, &name);
    assert_eq!(src.kind(), SourceKind::Mtp);

    let files = src.list().unwrap();
    println!("设备 {name} 共 {} 个媒体文件", files.len());
    for f in files.iter().take(20) {
        println!("{} ({} bytes, {})", f.rel_path, f.size, f.mtime);
    }
    if let Some(first) = files.first().cloned() {
        let head = src.open_head(&first.id, 4096).unwrap();
        assert!(head.len() <= 4096);
        let mut stream = src.stream(&first.id).unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).unwrap();
        assert_eq!(buf.len() as u64, first.size, "stream 应读全量");
        drop(stream);
        println!("已验证全量读取: {} ({}B)", first.rel_path, first.size);
    }
}
