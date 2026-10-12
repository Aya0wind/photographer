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
    /// 全部媒体文件总字节。
    pub bytes_total: u64,
    /// 宽松键预判的“未导入”文件数。
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
    on_batch: Option<super::FileBatchCallback>,
) -> DeviceResult<DeviceSnapshot> {
    if let Some(files_by_kind) = source.fast_file_counts()? {
        return Ok(DeviceSnapshot { id: source.id(), name: source.name(), kind: source.kind(), files_by_kind, bytes_total: 0, new_files: 0 });
    }
    let entries = match on_batch {
        Some(callback) => source.list_names_with_progress(callback)?,
        None => source.list_names()?,
    };
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
        snapshot.bytes_total += entry.size;
        let kind = classify(&entry.rel_path, &[]);
        *snapshot.files_by_kind.entry(kind).or_insert(0) += 1;
    }
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
        fn id(&self) -> String { "camera".into() }
        fn name(&self) -> String { "Camera".into() }
        fn kind(&self) -> SourceKind { SourceKind::Mtp }
        fn fast_file_counts(&self) -> DeviceResult<Option<BTreeMap<AssetKind, u64>>> {
            Ok(Some(BTreeMap::from([(AssetKind::Photo, 407), (AssetKind::Raw, 990)])))
        }
        fn list(&self) -> DeviceResult<Vec<super::super::FileEntry>> { panic!("Counting must not list files") }
        fn list_names(&self) -> DeviceResult<Vec<super::super::FileEntry>> { panic!("Counting must not read names") }
        fn open_head(&self, _: &str, _: u64) -> DeviceResult<Vec<u8>> { panic!("Counting must not read content") }
        fn stream(&self, _: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> { panic!("Counting must not transfer photos") }
    }
    #[test]
    fn counting_finishes_without_enumerating_or_reading_any_file() {
        let callback: super::super::FileBatchCallback = std::sync::Arc::new(|_| panic!("Names are deferred"));
        let snapshot = scan_device_with_progress(&Counter, None, false, Some(callback)).unwrap();
        assert_eq!(snapshot.files_by_kind.values().sum::<u64>(), 1397);
        assert_eq!(snapshot.bytes_total, 0);
    }
}
