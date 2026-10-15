//! 设备发现只枚举名称并统计；详细统计用于显式监视导入的预判。

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::{classify, DeviceResult, DeviceSource};
use crate::db::Db;
use crate::events::{AssetKind, SourceKind};

/// 设备扫描快照（导入弹窗数据源；serde camelCase，filesByKind 为
/// 字符串 key 的 map——serde_json 对单位变体 key 自动按字符串序列化）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceSnapshot {
    pub id: String,
    pub name: String,
    pub kind: SourceKind,
    /// 按类型统计的文件数（photo/raw/other）。
    pub files_by_kind: BTreeMap<AssetKind, u64>,
    /// 详细预判的媒体总字节；轻量发现阶段为 0（尚未读取）。
    pub bytes_total: u64,
    /// 详细预判的“未导入”文件数；轻量发现阶段为 0（尚未查重）。
    pub new_files: u64,
}

/// 监视文件夹导入预判：读取目录属性与数据库键，不读取照片内容。
pub fn scan_device<'a>(
    source: &dyn DeviceSource,
    db: impl Into<Option<&'a Db>>,
    skip_imported: bool,
) -> DeviceResult<DeviceSnapshot> {
    let db = db.into();
    let entries = source.list()?;
    let mut snapshot = DeviceSnapshot {
        id: source.id(),
        name: source.name(),
        kind: source.kind(),
        files_by_kind: BTreeMap::new(),
        bytes_total: 0,
        new_files: 0,
    };
    for entry in &entries {
        snapshot.bytes_total += entry.size;
        *snapshot
            .files_by_kind
            .entry(classify(&entry.rel_path, &[]))
            .or_insert(0) += 1;
        if !skip_imported || !db.is_some_and(|db| loose_imported(db, entry)) {
            snapshot.new_files += 1;
        }
    }
    Ok(snapshot)
}

pub fn scan_device_with_progress(
    source: &dyn DeviceSource,
    _db: Option<&Db>,
    _skip_imported: bool,
    _on_batch: Option<super::FileBatchCallback>,
) -> DeviceResult<DeviceSnapshot> {
    let started = std::time::Instant::now();
    if let Some(files_by_kind) = source.fast_file_counts()? {
        super::diagnostics::record(format!(
            "device count complete: {}; {} files; elapsed={:?}",
            source.id(),
            files_by_kind.values().sum::<u64>(),
            started.elapsed()
        ));
        return Ok(DeviceSnapshot {
            id: source.id(),
            name: source.name(),
            kind: source.kind(),
            files_by_kind,
            bytes_total: 0,
            new_files: 0,
        });
    }
    let entries = source.list_names()?;
    let mut snapshot = DeviceSnapshot {
        id: source.id(),
        name: source.name(),
        kind: source.kind(),
        files_by_kind: BTreeMap::new(),
        bytes_total: 0,
        new_files: 0,
    };
    for entry in &entries {
        // 分类仅用文件名；未知格式不为点数读取内容。
        let kind = classify(&entry.rel_path, &[]);
        *snapshot.files_by_kind.entry(kind).or_insert(0) += 1;
    }
    super::diagnostics::record(format!(
        "device count fallback complete: {}; {} files; elapsed={:?}",
        source.id(),
        entries.len(),
        started.elapsed()
    ));
    Ok(snapshot)
}

fn loose_imported(db: &Db, entry: &super::FileEntry) -> bool {
    let filename = entry.rel_path.rsplit('/').next().unwrap_or(&entry.rel_path);
    let format = |time: DateTime<Utc>| time.to_rfc3339_opts(SecondsFormat::Millis, true);
    db.find_asset_loose_any(
        entry.size,
        filename,
        &format(entry.mtime - chrono::Duration::seconds(2)),
        &format(entry.mtime + chrono::Duration::seconds(2)),
    )
    .map(|hit| hit.is_some())
    .unwrap_or(false)
}

#[cfg(test)]
mod fast_count_tests {
    use super::*;
    struct Counter;
    impl DeviceSource for Counter {
        fn id(&self) -> String {
            "camera".into()
        }
        fn name(&self) -> String {
            "Camera".into()
        }
        fn kind(&self) -> SourceKind {
            SourceKind::Mtp
        }
        fn fast_file_counts(&self) -> DeviceResult<Option<BTreeMap<AssetKind, u64>>> {
            Ok(Some(BTreeMap::from([
                (AssetKind::Photo, 407),
                (AssetKind::Raw, 990),
            ])))
        }
        fn list(&self) -> DeviceResult<Vec<super::super::FileEntry>> {
            panic!("Counting must not list files")
        }
        fn list_names(&self) -> DeviceResult<Vec<super::super::FileEntry>> {
            panic!("Counting must not read names")
        }
        fn open_head(&self, _: &str, _: u64) -> DeviceResult<Vec<u8>> {
            panic!("Counting must not read content")
        }
        fn stream(&self, _: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
            panic!("Counting must not transfer photos")
        }
    }
    #[test]
    fn counting_finishes_without_enumerating_or_reading_any_file() {
        let callback: super::super::FileBatchCallback =
            std::sync::Arc::new(|_| panic!("Names are deferred"));
        let snapshot = scan_device_with_progress(&Counter, None, false, Some(callback)).unwrap();
        assert_eq!(snapshot.files_by_kind.values().sum::<u64>(), 1397);
        assert_eq!(snapshot.bytes_total, 0);
    }
}

#[cfg(test)]
mod discovery_tests {
    use super::*;
    use crate::devices::{
        folder::LocalFolderSource, selection::LocalPathsSource, volume::VolumeSource,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn local_sources_count_without_names_then_list_names_without_file_attributes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("DCIM")).unwrap();
        std::fs::write(root.path().join("DCIM/A.JPG"), b"jpeg").unwrap();
        std::fs::write(root.path().join("DCIM/B.NEF"), b"raw").unwrap();
        std::fs::write(root.path().join("DCIM/movie.mp4"), b"video").unwrap();
        let sources: Vec<Box<dyn DeviceSource>> = vec![
            Box::new(VolumeSource::new(root.path())),
            Box::new(LocalFolderSource::new(root.path()).unwrap()),
            Box::new(
                LocalPathsSource::new(&[
                    root.path().to_string_lossy().into_owned(),
                    root.path()
                        .join("DCIM/A.JPG")
                        .to_string_lossy()
                        .into_owned(),
                ])
                .unwrap(),
            ),
        ];
        for source in sources {
            let callback = Arc::new(|_| panic!("Discovery must not publish file names"));
            let counts = scan_device_with_progress(&*source, None, false, Some(callback)).unwrap();
            assert_eq!(
                counts.files_by_kind,
                BTreeMap::from([(AssetKind::Photo, 1), (AssetKind::Raw, 1)])
            );
            assert_eq!((counts.bytes_total, counts.new_files), (0, 0));
            let names = source.list_names().unwrap();
            assert_eq!(names.len(), 2);
            assert!(names
                .iter()
                .all(|entry| entry.size == 0 && entry.mtime == DateTime::UNIX_EPOCH));
            let ids: Vec<_> = names.iter().map(|entry| entry.id.clone()).collect();
            let sizes = source.file_sizes(&ids).unwrap().unwrap();
            assert_eq!(sizes.iter().map(|(_, size)| size).sum::<u64>(), 7);
            // Actual import still gets real attributes, never discovery placeholders.
            let detailed = source.list().unwrap();
            assert!(detailed
                .iter()
                .all(|entry| entry.size > 0 && entry.mtime > DateTime::UNIX_EPOCH));
        }
    }

    #[test]
    fn counts_refresh_and_excluded_subtrees_are_not_enumerated() {
        let root = tempfile::tempdir().unwrap();
        let excluded = root.path().join("excluded");
        std::fs::create_dir(&excluded).unwrap();
        std::fs::write(excluded.join("hidden.jpg"), b"no").unwrap();
        std::fs::write(root.path().join("a.jpg"), b"yes").unwrap();
        let source = LocalFolderSource::with_exclude(root.path(), Some(&excluded)).unwrap();
        assert_eq!(
            source.fast_file_counts().unwrap().unwrap()[&AssetKind::Photo],
            1
        );
        std::fs::write(root.path().join("b.jpg"), b"yes").unwrap();
        assert_eq!(
            source.fast_file_counts().unwrap().unwrap()[&AssetKind::Photo],
            2
        );
        let batches = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&batches);
        let names = source
            .list_names_with_progress(Arc::new(move |batch| output.lock().unwrap().extend(batch)))
            .unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(batches.lock().unwrap().len(), 2);
        assert!(names
            .iter()
            .all(|entry| !entry.rel_path.starts_with("excluded")));
    }
}
