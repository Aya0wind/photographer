//! M1 T8 设备编排测试：扫描快照统计、宽松键 new_files 预判、惰性头读。

#[path = "../src/db/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod db;
#[path = "../src/devices/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod devices;
#[path = "../src/events/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod events;

use std::fs;

use chrono::{SecondsFormat, Utc};
use db::{AssetRow, Db};
use devices::orchestrator::scan_device;
use devices::volume::VolumeSource;
use devices::{DeviceResult, DeviceSource, FileEntry, SourceKind};
use events::AssetKind;

fn build_tree(dir: &std::path::Path) {
    fs::create_dir_all(dir.join("DCIM")).unwrap();
    fs::write(dir.join("DCIM/A.jpg"), vec![0u8; 100]).unwrap();
    fs::write(dir.join("DCIM/B.nef"), vec![0u8; 200]).unwrap();
    fs::write(dir.join("DCIM/C.mp4"), vec![0u8; 50]).unwrap();
    fs::write(dir.join("DCIM/notes.txt"), vec![0u8; 7]).unwrap(); // 非媒体：list 层排除
}

fn open_db(dir: &std::path::Path) -> Db {
    let db = Db::open(&dir.join("library.db")).unwrap();
    db.migrate().unwrap();
    db
}

#[test]
fn snapshot_counts_by_kind_and_bytes() {
    let src = tempfile::tempdir().unwrap();
    build_tree(src.path());
    let db_dir = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let source = VolumeSource::new(src.path());

    let snapshot = scan_device(&source, &db, true).unwrap();

    assert_eq!(snapshot.id, source.id());
    assert_eq!(snapshot.kind, SourceKind::Volume);
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Photo), Some(&1));
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Raw), Some(&1));
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Video), Some(&1));
    assert_eq!(snapshot.bytes_total, 350);
    assert_eq!(snapshot.new_files, 3, "空库应全部视为新文件");
    // serde：filesByKind 为字符串 key map
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["filesByKind"]["photo"], 1);
    assert_eq!(json["bytesTotal"], 350);
    assert_eq!(json["newFiles"], 3);
}

#[test]
fn skip_imported_deducts_loose_matched_assets() {
    let src = tempfile::tempdir().unwrap();
    build_tree(src.path());
    let db_dir = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());

    // 预置一行匹配 A.jpg 的资产（size+filename+mtime±2s 宽松键）
    let mtime: chrono::DateTime<Utc> = fs::metadata(src.path().join("DCIM/A.jpg"))
        .unwrap()
        .modified()
        .unwrap()
        .into();
    db.insert_asset(&AssetRow {
        path: r"Y:\照片\2020\A.jpg".into(),
        filename: "A.jpg".into(),
        size: 100,
        mtime: mtime.to_rfc3339_opts(SecondsFormat::Millis, true),
        xxhash: 42,
        sha256: [7u8; 32],
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: mtime.to_rfc3339_opts(SecondsFormat::Millis, true),
    })
    .unwrap();

    let source = VolumeSource::new(src.path());
    let snapshot = scan_device(&source, &db, true).unwrap();
    assert_eq!(snapshot.new_files, 2, "宽松命中的 A.jpg 不计入新文件");
    assert_eq!(
        snapshot.files_by_kind.get(&AssetKind::Photo),
        Some(&1),
        "统计不受查重影响"
    );

    // 关闭 skip_imported：全部视为新文件
    let all_new = scan_device(&source, &db, false).unwrap();
    assert_eq!(all_new.new_files, 3);
}

/// 惰性头读：无扩展名/未知扩展名的条目靠魔数判类。
struct StubSource;

impl DeviceSource for StubSource {
    fn id(&self) -> String {
        "stub".into()
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Volume
    }
    fn name(&self) -> String {
        "stub".into()
    }
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        Ok(vec![FileEntry {
            id: "noext".into(),
            rel_path: "noext".into(),
            size: 4,
            mtime: Utc::now(),
        }])
    }
    fn open_head(&self, _id: &str, _max: u64) -> DeviceResult<Vec<u8>> {
        Ok(vec![0xFF, 0xD8, 0xFF, 0xE0]) // JPEG 魔数
    }
    fn stream(&self, _id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        Err(devices::DeviceError::NotSupported("stub".into()))
    }
}

#[test]
fn unknown_extension_falls_back_to_magic_head_read() {
    let db_dir = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let snapshot = scan_device(&StubSource, &db, true).unwrap();
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Photo), Some(&1));
    assert_eq!(snapshot.new_files, 1);
}
