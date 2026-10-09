//! M1 T8 设备编排：设备扫描（统计快照 + 新文件预判），供弹窗/向导消费。
//!
//! 分类惰性策略：扩展名可判类的文件不读头；无法按扩展名判类的才 `open_head`
//! 做魔数识别。新文件数用宽松键（size+filename+mtime±2s，与 T7 §查重①同源）
//! 预判——真正的精确查重发生在导入时的全量哈希复核。

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use super::{classify, DeviceResult, DeviceSource};
use crate::db::Db;
use crate::events::{AssetKind, SourceKind};

/// mtime 容差：与引擎宽松查重键保持一致。
const MTIME_TOLERANCE: chrono::Duration = chrono::Duration::seconds(2);

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

/// 扫描设备：list → 分类统计 + 新文件预判。
pub fn scan_device<'a>(
    source: &dyn DeviceSource,
    db: impl Into<Option<&'a Db>>,
    skip_imported: bool,
) -> DeviceResult<DeviceSnapshot> {
    scan_device_with_progress(source, db.into(), skip_imported, None)
}

pub fn scan_device_with_progress(
    source: &dyn DeviceSource,
    db: Option<&Db>,
    skip_imported: bool,
    on_batch: Option<super::FileBatchCallback>,
) -> DeviceResult<DeviceSnapshot> {
    let entries = match on_batch {
        Some(callback) => source.list_with_progress(callback)?,
        None => source.list()?,
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
        snapshot.bytes_total += entry.size;
        // 惰性头读：扩展名可判类则不读头（Volume 源 list 已按媒体扩展名过滤）
        let mut kind = classify(&entry.rel_path, &[]);
        if kind == AssetKind::Other {
            let head = source.open_head(&entry.id, 64 * 1024)?;
            kind = classify(&entry.rel_path, &head);
        }
        *snapshot.files_by_kind.entry(kind).or_insert(0) += 1;

        // 新文件 = 未开启跳过导入，或宽松键未命中既有资产
        if !skip_imported || !db.is_some_and(|db| loose_imported(db, entry)) {
            snapshot.new_files += 1;
        }
    }
    Ok(snapshot)
}

/// 宽松键已导入判定（查询失败按“未导入”处理——宁可多弹不可漏弹；
/// 全库口径的 UI 提示，导入查重闸门在引擎内按目标照片库范围执行）。
fn loose_imported(db: &Db, entry: &super::FileEntry) -> bool {
    let filename = entry.rel_path.rsplit('/').next().unwrap_or(&entry.rel_path);
    let from = rfc3339(entry.mtime - MTIME_TOLERANCE);
    let to = rfc3339(entry.mtime + MTIME_TOLERANCE);
    db.find_asset_loose_any(entry.size, filename, &from, &to)
        .map(|hit| hit.is_some())
        .unwrap_or(false)
}

fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Millis, true)
}
