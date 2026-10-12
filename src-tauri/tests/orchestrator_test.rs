//! M1 T8 设备编排测试：扫描快照统计、宽松键 new_files 预判、惰性头读。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, settings, tasks, thumbs,
};

use std::fs;

use chrono::{SecondsFormat, Utc};
use db::AssetRow;
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

#[test]
fn snapshot_counts_by_kind_and_bytes() {
    let src = tempfile::tempdir().unwrap();
    build_tree(src.path());
    let db_dir = tempfile::tempdir().unwrap();
    let db = common::open_db(db_dir.path());
    let source = VolumeSource::new(src.path());

    let snapshot = scan_device(&source, &db, true).unwrap();

    assert_eq!(snapshot.id, source.id());
    assert_eq!(snapshot.kind, SourceKind::Volume);
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Photo), Some(&1));
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Raw), Some(&1));
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Video), None);
    assert_eq!(snapshot.bytes_total, 300);
    assert_eq!(snapshot.new_files, 2, "空库应全部视为新图片");
    // serde：filesByKind 为字符串 key map
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["filesByKind"]["photo"], 1);
    assert_eq!(json["bytesTotal"], 300);
    assert_eq!(json["newFiles"], 2);
}

#[test]
fn skip_imported_deducts_loose_matched_assets() {
    let src = tempfile::tempdir().unwrap();
    build_tree(src.path());
    let db_dir = tempfile::tempdir().unwrap();
    let db = common::open_db(db_dir.path());

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
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: mtime.to_rfc3339_opts(SecondsFormat::Millis, true),
        origin: "imported".into(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();

    let source = VolumeSource::new(src.path());
    let snapshot = scan_device(&source, &db, true).unwrap();
    assert_eq!(
        snapshot.new_files, 1,
        "宽松命中的 A.jpg 与视频都不计入新文件"
    );
    assert_eq!(
        snapshot.files_by_kind.get(&AssetKind::Photo),
        Some(&1),
        "统计不受查重影响"
    );

    // 关闭 skip_imported：全部视为新文件
    let all_new = scan_device(&source, &db, false).unwrap();
    assert_eq!(all_new.new_files, 2);
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
        panic!("Discovery must not request the detailed file list")
    }
    fn list_names(&self) -> DeviceResult<Vec<FileEntry>> {
        Ok(vec![FileEntry {
            id: "noext".into(),
            rel_path: "noext".into(),
            size: 4,
            mtime: Utc::now(),
        }])
    }
    fn open_head(&self, _id: &str, _max: u64) -> DeviceResult<Vec<u8>> {
        panic!("Discovery must never read file contents")
    }
    fn stream(&self, _id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        Err(devices::DeviceError::NotSupported("stub".into()))
    }
}

#[test]
fn discovery_keeps_cheap_metadata_without_reading_contents() {
    let dir = tempfile::tempdir().unwrap();
    build_tree(dir.path());
    let source = VolumeSource::new(dir.path());
    let names = source.list_names().unwrap();
    assert_eq!(names.len(), 2);
    assert_eq!(names.iter().map(|file| file.size).sum::<u64>(), 300);
    let full = source.list().unwrap();
    assert_eq!(full.iter().map(|file| file.size).sum::<u64>(), 300);
    let snapshot = devices::orchestrator::scan_device_with_progress(&source, None, false, None).unwrap();
    assert_eq!(snapshot.files_by_kind.values().sum::<u64>(), 2);
    assert_eq!(snapshot.bytes_total, 300);
}

#[test]
fn discovery_counts_unknown_files_without_reading_contents() {
    let db_dir = tempfile::tempdir().unwrap();
    let db = common::open_db(db_dir.path());
    let snapshot = devices::orchestrator::scan_device_with_progress(&StubSource, Some(&db), true, None).unwrap();
    assert_eq!(snapshot.files_by_kind.get(&AssetKind::Other), Some(&1));
    assert_eq!(snapshot.bytes_total, 4);
    assert_eq!(snapshot.new_files, 0);
}
