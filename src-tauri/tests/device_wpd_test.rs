//! WPD/MTP 源纯函数：相对路径拼接、OLE/FILETIME/ISO 日期换算，
//! 外加需真机的手动验收用例（默认忽略）。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, settings};

use std::io::Read;

use common::utc;
use devices::wpd::{filetime_to_utc, join_rel_path, ole_date_to_utc, parse_wpd_date_string};
use devices::{DeviceSource, SourceKind};

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
