//! 平台边界。业务代码只依赖这里的接口，OS 实现在对应目录维护。
//! macOS/Linux/Android 当前仅为扩展入口，沿用 unsupported 的安全回退。

#[derive(Debug, Clone)]
pub(crate) struct FilesystemRoot {
    pub name: String,
    pub path: String,
}

#[cfg(windows)]
#[path = "windows/mod.rs"]
mod native;
#[cfg(target_os = "macos")]
#[path = "macos/mod.rs"]
mod native;
#[cfg(target_os = "linux")]
#[path = "linux/mod.rs"]
mod native;
#[cfg(target_os = "android")]
#[path = "android/mod.rs"]
mod native;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "linux",
    target_os = "android"
)))]
#[path = "unsupported/mod.rs"]
mod native;

#[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
mod unsupported;

pub(crate) use native::{
    clipboard_copy_files, configure_background_command, configure_sequential_read, crt_putenv,
    development_library_paths, drive_roots, execution_providers, font_candidates,
    gphoto_driver_marker, gphoto_library_name, gphoto_port_library_name, is_hidden_or_system,
    load_bundle_library, locate_development_driver_dir, open_with_system, preload_dependencies,
    query_volume_serial, reveal_files, sony_helper_candidates, supports_directml, volume_label,
    volume_root,
};

#[cfg(all(test, windows))]
pub(crate) use native::{child_pidls, ShellApartment};

/// 保留 Windows 的目录分组规则；该规则仅供 Windows Shell 实现及原有测试使用。
pub(crate) fn group_by_parent(paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for path in paths {
        let parent = std::path::Path::new(path)
            .parent()
            .map(|p| p.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let slot = *index.entry(parent.clone()).or_insert_with(|| {
            groups.push((parent, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(path.clone());
    }
    groups
}
