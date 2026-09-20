//! 卷设备源：list 过滤/排序/归一化、open_head 截断、stream 全量读、
//! id/kind/name、路径穿越拒绝、删源（move 模式），以及热插拔纯函数
//! （unitmask→盘符、PnP 展示名）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::io::Read;

use common::{build_tree, utc};
use devices::folder::LocalFolderSource;
use devices::hotplug::{pnp_display_name, unitmask_to_drives};
use devices::volume::VolumeSource;
use devices::{DeviceError, DeviceSource, SourceKind};

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
// 删源（M2 move 模式）
// ---------------------------------------------------------------------------

#[test]
fn source_delete_removes_file_and_rejects_traversal() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());

    let vol = VolumeSource::new(dir.path());
    vol.delete("DCIM/100CANON/IMG_0001.CR3").unwrap();
    assert!(!dir.path().join("DCIM/100CANON/IMG_0001.CR3").exists());

    // 文件夹源同样可删（委托卷源）
    let folder = LocalFolderSource::new(dir.path()).unwrap();
    folder.delete("DCIM/100CANON/IMG_0002.jpg").unwrap();
    assert!(!dir.path().join("DCIM/100CANON/IMG_0002.jpg").exists());

    // 路径穿越拒绝
    assert!(matches!(
        vol.delete("../escape.jpg"),
        Err(DeviceError::Other(_))
    ));
    // 删除不存在的文件 → Io 错误（调用方记告警，不判导入失败）
    assert!(matches!(
        vol.delete("DCIM/100CANON/IMG_0001.CR3"),
        Err(DeviceError::Io(_))
    ));
}

// ---------------------------------------------------------------------------
// 热插拔纯函数（盘符掩码 / PnP 展示名）
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
