//! 引擎文件系统工具：源根推导（嵌套守卫/空目录清理）、move 后空目录
//! 深优先清理、RFC3339 时间格式化。

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use walkdir::WalkDir;

use crate::devices::folder::FOLDER_ID_PREFIX;
use crate::devices::volume::is_ignored_dir;
use crate::devices::{DeviceSource, SourceKind};

/// 已存在的目录无需再向文件服务器发 mkdir；并发创建后复查目录，
/// 避免部分 SMB 服务将已存在目录报为 Windows ERROR_ALREADY_EXISTS。
pub(super) fn ensure_directory(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        return Ok(());
    }
    match fs::create_dir_all(path) {
        Ok(()) => Ok(()),
        Err(_) if path.is_dir() => Ok(()),
        Err(error) => Err(error),
    }
}

/// 文件系统源的根目录（嵌套守卫与移动后空目录清理用）；MTP 无路径语义 → None。
pub(super) fn source_root_of(source: &dyn DeviceSource) -> Option<PathBuf> {
    let id = source.id();
    match source.kind() {
        SourceKind::Mtp => None,
        // 卷：盘符 "E:" → "E:\"；测试目录路径直接可用
        SourceKind::Volume => {
            if id.len() == 2 && id.ends_with(':') {
                Some(PathBuf::from(format!("{id}\\")))
            } else {
                Some(PathBuf::from(id))
            }
        }
        SourceKind::Folder => id.strip_prefix(FOLDER_ID_PREFIX).map(PathBuf::from),
    }
}

/// move 后清理空的源中间子目录（best-effort）：深优先逐级 `remove_dir`
/// （仅空目录可删），保留源根；跳过点前缀/系统目录（与枚举规则一致）。
/// 返回删除的目录数。
pub(super) fn cleanup_empty_dirs(root: &Path) -> usize {
    let mut dirs: Vec<PathBuf> = WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| e.depth() == 0 || !is_ignored_dir(e))
        .filter_map(Result::ok)
        .filter(|e| e.depth() > 0 && e.file_type().is_dir())
        .map(|e| e.path().to_path_buf())
        .collect();
    // 浅→深排序后反序遍历 = 深优先；子目录先删，父目录才可能为空
    dirs.sort_by_key(|d| d.components().count());
    dirs.iter()
        .rev()
        .filter(|d| fs::remove_dir(d).is_ok())
        .count()
}

pub(super) fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
