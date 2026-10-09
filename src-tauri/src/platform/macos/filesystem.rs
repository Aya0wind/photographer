use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(crate) fn drive_roots() -> std::io::Result<Vec<super::super::FilesystemRoot>> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "无法解析用户目录"))?;
    let mut roots = vec![super::super::FilesystemRoot {
        name: "用户目录".into(),
        path: home.to_string_lossy().into_owned(),
    }];
    let volumes = Path::new("/Volumes");
    if let Ok(entries) = std::fs::read_dir(volumes) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                let path = entry.path();
                roots.push(super::super::FilesystemRoot {
                    name: entry.file_name().to_string_lossy().into_owned(),
                    path: path.to_string_lossy().into_owned(),
                });
            }
        }
    }
    Ok(roots)
}

pub(crate) fn is_hidden_or_system(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

pub(crate) fn volume_label(root: &Path) -> Option<String> {
    root.file_name()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_string_lossy().into_owned())
}

pub(crate) fn filesystem_identity(
    path: &Path,
) -> std::io::Result<super::super::FilesystemIdentity> {
    let mut probe = path;
    let absolute;
    if !probe.is_absolute() {
        absolute = std::env::current_dir()?.join(probe);
        probe = &absolute;
    }
    loop {
        match std::fs::metadata(probe) {
            Ok(metadata) => return Ok(super::super::FilesystemIdentity(metadata.dev())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                probe = probe.parent().ok_or(error)?;
            }
            Err(error) => return Err(error),
        }
    }
}

pub(crate) fn configure_sequential_read(_: &mut std::fs::OpenOptions) {}

/// 资产登记指纹（§三 增量扫描两级识别）：APFS/HFS+ 无独立卷序列号 +
/// 文件 id 体系，dev（卷设备号）+ ino（inode 号）即等价物——硬链接共享
/// inode，rename 不变。
#[allow(dead_code)]
pub(crate) fn file_registration_id(
    path: &Path,
) -> std::io::Result<super::super::FileRegistrationId> {
    let metadata = std::fs::metadata(path)?;
    Ok(super::super::FileRegistrationId {
        volume_serial: metadata.dev(),
        file_id: format!("{:016x}", metadata.ino()),
    })
}

pub(crate) fn volume_root(id: &str) -> PathBuf {
    PathBuf::from(id)
}
