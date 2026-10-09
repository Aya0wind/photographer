use std::path::{Path, PathBuf};
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

pub(crate) fn drive_roots() -> std::io::Result<Vec<super::super::FilesystemRoot>> {
    // GetLogicalDrives 一次读取位掩码，避免对断开的映射盘逐个 Path::exists。
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    if mask == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((b'A'..=b'Z')
        .enumerate()
        .filter(|(index, _)| mask & (1 << index) != 0)
        .map(|(_, letter)| {
            let letter = letter as char;
            let root = format!("{letter}:\\");
            super::super::FilesystemRoot {
                name: format!("{letter}:"),
                path: root,
            }
        })
        .collect())
}

pub(crate) fn is_hidden_or_system(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    entry
        .metadata()
        .map(|meta| {
            let attrs = meta.file_attributes();
            attrs & FILE_ATTRIBUTE_HIDDEN != 0 || attrs & FILE_ATTRIBUTE_SYSTEM != 0
        })
        .unwrap_or(false)
}

pub(crate) fn volume_label(root: &Path) -> Option<String> {
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
    Some(String::from_utf16_lossy(&name[..len]))
}

pub(crate) fn filesystem_identity(
    path: &Path,
) -> std::io::Result<super::super::FilesystemIdentity> {
    let info = file_id_info(path)?;
    Ok(super::super::FilesystemIdentity(info.VolumeSerialNumber))
}

/// 资产登记指纹（§三 增量扫描两级识别）：卷序列号 + 卷内文件 id。
/// 同卷同 file id = 同一物理文件（硬链接两侧相同、rename 不变）；NTFS 的
/// 128-bit FILE_ID 落 16 字节小端，存小写十六进制串。exFAT/FAT 不支持
/// FileIdInfo 查询 → Err（调用方按「指纹不可用」走哈希路径）。
/// 生产消费点 = M2 登记管道（当前仅平台单测消费，故 allow）。
#[allow(dead_code)]
pub(crate) fn file_registration_id(
    path: &Path,
) -> std::io::Result<super::super::FileRegistrationId> {
    let info = file_id_info(path)?;
    let file_id = info
        .FileId
        .Identifier
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(super::super::FileRegistrationId {
        volume_serial: info.VolumeSerialNumber,
        file_id,
    })
}

/// 打开句柄取 FILE_ID_INFO（存在路径直达；未来目的地沿最近存在祖先上溯，
/// 与 filesystem_identity 同语义）。查询失败也关句柄。
fn file_id_info(path: &Path) -> std::io::Result<windows::Win32::Storage::FileSystem::FILE_ID_INFO> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{HRESULT, PCWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FileIdInfo, GetFileInformationByHandleEx, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_ID_INFO, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        OPEN_EXISTING,
    };
    let absolute;
    let mut probe = if path.is_absolute() {
        path
    } else {
        absolute = std::env::current_dir()?.join(path);
        &absolute
    };
    let handle = loop {
        let mut wide: Vec<u16> = probe.as_os_str().encode_wide().collect();
        if wide.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "路径含 NUL",
            ));
        }
        wide.push(0);
        // Follow junctions/symlinks; directory handles require BACKUP_SEMANTICS.
        match unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                FILE_READ_ATTRIBUTES.0,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS,
                None,
            )
        } {
            Ok(handle) => break handle,
            Err(error)
                if error.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0)
                    || error.code() == HRESULT::from_win32(ERROR_PATH_NOT_FOUND.0) =>
            {
                // Future destination paths may not exist; inspect the nearest existing ancestor.
                probe = probe
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::NotFound, error.to_string())
                    })?;
            }
            Err(error) => return Err(std::io::Error::other(error.to_string())),
        }
    };
    let mut info = FILE_ID_INFO::default();
    let result = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            (&mut info as *mut FILE_ID_INFO).cast(),
            std::mem::size_of::<FILE_ID_INFO>() as u32,
        )
    };
    // Close even if querying the filesystem failed. The 64-bit volume identity is not filename length.
    unsafe {
        let _ = CloseHandle(handle);
    }
    result.map_err(|error| std::io::Error::other(error.to_string()))?;
    Ok(info)
}

pub(crate) fn configure_sequential_read(opts: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    opts.custom_flags(0x08000000); // FILE_FLAG_SEQUENTIAL_SCAN
}

/// 设备枚举标识（盘符）转为文件系统卷根。
pub(crate) fn volume_root(id: &str) -> PathBuf {
    PathBuf::from(format!("{id}\\"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_files_and_future_destinations_have_same_identity() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("photo.jpg");
        std::fs::write(&src, b"photo").unwrap();
        let future = tmp.path().join("missing/sub/photo.jpg");
        assert_eq!(
            filesystem_identity(&src).unwrap(),
            filesystem_identity(&future).unwrap()
        );
        assert!(crate::platform::same_filesystem(&src, &future).unwrap());
        assert_eq!(
            filesystem_identity(std::path::Path::new(".")).unwrap(),
            filesystem_identity(&std::env::current_dir().unwrap()).unwrap()
        );
    }

    #[test]
    fn invalid_path_returns_error_instead_of_false_identity() {
        assert!(filesystem_identity(std::path::Path::new("path\0with-nul")).is_err());
        assert!(filesystem_identity(std::path::Path::new(r"\\?\NonexistentVolume\{:}\x")).is_err());
    }

    /// 登记指纹契约（§三 两级识别）：硬链接两侧同卷同 file id、rename 不变、
    /// 不同文件不同 id。经 `crate::platform::file_registration_id` 统一入口
    /// 调用（业务代码同款路径）。仅在支持 FileIdInfo 的卷（NTFS）上有
    /// 意义——临时目录在 FAT/exFAT 时查询报错，跳过断言而非判失败。
    #[test]
    fn registration_id_is_stable_across_hardlinks_and_rename() {
        let registration_id = crate::platform::file_registration_id;
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.jpg");
        std::fs::write(&a, b"photo").unwrap();
        let b = tmp.path().join("b.jpg");
        match std::fs::hard_link(&a, &b) {
            Ok(()) => {}
            Err(error) => {
                // 卷不支持硬链接（FAT 系）：本契约无从验证，跳过。
                eprintln!("跳过：临时卷不支持硬链接（{error}）");
                return;
            }
        }
        let id_a = match registration_id(&a) {
            Ok(id) => id,
            Err(error) => {
                eprintln!("跳过：临时卷不支持 FileIdInfo（{error}）");
                return;
            }
        };
        let id_b = registration_id(&b).unwrap();
        assert_eq!(id_a, id_b, "硬链接两侧指纹必须相同");

        let renamed = tmp.path().join("renamed.jpg");
        std::fs::rename(&a, &renamed).unwrap();
        assert_eq!(
            registration_id(&renamed).unwrap(),
            id_a,
            "rename 不改变 file id"
        );

        std::fs::write(tmp.path().join("other.jpg"), b"other").unwrap();
        let other = registration_id(&tmp.path().join("other.jpg")).unwrap();
        assert_ne!(other.file_id, id_a.file_id, "不同文件 file id 不同");
        assert_eq!(other.volume_serial, id_a.volume_serial, "同卷序列号一致");
    }
}
