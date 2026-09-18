//! 导入查重分层（引擎收集端消费）：
//! ① 宽松键（size+filename+mtime±2s）begin 阶段批量预判；
//! ② 全量 (size, xxhash) 精确复核（写盘完成后）。
//! ③ 目标路径冲突策略在 mod.rs `finish_copy` 内联处理。

use crate::db::Db;
use crate::devices::FileEntry;

use super::fsutil::rfc3339;

/// 宽松查重键的 mtime 容差。
const MTIME_TOLERANCE: chrono::Duration = chrono::Duration::seconds(2);

/// 宽松查重键命中判定（size + filename + mtime±2s）。
pub(super) fn loose_hit(db: &Db, entry: &FileEntry) -> rusqlite::Result<bool> {
    let filename = entry.rel_path.rsplit('/').next().unwrap_or(&entry.rel_path);
    let from = rfc3339(entry.mtime - MTIME_TOLERANCE);
    let to = rfc3339(entry.mtime + MTIME_TOLERANCE);
    Ok(db
        .find_asset_loose(entry.size, filename, &from, &to)?
        .is_some())
}

/// 全量精确复核命中判定（size + xxhash）：库内已有同指纹资产即命中。
pub(super) fn exact_hit(db: &Db, size: u64, xxhash: u64) -> rusqlite::Result<bool> {
    Ok(db.find_asset_by_size_xxh(size, xxhash)?.is_some())
}
