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

/// `FILE_FLAG_SEQUENTIAL_SCAN`：提示系统按顺序访问优化预读（spec §5.1）。
#[cfg(windows)]
const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;

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
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.custom_flags(FILE_FLAG_SEQUENTIAL_SCAN);
        }
        Ok(opts.open(path)?)
    }
}

impl DeviceSource for VolumeSource {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Volume
    }

    fn name(&self) -> String {
        volume_label(&self.root).unwrap_or_else(|| self.root.to_string_lossy().into_owned())
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        let mut out = Vec::new();
        let walker = WalkDir::new(&self.root)
            .into_iter()
            .filter_entry(|e| !is_ignored_dir(e));
        for entry in walker {
            let entry = entry.map_err(|e| DeviceError::Io(e.into()))?;
            if !entry.file_type().is_file() {
                continue;
            }
            let ext = entry
                .path()
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_default();
            if !is_media_ext(&ext) {
                continue;
            }
            let rel = match entry.path().strip_prefix(&self.root) {
                Ok(rel) => rel,
                Err(_) => continue, // 理论不可达：walkdir 条目均以 root 为前缀
            };
            let rel_path = normalize_rel_path(&rel.to_string_lossy());
            let meta = entry.metadata().map_err(|e| DeviceError::Io(e.into()))?;
            out.push(FileEntry {
                mtime: meta.modified().map_err(DeviceError::Io)?.into(),
                size: meta.len(),
                id: rel_path.clone(),
                rel_path,
            });
        }
        out.sort_unstable_by(|a, b| a.rel_path.cmp(&b.rel_path));
        Ok(out)
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

/// 查询卷标（GetVolumeInformationW）；未挂载/无标签/失败时返回 None，
/// 调用方回退到根路径展示。
#[cfg(windows)]
fn volume_label(root: &Path) -> Option<String> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;

    let mut probe = root.to_string_lossy().into_owned();
    // 盘符根（"E:"）必须补尾反斜杠才能作为卷根查询
    if probe.len() == 2 && probe.ends_with(':') {
        probe.push('\\');
    }
    let wide: Vec<u16> = std::ffi::OsStr::new(&probe)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut name = [0u16; 256];
    // SAFETY: wide 以 NUL 结尾且在本调用内存活；name 为合法可写缓冲
    let ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            Some(&mut name),
            None,
            None,
            None,
            None,
        )
    };
    if ok.is_err() {
        return None;
    }
    let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
    if len == 0 {
        None
    } else {
        Some(String::from_utf16_lossy(&name[..len]))
    }
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn volume_label(_root: &Path) -> Option<String> {
    None
}

/// 热插拔卷到达时查询卷标（drive 形如 `E:`）。
#[cfg(windows)]
pub(crate) fn drive_label(drive: &str) -> Option<String> {
    volume_label(Path::new(drive))
}

#[cfg(not(windows))]
#[allow(dead_code)]
pub(crate) fn drive_label(_drive: &str) -> Option<String> {
    None
}
