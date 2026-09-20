//! 本地文件夹源（M2，spec §5.11“从文件夹导入”）：把用户自选的任意目录
//! 统一为 `DeviceSource`，与卷/相机走同一条导入流水线。
//!
//! id = `FOLDER:<用户输入形态的绝对路径>`（absolutize 清 `.` 段与尾分隔符
//! 保证幂等，但**不 canonicalize**——映射盘 `Y:\照片` 不得被解析成
//! `\\192.168.31.103\...` UNC 形态；canonical 仅用于内部一致性比对）。
//! 枚举复用卷源规则（媒体扩展名过滤 + 跳过点前缀/系统目录）；可选
//! `exclude` 子树在枚举时整体跳过（自我嵌套守卫的防御层：目标收纳区
//! 落在源内时，源枚举不产出目标树内的文件）。

use std::fs;
use std::path::{Component, Path, PathBuf};

use super::volume::VolumeSource;
use super::{DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

/// 文件夹源 id 前缀。
pub const FOLDER_ID_PREFIX: &str = "FOLDER:";

/// 本地文件夹源。
pub struct LocalFolderSource {
    /// 委托卷源实现（walkdir 枚举/过滤/读取完全同规则）。
    inner: VolumeSource,
    /// 注册/展示根：用户输入形态（绝对化但不解析链接；映射盘符保留）。
    display_root: PathBuf,
    id: String,
    /// 枚举时排除的子树（相对源根的 `/` 分隔路径；None = 不排除）。
    exclude_rel: Option<String>,
}

impl LocalFolderSource {
    /// 打开文件夹源；目录不存在/不可访问 → `DeviceError::Io`。
    pub fn new(root: impl AsRef<Path>) -> DeviceResult<Self> {
        Self::with_exclude(root, None)
    }

    /// 同 `new`，但枚举时排除 `exclude` 子树（exclude 不在源内则忽略）。
    /// canonicalize 仅在此处用于“exclude 是否落在源内”的归属判定
    /// （两侧同为 canonical，规避映射盘符/UNC 形态不一致），**不进入 id**。
    pub fn with_exclude(root: impl AsRef<Path>, exclude: Option<&Path>) -> DeviceResult<Self> {
        let display_root = absolutize(root.as_ref());
        // 存在性/可访问性检查（目录缺失 → Io）；结果只用于内部比对
        let canonical_root = fs::canonicalize(&display_root)?;
        let exclude_rel = exclude
            .and_then(|p| fs::canonicalize(p).ok())
            .and_then(|c| {
                c.strip_prefix(&canonical_root)
                    .ok()
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            });
        Ok(Self {
            id: format!("{FOLDER_ID_PREFIX}{}", display_root.to_string_lossy()),
            inner: VolumeSource::new(&display_root),
            display_root,
            exclude_rel,
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
        self.display_root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.display_root.to_string_lossy().into_owned())
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        let files = self.inner.list()?;
        match &self.exclude_rel {
            None => Ok(files),
            // 相对路径比对：不做逐文件 canonicalize（万级文件 × NAS = 网络
            // 往返灾难）；rel_path 与 exclude_rel 均为 `/` 分隔
            Some(exclude) => Ok(files
                .into_iter()
                .filter(|f| !Path::new(&f.rel_path).starts_with(Path::new(exclude)))
                .collect()),
        }
    }

    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        self.inner.open_head(id, max)
    }

    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        self.inner.stream(id)
    }

    fn local_path(&self, id: &str) -> Option<std::path::PathBuf> {
        self.inner.local_path(id)
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

/// 绝对化但**不解析**：相对路径拼当前目录；重组 components 清除 `.` 段
/// 与尾部分隔符（幂等）；保留盘符/UNC/符号链接的用户输入形态
/// （canonicalize 会把映射盘 `Y:\` 解析成 `\\?\UNC\server\share\`——
/// id 与前端展示都必须保留盘符形态，canonical 仅用于内部一致性比对）。
fn absolutize(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(cwd) => cwd.join(path),
            Err(_) => path.to_path_buf(),
        }
    };
    let mut out = PathBuf::new();
    for component in absolute.components() {
        if let Component::CurDir = component {
            continue;
        }
        out.push(component.as_os_str());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// verbatim 前缀剥离三形态（映射盘 canonical 会产出 `\\?\UNC\...`，
    /// 剥成残缺 `UNC\...` 是 2026-09-18 线上 bug 的根因之一）。
    #[test]
    fn strip_verbatim_local_drive() {
        assert_eq!(strip_verbatim(Path::new(r"\\?\C:\a\b")), r"C:\a\b");
    }

    #[test]
    fn strip_verbatim_unc_share() {
        assert_eq!(
            strip_verbatim(Path::new(r"\\?\UNC\192.168.31.103\Photos (存储空间2)")),
            r"\\192.168.31.103\Photos (存储空间2)"
        );
    }

    #[test]
    fn strip_verbatim_plain_paths_untouched() {
        assert_eq!(strip_verbatim(Path::new(r"C:\a\b")), r"C:\a\b");
        assert_eq!(
            strip_verbatim(Path::new(r"\\srv\share\a")),
            r"\\srv\share\a"
        );
    }

    /// id 必须由用户输入形态构成（保留映射盘符 Y:），不做 canonical 解析：
    /// canonicalize 会把 `Y:\照片` 变成 `\\?\UNC\192.168.31.103\...\照片`。
    /// 本地用 verbatim 输入复现“形态转换”：旧实现 id 被 canonical 成 C:\...。
    #[test]
    fn folder_id_keeps_user_input_form_not_canonical() {
        let tmp = tempfile::tempdir().unwrap();
        let verbatim = format!(r"\\?\{}", tmp.path().display());
        let src = LocalFolderSource::new(&verbatim).unwrap();
        assert_eq!(src.id(), format!("FOLDER:{verbatim}"));
        // 同一目录的普通形态 → 不同 id（按输入形态注册）；变体写法幂等
        let plain = LocalFolderSource::new(tmp.path().join(".")).unwrap();
        assert_eq!(plain.id(), format!("FOLDER:{}", tmp.path().display()));
    }

    #[test]
    fn absolutize_dots_and_trailing_separators() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        // 尾部 `.` 段与尾分隔符清除（幂等 id）
        assert_eq!(absolutize(&root.join(".")), root);
        let with_slash = PathBuf::from(format!(r"{}\", root.display()));
        assert_eq!(absolutize(&with_slash), root);
        // 已是规范形态原样保留（含前导盘符与根）
        assert_eq!(absolutize(&root), root);
    }
}
