//! 导入查重分层（引擎收集端消费；**同库范围**——2026-10-09 §四 跨库重复
//! 是合法状态，照常登记不查重）：
//! ① 宽松键（size+filename+mtime±2s）begin 阶段批量预判；
//! ② 全量 (size, xxhash) 精确复核（写盘完成后）。
//! ③ 目标路径冲突策略在 mod.rs `finish_copy` 内联处理。
//! §八-1：skip 仅当存在**在线**同哈希资产——仅命中 missing 资产 = 移动/
//! 改名回归，重绑路径而非吞掉（旧路径文件已不在盘，skip 会把照片永久
//! 锁在画廊缺失态）。

use crate::db::Db;
use crate::devices::FileEntry;

use super::fsutil::rfc3339;

/// 宽松查重键的 mtime 容差。
const MTIME_TOLERANCE: chrono::Duration = chrono::Duration::seconds(2);

/// 宽松查重键命中判定（size + filename + mtime±2s；目标库内**在线**资产
/// ——`find_asset_loose` 的 missing=0 过滤保证 §八-1 口径，missing 行放行
/// 到精确层走重绑）。
pub(super) fn loose_hit(db: &Db, library_id: &str, entry: &FileEntry) -> rusqlite::Result<bool> {
    let filename = entry.rel_path.rsplit('/').next().unwrap_or(&entry.rel_path);
    let from = rfc3339(entry.mtime - MTIME_TOLERANCE);
    let to = rfc3339(entry.mtime + MTIME_TOLERANCE);
    Ok(db
        .find_asset_loose(library_id, entry.size, filename, &from, &to)?
        .is_some())
}

/// 全量精确复核判定（size + xxhash；目标库内）。§八-1 三分支：
/// - 在线同哈希 → Skip（唯一合法的 skip 条件）；
/// - 仅 missing 同哈希 → 移动/改名回归，携带资产 id 交调用方重绑路径；
/// - 无同哈希 → Fresh 照常登记。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ExactVerdict {
    /// 在线同哈希资产存在：按查重策略 skip。
    Skip,
    /// 仅 missing 同哈希资产（id）：重绑（§八-1 导入侧）。
    Rebind(i64),
    /// 目标库内无同哈希资产：照常登记。
    Fresh,
}

/// 精确复核（`find_asset_brief_by_size_xxh` 的在线优先排序兜住同指纹
/// 多行历史数据：在线行先于 missing 行命中去重）。
pub(super) fn exact_verdict(
    db: &Db,
    library_id: &str,
    size: u64,
    xxhash: u64,
) -> rusqlite::Result<ExactVerdict> {
    match db.find_asset_brief_by_size_xxh(library_id, size, xxhash)? {
        Some((id, _, true)) => Ok(ExactVerdict::Rebind(id)),
        Some(_) => Ok(ExactVerdict::Skip),
        None => Ok(ExactVerdict::Fresh),
    }
}
