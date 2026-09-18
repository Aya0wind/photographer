//! 本地文件夹源（M2，spec §5.11“从文件夹导入”）：把用户自选的任意目录
//! 统一为 `DeviceSource`，与卷/相机走同一条导入流水线。
//!
//! id 规范化为 `FOLDER:<canonical 绝对路径>`（剔除 `\\?\` verbatim 前缀，
//! 便于前端展示与重复选择同一目录时命中同一注册表键）。
//! 枚举复用卷源规则（媒体扩展名过滤 + 跳过点前缀/系统目录）；可选
//! `exclude` 子树在枚举时整体跳过（自我嵌套守卫的防御层：目标收纳区
//! 落在源内时，源枚举不产出目标树内的文件）。

use std::fs;
use std::path::{Path, PathBuf};

use super::volume::VolumeSource;
use super::{DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

/// 文件夹源 id 前缀。
pub const FOLDER_ID_PREFIX: &str = "FOLDER:";

/// 本地文件夹源。
pub struct LocalFolderSource {
    /// 委托卷源实现（walkdir 枚举/过滤/读取完全同规则）。
    inner: VolumeSource,
    /// canonical 根（内部使用，含 verbatim 前缀）。
    root: PathBuf,
    id: String,
    /// 枚举时排除的子树（canonical；目标收纳区）。
    exclude: Option<PathBuf>,
}

impl LocalFolderSource {
    /// 打开文件夹源；目录不存在/不可访问 → `DeviceError::Io`。
    pub fn new(root: impl AsRef<Path>) -> DeviceResult<Self> {
        Self::with_exclude(root, None)
    }

    /// 同 `new`，但枚举时排除 `exclude` 子树（canonical 化失败则忽略）。
    pub fn with_exclude(root: impl AsRef<Path>, exclude: Option<&Path>) -> DeviceResult<Self> {
        let canonical = fs::canonicalize(root.as_ref())?;
        let inner = VolumeSource::new(&canonical);
        Ok(Self {
            id: format!("{FOLDER_ID_PREFIX}{}", strip_verbatim(&canonical)),
            root: canonical,
            inner,
            exclude: exclude.and_then(|p| fs::canonicalize(p).ok()),
        })
    }
}

impl DeviceSource for LocalFolderSource {
    fn id(&self) -> String {
        self.id.clone()
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Folder
    }

    fn name(&self) -> String {
        // 展示名取末段目录名（无末段时回退完整路径）
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| strip_verbatim(&self.root))
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        let files = self.inner.list()?;
        match &self.exclude {
            None => Ok(files),
            Some(exclude) => Ok(files
                .into_iter()
                .filter(|f| !self.root.join(&f.rel_path).starts_with(exclude))
                .collect()),
        }
    }

    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        self.inner.open_head(id, max)
    }

    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        self.inner.stream(id)
    }

    fn delete(&self, id: &str) -> DeviceResult<()> {
        self.inner.delete(id)
    }
}

/// 从 canonical 路径剔除 `\\?\` / `\\?\UNC\` verbatim 前缀。
pub(crate) fn strip_verbatim(path: &Path) -> String {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        s.into_owned()
    }
}
