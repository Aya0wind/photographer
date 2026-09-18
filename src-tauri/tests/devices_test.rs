//! 设备源测试：卷源（tempdir 造树）、热插拔纯函数、WPD 纯函数。
//!
//! devices 模块尚未在 lib.rs 对外公开（M1 骨架），集成测试用 `#[path]`
//! 直接纳入源码模块树；模块内部 `crate::events` 引用在测试 crate 同样成立。

#[path = "../src/db/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树（orchestrator 依赖）
mod db;
#[path = "../src/devices/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod devices;
#[path = "../src/events/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod events;

use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, NaiveDate, Utc};
use devices::hotplug::{pnp_display_name, unitmask_to_drives};
use devices::volume::VolumeSource;
use devices::wpd::{filetime_to_utc, join_rel_path, ole_date_to_utc, parse_wpd_date_string};
use devices::{DeviceError, DeviceSource, SourceKind};

/// 造标准测试树：3 个媒体文件 + 各类应忽略项。
fn build_tree(root: &Path) {
    let dcim = root.join("DCIM").join("100CANON");
    fs::create_dir_all(&dcim).unwrap();
    // 大小写混合扩展名（.CR3 大写）
    fs::write(dcim.join("IMG_0001.CR3"), vec![b'C'; 100]).unwrap();
    fs::write(dcim.join("IMG_0002.jpg"), b"jpeg-bytes").unwrap();
    fs::write(dcim.join("MVI_0003.MP4"), b"mp4-bytes!!").unwrap();

    // 非媒体扩展名
    fs::write(root.join("notes.txt"), b"ignore me").unwrap();

    // 系统目录（应整体跳过）
    let sys = root.join("System Volume Information");
    fs::create_dir_all(&sys).unwrap();
    fs::write(sys.join("WPQR0001.jpg"), b"x").unwrap();
    let bin = root.join("$RECYCLE.BIN");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("trashed.jpg"), b"x").unwrap();

    // 隐藏目录（`.` 开头）
    let hidden = root.join(".Trashes");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(hidden.join("hidden.jpg"), b"x").unwrap();
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
        .and_utc()
}

// ---------------------------------------------------------------------------
// 卷设备源
// ---------------------------------------------------------------------------

#[test]
fn volume_list_filters_sorts_and_normalizes() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let src = VolumeSource::new(dir.path());

    let files = src.list().unwrap();
    let rels: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
    // 大小写混合扩展名（.CR3/.jpg/.MP4）全部命中；系统/回收站/隐藏目录/非媒体全部排除
    assert_eq!(
        rels,
        [
            "DCIM/100CANON/IMG_0001.CR3",
            "DCIM/100CANON/IMG_0002.jpg",
            "DCIM/100CANON/MVI_0003.MP4",
        ]
    );

    for f in &files {
        // 卷设备的 id 即相对路径，且统一 `/` 分隔（无 `\`、无 `./` 前缀）
        assert_eq!(f.id, f.rel_path);
        assert!(!f.rel_path.contains('\\'));
        assert!(!f.rel_path.starts_with("./"));
    }

    let cr3 = &files[0];
    assert_eq!(cr3.size, 100);
    assert!(cr3.mtime > utc(2020, 1, 1, 0, 0, 0));
}

#[test]
fn volume_open_head_truncates_to_max() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let src = VolumeSource::new(dir.path());

    let head = src.open_head("DCIM/100CANON/IMG_0001.CR3", 10).unwrap();
    assert_eq!(head, vec![b'C'; 10]);

    // max 大于文件大小 → 返回全量
    let full = src
        .open_head("DCIM/100CANON/IMG_0001.CR3", 1 << 20)
        .unwrap();
    assert_eq!(full.len(), 100);
}

#[test]
fn volume_stream_reads_all() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let src = VolumeSource::new(dir.path());

    let mut stream = src.stream("DCIM/100CANON/IMG_0002.jpg").unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"jpeg-bytes");
}

#[test]
fn volume_id_kind_and_name() {
    // 盘符根规范化：`E:\` → `E:`（不触发文件系统访问）
    let drive = VolumeSource::new(r"E:\");
    assert_eq!(drive.id(), "E:");
    assert_eq!(drive.kind(), SourceKind::Volume);

    // tempdir：卷标可能取到（宿主盘）也可能回退根路径，仅需非空
    let dir = tempfile::tempdir().unwrap();
    let src = VolumeSource::new(dir.path());
    assert!(!src.id().is_empty());
    assert!(!src.name().is_empty());
    assert_eq!(src.kind(), SourceKind::Volume);
}

#[test]
fn volume_rejects_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let src = VolumeSource::new(dir.path());

    assert!(matches!(
        src.open_head("../escape.jpg", 8),
        Err(DeviceError::Other(_))
    ));
    assert!(matches!(
        src.stream(r"..\escape.jpg"),
        Err(DeviceError::Other(_))
    ));
}

// ---------------------------------------------------------------------------
// 热插拔纯函数
// ---------------------------------------------------------------------------

#[test]
fn hotplug_unitmask_to_drives() {
    assert_eq!(unitmask_to_drives(0), Vec::<String>::new());
    assert_eq!(unitmask_to_drives(1), ["A:"]); // bit0 = 'A'
    assert_eq!(unitmask_to_drives(1 << 4), ["E:"]);
    assert_eq!(unitmask_to_drives(1 << 25), ["Z:"]);
    assert_eq!(unitmask_to_drives((1 << 2) | (1 << 3)), ["C:", "D:"]);
    assert_eq!(unitmask_to_drives(u32::MAX).len(), 26);
}

#[test]
fn hotplug_pnp_display_name_last_nonempty_segment() {
    let pnp =
        "\\\\?\\usb#vid_04a9&pid_31f4#002166okdgn00ayzr#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";
    assert_eq!(
        pnp_display_name(pnp),
        "{6ac27878-a6fa-4155-ba85-f98f491d4f33}"
    );
    // 尾部空段跳过
    assert_eq!(pnp_display_name("usb#vid_1000#serial#"), "serial");
    // 无分隔符回退原串
    assert_eq!(pnp_display_name("single"), "single");
    // 全空段回退原串
    assert_eq!(pnp_display_name("###"), "###");
    assert_eq!(pnp_display_name(""), "");
}

// ---------------------------------------------------------------------------
// WPD 纯函数
// ---------------------------------------------------------------------------

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
