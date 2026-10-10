//! 平台边界。业务代码只依赖这里的接口，OS 实现在对应目录维护。
//! macOS 提供文件系统、系统集成、CoreML 和可移动卷轮询；Linux/Android
//! 仍沿用 unsupported 的安全回退。

#[derive(Debug, Clone)]
pub(crate) struct FilesystemRoot {
    pub name: String,
    pub path: String,
}

/// Availability is explicit; empty discovery results mean no connected devices.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformCapabilities {
    pub filesystem_roots: bool,
    pub volume_devices: bool,
    pub portable_devices: bool,
    pub hotplug: bool,
    pub system_open: bool,
    pub file_clipboard: bool,
    pub file_reveal: bool,
    pub document_uris: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AccelerationPreference {
    Auto,
    Cpu,
    DirectMl,
    CoreMl,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum InferenceBackend {
    Cpu,
    DirectMl,
    /// macOS CoreML EP（macOS 移植线预留；Windows 构建下不可构造）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    CoreMl,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct InferencePlan {
    pub backend: InferenceBackend,
    pub optimization: ort::session::builder::GraphOptimizationLevel,
    pub prefer_batch: bool,
}
impl InferencePlan {
    pub(crate) fn cpu() -> Self {
        Self {
            backend: InferenceBackend::Cpu,
            optimization: ort::session::builder::GraphOptimizationLevel::Level3,
            prefer_batch: false,
        }
    }
}

/// Process-local filesystem identity. Do not persist or cache across mount changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilesystemIdentity(pub(crate) u64);

/// 资产登记指纹（单数据库多照片库 §三 增量扫描两级识别）：卷序列号 +
/// 卷内文件 id——同卷同 id = 同一物理文件（硬链接），零哈希直跳去重。
/// Windows = FILE_ID_INFO（NTFS 128-bit FILE_ID 十六进制；exFAT/FAT 查询
/// 失败 → Err，调用方走哈希路径）；macOS = dev + inode。
/// 生产消费点 = M2 登记管道（当前仅平台单测消费，同 [`same_filesystem`] 先例）。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRegistrationId {
    pub(crate) volume_serial: u64,
    /// 卷内文件 id（小写十六进制串；NTFS ≤128-bit / macOS inode 64-bit）。
    pub(crate) file_id: String,
}

/// Borrowed reference: URI resources never pass through local filesystem validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResourceRef<'a> {
    LocalPath(&'a str),
    Uri(&'a str),
}
impl<'a> ResourceRef<'a> {
    pub(crate) fn parse(value: &'a str) -> Self {
        if value.split_once("://").is_some_and(|(scheme, _)| {
            scheme.len() > 1 // A single letter followed by colon is a Windows drive.
                && scheme.bytes().enumerate().all(|(index, c)| {
                    c.is_ascii_alphabetic()
                        || (index > 0 && (c.is_ascii_digit() || matches!(c, b'+' | b'-' | b'.')))
                })
        }) {
            Self::Uri(value)
        } else {
            Self::LocalPath(value)
        }
    }
    pub(crate) fn local_file(self) -> Result<&'a std::path::Path, String> {
        match self {
            Self::LocalPath(path) => Ok(std::path::Path::new(path)),
            Self::Uri(_) => Err("当前平台尚未实现文档 URI 访问".into()),
        }
    }
}

/// Cold preflight only; actual moves should try rename and fall back to verified copying.
#[allow(dead_code)]
pub(crate) fn same_filesystem(a: &std::path::Path, b: &std::path::Path) -> std::io::Result<bool> {
    Ok(filesystem_identity(a)? == filesystem_identity(b)?)
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

/// 资产登记指纹（§三 两级识别）：卷序列号 + 卷内文件 id——同卷同 id =
/// 同一物理文件（硬链接）。生产消费点 = M2 导入/增量扫描登记管道（经
/// `crate::platform::file_registration_id` 统一入口调用）；当前仅平台单测
/// 经本入口验证接线，故 allow（同 [`same_filesystem`] 先例）。
#[allow(unused_imports)]
pub(crate) use native::file_registration_id;

// lib 全量消费；测试经 #[path] 单独编译时消费面不全（同上先例）。
#[allow(unused_imports)]
pub(crate) use native::{
    capabilities, clipboard_copy_files, configure_background_command, configure_sequential_read,
    crt_putenv, development_library_paths, drive_roots, execution_providers, filesystem_identity,
    font_candidates, gphoto_driver_marker, gphoto_library_name, gphoto_port_library_name,
    inference_plan, is_hidden_or_system, load_bundle_library, locate_development_driver_dir,
    open_with_system, preload_dependencies, reveal_files, sony_helper_candidates, volume_label,
    volume_root,
};

#[cfg(all(test, windows))]
#[allow(unused_imports)] // 仅个别测试二进制消费
pub(crate) use native::{child_pidls, ShellApartment};

/// 保留 Windows 的目录分组规则；该规则仅供 Windows Shell 实现及原有测试使用。
pub(crate) fn group_by_parent(paths: &[String]) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut index = std::collections::HashMap::new();
    for path in paths {
        // Keep the grouping contract independent of the host running tests:
        // Windows paths may arrive while the caller is on macOS/Linux.
        let normalized = path.replace('\\', "/");
        let key = normalized
            .rsplit_once('/')
            .map(|(parent, _)| parent)
            .unwrap_or_default()
            .to_ascii_lowercase();
        let display_parent = if path.contains('\\') {
            path.rfind('\\')
                .or_else(|| path.rfind('/'))
                .map(|index| path[..index].to_ascii_lowercase())
                .unwrap_or_default()
        } else {
            std::path::Path::new(path)
                .parent()
                .map(|parent| parent.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default()
        };
        let slot = *index.entry(key).or_insert_with(|| {
            groups.push((display_parent, Vec::new()));
            groups.len() - 1
        });
        groups[slot].1.push(path.clone());
    }
    groups
}

#[cfg(test)]
mod contract_tests {
    use super::*;

    #[test]
    fn resource_kind_preserves_paths_and_separates_document_uris() {
        for path in [
            r"C:\photos\a.jpg",
            "C:/photos/a.jpg",
            "C://photos/a.jpg",
            "C:relative.jpg",
            "/photos/A.jpg",
            "relative.jpg",
            r"\\server\share\a.jpg",
        ] {
            let resource = ResourceRef::parse(path);
            assert_eq!(resource, ResourceRef::LocalPath(path));
            assert_eq!(resource.local_file().unwrap(), std::path::Path::new(path));
        }
        for uri in [
            "content://media/external/1",
            "file:///photos/a.jpg",
            "https://example.com/a.jpg",
        ] {
            let resource = ResourceRef::parse(uri);
            assert_eq!(resource, ResourceRef::Uri(uri));
            assert!(resource.local_file().is_err());
        }
    }

    #[test]
    fn cpu_preference_never_enables_acceleration_or_batching() {
        let plan = inference_plan(AccelerationPreference::Cpu);
        assert_eq!(plan.backend, InferenceBackend::Cpu);
        assert!(!plan.prefer_batch);
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn unavailable_capabilities_are_errors_without_native_threads() {
        assert!(!capabilities().portable_devices);
        assert!(!capabilities().volume_devices);
        assert!(!capabilities().hotplug);
        assert_eq!(
            drive_roots().unwrap_err().kind(),
            std::io::ErrorKind::Unsupported
        );
        assert_eq!(
            filesystem_identity(std::path::Path::new("."))
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::Unsupported
        );
        assert_eq!(
            inference_plan(AccelerationPreference::Auto).backend,
            InferenceBackend::Cpu
        );
        assert_eq!(
            inference_plan(AccelerationPreference::DirectMl).backend,
            InferenceBackend::Cpu
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_capabilities_expose_folder_filesystem_and_system_integration() {
        let caps = capabilities();
        assert!(caps.filesystem_roots);
        assert!(caps.volume_devices);
        assert!(!caps.portable_devices);
        assert!(!caps.hotplug);
        assert!(caps.system_open);
        assert!(caps.file_clipboard);
        assert!(caps.file_reveal);
        assert!(!caps.document_uris);
        assert!(!drive_roots().unwrap().is_empty());
        assert_eq!(
            inference_plan(AccelerationPreference::Auto).backend,
            InferenceBackend::CoreMl
        );
    }
}
