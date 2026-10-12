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
// wpd_backend，接入前整模块按骨架保留，避免未消费告警。
#[allow(dead_code)]
mod ic_transport;
