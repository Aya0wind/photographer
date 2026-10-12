//! macOS platform adapters.
mod filesystem;
mod libraries;
mod runtime;
mod system;

pub(crate) use filesystem::*;
pub(crate) use libraries::*;
pub(crate) use runtime::*;
pub(crate) use system::*;

// 联机拍摄（计划 §D）：ImageCaptureCore PTP 传输骨架（ic_transport.rs，
// 实现 tethering::ptp_control::PtpTransport）。尚未接入 CameraBackend /
// wpd_backend，且从未在 macOS 编译通过（2026-10-12 dev CI mac 构建失败
// 根因）——默认不编译；接 macOS 真机验证时开 macos-ic-transport feature
// 修骨架后再接线。
#[cfg(feature = "macos-ic-transport")]
#[allow(dead_code)]
mod ic_transport;
