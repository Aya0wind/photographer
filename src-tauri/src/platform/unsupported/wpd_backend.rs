//! 未适配平台的相机占位；不声明可用能力。
use super::super::backend::{CameraBackend, CameraInfo, Capabilities, CapturedObject, TetherError};
use std::time::Duration;

#[derive(Default)]
pub struct WpdMtpBackend;
impl WpdMtpBackend {
    pub fn new() -> Self {
        Self
    }
}
impl CameraBackend for WpdMtpBackend {
    fn id(&self) -> &'static str {
        "wpd-mtp"
    }
    fn name(&self) -> &'static str {
        "WPD MTP（Windows 便携设备）"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::NONE
    }
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        Ok(Vec::new())
    }
    fn connect(&self, _: &str) -> Result<CameraInfo, TetherError> {
        Err(TetherError::Other("WPD 仅在 Windows 可用".into()))
    }
    fn disconnect(&self, _: &str) {}
    fn capture_still(&self, _: &str, _: Duration) -> Result<CapturedObject, TetherError> {
        Err(TetherError::Other("WPD 仅在 Windows 可用".into()))
    }
}
