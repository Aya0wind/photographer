//! 未适配平台的显式回退，不执行原生系统操作。
use std::path::{Path, PathBuf};

pub(crate) fn drive_roots() -> std::io::Result<Vec<super::FilesystemRoot>> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台尚未实现文件系统根枚举",
    ))
}
pub(crate) fn is_hidden_or_system(_: &std::fs::DirEntry) -> bool {
    false
}
pub(crate) fn volume_label(_: &Path) -> Option<String> {
    None
}
pub(crate) fn filesystem_identity(_: &Path) -> std::io::Result<super::FilesystemIdentity> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "当前平台尚未实现文件系统识别",
    ))
}
pub(crate) fn configure_sequential_read(_: &mut std::fs::OpenOptions) {}
pub(crate) fn configure_background_command(_: &mut std::process::Command) {}
pub(crate) fn capabilities() -> super::PlatformCapabilities {
    super::PlatformCapabilities::default()
}
pub(crate) fn inference_plan(_: super::AccelerationPreference) -> super::InferencePlan {
    super::InferencePlan::cpu()
}
pub(crate) fn execution_providers(
    _: super::InferenceBackend,
) -> Vec<ort::ep::ExecutionProviderDispatch> {
    vec![ort::ep::CPU::default().build()]
}
pub(crate) fn font_candidates() -> Vec<PathBuf> {
    Vec::new()
}
pub(crate) fn sony_helper_candidates() -> Vec<PathBuf> {
    Vec::new()
}
pub(crate) fn open_with_system(_: super::ResourceRef<'_>) -> Result<(), String> {
    Err("当前平台尚未实现系统打开".into())
}
pub(crate) fn clipboard_copy_files(_: &[String]) -> Result<(), String> {
    Err("当前平台尚未实现文件剪贴板".into())
}
pub(crate) fn reveal_files(_: &[String]) -> Result<u32, String> {
    Err("当前平台尚未实现文件定位".into())
}
pub(crate) fn gphoto_library_name() -> &'static str {
    "libgphoto2-unavailable"
}
pub(crate) fn gphoto_port_library_name() -> &'static str {
    "libgphoto2-port-unavailable"
}
pub(crate) fn gphoto_driver_marker(_: bool) -> &'static str {
    "unavailable"
}
pub(crate) fn development_library_paths() -> Vec<PathBuf> {
    Vec::new()
}
pub(crate) fn locate_development_driver_dir(_: &Path, _: &str, _: &str) -> Option<PathBuf> {
    None
}
pub(crate) fn preload_dependencies(_: &Path) {}
pub(crate) fn crt_putenv(_: &str) {}
pub(crate) unsafe fn load_bundle_library(_: &Path) -> Result<libloading::Library, String> {
    Err("当前平台尚未适配相机动态库加载".into())
}

/// 占位平台不枚举卷；手工输入保留路径形态。
pub(crate) fn volume_root(id: &str) -> PathBuf {
    PathBuf::from(id)
}
