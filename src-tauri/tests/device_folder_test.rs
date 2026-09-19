//! 本地文件夹源（M2“从文件夹导入”）：稳定 id（用户输入形态）、
//! 枚举规则与卷源一致、缺目录报错、exclude 子树排除。

mod common;

pub use common::{db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs};

use std::fs;
use std::io::Read;
use std::path::Path;

use common::build_tree;
use devices::folder::LocalFolderSource;
use devices::{DeviceError, DeviceSource, SourceKind};

#[test]
fn folder_source_lists_media_with_stable_id() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());

    let src = LocalFolderSource::new(dir.path()).unwrap();
    // id 形如 FOLDER:<规范化绝对路径>，不含 verbatim 前缀
    let id = src.id();
    let root = id.strip_prefix("FOLDER:").expect("id 应以 FOLDER: 开头");
    assert!(!root.starts_with(r"\\?\"), "不暴露 verbatim 路径: {root}");
    // 路径真实存在（strip 后仍可解析）
    assert!(Path::new(root).is_dir());
    assert_eq!(src.kind(), SourceKind::Folder);
    assert!(!src.name().is_empty());

    // 同一路径（未规范化的变体写法）→ 同一 id
    let variant = dir.path().join("."); // 尾部 `/.`
    assert_eq!(LocalFolderSource::new(&variant).unwrap().id(), id);

    let files = src.list().unwrap();
    let rels: Vec<&str> = files.iter().map(|f| f.rel_path.as_str()).collect();
    // 点前缀目录 / 系统目录 / 非媒体扩展名全部跳过（与卷源同规则）
    assert_eq!(
        rels,
        [
            "DCIM/100CANON/IMG_0001.CR3",
            "DCIM/100CANON/IMG_0002.jpg",
            "DCIM/100CANON/MVI_0003.MP4",
        ]
    );

    // open_head / stream 与卷源同等可用
    let head = src.open_head("DCIM/100CANON/IMG_0001.CR3", 10).unwrap();
    assert_eq!(head, vec![b'C'; 10]);
    let mut stream = src.stream("DCIM/100CANON/IMG_0002.jpg").unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).unwrap();
    assert_eq!(buf, b"jpeg-bytes");
}

#[test]
fn folder_source_missing_dir_is_error() {
    let err = LocalFolderSource::new(r"C:\definitely\not\exist\dir")
        .err()
        .expect("不存在目录应报错");
    assert!(matches!(err, DeviceError::Io(_)));
}

#[test]
fn folder_source_excludes_subtree() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    // DCIM 直属一个媒体文件（区分“排除整棵 DCIM”与“仅排除 100CANON”）
    fs::write(dir.path().join("DCIM/IMG_TOP.jpg"), b"jpeg-top").unwrap();

    // 排除目标子树：DCIM 整棵跳过 → 空（本树其余媒体都在 DCIM 下）
    let exclude = dir.path().join("DCIM");
    let src = LocalFolderSource::with_exclude(dir.path(), Some(&exclude)).unwrap();
    assert!(src.list().unwrap().is_empty());

    // 排除更深的子目录：仅跳过 100CANON，DCIM 直属文件保留
    let exclude_deep = dir.path().join("DCIM").join("100CANON");
    let src = LocalFolderSource::with_exclude(dir.path(), Some(&exclude_deep)).unwrap();
    let rels: Vec<String> = src
        .list()
        .unwrap()
        .into_iter()
        .map(|f| f.rel_path)
        .collect();
    assert_eq!(rels, ["DCIM/IMG_TOP.jpg"]);

    // 排除不存在的路径 → 忽略（全量枚举）
    let nope = dir.path().join("nope");
    let src = LocalFolderSource::with_exclude(dir.path(), Some(&nope)).unwrap();
    assert_eq!(src.list().unwrap().len(), 4);
}
