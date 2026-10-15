//! 卷设备源（读卡器/U 盘）：`DeviceSource` 的文件系统实现。
//!
//! 生产环境传盘符根（如 `E:\`），测试传任意目录。`list` 用 walkdir 递归遍历，
//! 忽略 `System Volume Information` / `$RECYCLE.BIN` / `.` 开头目录；读取统一带
//! `FILE_FLAG_SEQUENTIAL_SCAN`（顺序预读提示，提高大文件导入吞吐）。
//! 并发流上限由导入引擎控制，本层不设限。

use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use super::{is_media_ext, normalize_rel_path, DeviceError, DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

/// 卷内非用户数据目录（spec §5.1：跳过系统目录）。
const IGNORED_DIRS: &[&str] = &["System Volume Information", "$RECYCLE.BIN"];

/// 卷设备源：root 为卷根（`E:\`）或任意测试目录。
pub struct VolumeSource {
    root: PathBuf,
    /// 规范化设备标识：`E:\` → `E:`；目录路径去尾部分隔符
    id: String,
}

impl VolumeSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let id = normalize_root_id(&root);
        Self { root, id }
    }

    /// 拒绝越界路径（防目录穿越），返回卷内绝对路径。
    fn resolve(&self, id: &str) -> DeviceResult<PathBuf> {
        if id.split(['/', '\\']).any(|seg| seg == "..") {
            return Err(DeviceError::Other(format!("invalid file id: {id}")));
        }
        Ok(self.root.join(id))
    }

    /// 打开卷内文件（顺序读标志）。
    fn open_file(&self, id: &str) -> DeviceResult<File> {
        let path = self.resolve(id)?;
        let mut opts = OpenOptions::new();
        opts.read(true);
        crate::platform::configure_sequential_read(&mut opts);
        Ok(opts.open(path)?)
    }
}

impl VolumeSource {
    fn walk_media(
        &self,
        exclude: Option<&str>,
        mut visit: impl FnMut(walkdir::DirEntry) -> DeviceResult<()>,
    ) -> DeviceResult<()> {
        let walker = WalkDir::new(&self.root).into_iter().filter_entry(|entry| {
            !is_ignored_dir(entry)
                && !exclude.is_some_and(|excluded| {
                    entry
                        .path()
                        .strip_prefix(&self.root)
                        .is_ok_and(|relative| relative.starts_with(excluded))
                })
        });
        for entry in walker {
            let entry = entry.map_err(|error| DeviceError::Io(error.into()))?;
            if entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|ext| is_media_ext(&ext.to_string_lossy()))
            {
                visit(entry)?;
            }
        }
        Ok(())
    }

    pub(super) fn count_files(
        &self,
        exclude: Option<&str>,
    ) -> DeviceResult<std::collections::BTreeMap<crate::events::AssetKind, u64>> {
        let mut counts = std::collections::BTreeMap::new();
        self.walk_media(exclude, |entry| {
            *counts
                .entry(super::classify(&entry.file_name().to_string_lossy(), &[]))
                .or_insert(0) += 1;
            Ok(())
        })?;
        Ok(counts)
    }

    pub(super) fn enumerate(
        &self,
        detailed: bool,
        exclude: Option<&str>,
        on_batch: Option<super::FileBatchCallback>,
    ) -> DeviceResult<Vec<FileEntry>> {
        let mut out = Vec::new();
        let mut sent = 0;
        let mut emitted = std::time::Instant::now();
        self.walk_media(exclude, |entry| {
            let rel = entry
                .path()
                .strip_prefix(&self.root)
                .map_err(|error| DeviceError::Other(error.to_string()))?;
            let rel_path = normalize_rel_path(&rel.to_string_lossy());
            // Names and counts must never stat each photo or open its contents.
            let (size, mtime) = if detailed {
                let metadata = entry
                    .metadata()
                    .map_err(|error| DeviceError::Io(error.into()))?;
                (metadata.len(), metadata.modified()?.into())
            } else {
                (0, chrono::DateTime::UNIX_EPOCH)
            };
            out.push(FileEntry {
                id: rel_path.clone(),
                rel_path,
                size,
                mtime,
            });
            if let Some(callback) = &on_batch {
                if sent == 0 || emitted.elapsed() >= std::time::Duration::from_millis(150) {
                    callback(out[sent..].to_vec());
                    sent = out.len();
                    emitted = std::time::Instant::now();
                }
            }
            Ok(())
        })?;
        if let Some(callback) = on_batch {
            if sent < out.len() || out.is_empty() {
                callback(out[sent..].to_vec());
            }
        }
        // The UI groups/sorts after names finish. Import's detailed list stays sorted.
        if detailed {
            out.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
        }
        Ok(out)
    }
}

impl DeviceSource for VolumeSource {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Volume
    }

    fn copy_only(&self) -> bool {
        super::present::is_storage_card(&self.root)
    }

    fn name(&self) -> String {
        crate::platform::volume_label(&self.root)
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        self.enumerate(true, None, None)
    }
    fn list_names(&self) -> DeviceResult<Vec<FileEntry>> {
        self.enumerate(false, None, None)
    }

    fn fast_file_counts(
        &self,
    ) -> DeviceResult<Option<std::collections::BTreeMap<crate::events::AssetKind, u64>>> {
        self.count_files(None).map(Some)
    }
    fn list_names_with_progress(
        &self,
        callback: super::FileBatchCallback,
    ) -> DeviceResult<Vec<FileEntry>> {
        self.enumerate(false, None, Some(callback))
    }

    fn file_sizes(&self, ids: &[String]) -> DeviceResult<Option<Vec<(String, u64)>>> {
        ids.iter()
            .map(|id| Ok((id.clone(), fs::metadata(self.resolve(id)?)?.len())))
            .collect::<DeviceResult<Vec<_>>>()
            .map(Some)
    }

    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        let file = self.open_file(id)?;
        let mut buf = Vec::new();
        file.take(max)
            .read_to_end(&mut buf)
            .map_err(DeviceError::Io)?;
        Ok(buf)
    }

    fn stream(&self, id: &str) -> DeviceResult<Box<dyn Read + Send>> {
        Ok(Box::new(self.open_file(id)?))
    }

    fn local_path(&self, id: &str) -> Option<PathBuf> {
        self.resolve(id).ok()
    }

    /// 删源（move 模式）：删除卷内文件。目录不随之清理（引擎负责空目录清理）。
    fn delete(&self, id: &str) -> DeviceResult<()> {
        Ok(fs::remove_file(self.resolve(id)?)?)
    }
}

/// 根路径规范化为设备 ID：`E:\` → `E:`，普通目录去尾部分隔符。
fn normalize_root_id(root: &Path) -> String {
    let s = root.to_string_lossy();
    let trimmed = s.trim_end_matches(['\\', '/']);
    if trimmed.is_empty() {
        s.into_owned()
    } else {
        trimmed.to_string()
    }
}

/// walkdir 过滤谓词：跳过系统目录与隐藏目录（根本身不跳过）。
/// 引擎的移动后空目录清理复用同一规则。
pub(crate) fn is_ignored_dir(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() == 0 || !entry.file_type().is_dir() {
        return false;
    }
    let name = entry.file_name().to_string_lossy();
    IGNORED_DIRS.contains(&name.as_ref()) || name.starts_with('.')
}

/// 热插拔卷到达时查询卷标；平台实现决定盘符/挂载点语义。
pub(crate) fn drive_label(drive: &str) -> Option<String> {
    crate::platform::volume_label(Path::new(drive))
}
