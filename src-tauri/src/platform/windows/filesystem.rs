use std::path::{Path, PathBuf};
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;

pub(crate) fn drive_roots() -> Vec<super::super::FilesystemRoot> {
    // GetLogicalDrives 一次读取位掩码，避免对断开的映射盘逐个 Path::exists。
    let mask = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
    (b'A'..=b'Z')
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
        .collect()
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

pub(crate) fn query_volume_serial(root: &Path) -> Option<u64> {
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetVolumeInformationW;
    let wide: Vec<u16> = root
        .to_str()?
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let mut serial: u32 = 0;
    let ok = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide.as_ptr()),
            None,
            None,
            Some(&mut serial),
            None,
            None,
        )
    };
    ok.is_ok().then(|| u64::from(serial))
}

pub(crate) fn configure_sequential_read(opts: &mut std::fs::OpenOptions) {
    use std::os::windows::fs::OpenOptionsExt;
    opts.custom_flags(0x08000000); // FILE_FLAG_SEQUENTIAL_SCAN
}

/// 设备枚举标识（盘符）转为文件系统卷根。
pub(crate) fn volume_root(id: &str) -> PathBuf {
    PathBuf::from(format!("{id}\\"))
}
