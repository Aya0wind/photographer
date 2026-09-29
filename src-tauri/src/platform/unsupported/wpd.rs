//! 未适配平台的设备占位；枚举为空，设备操作返回不支持。
use crate::devices::{DeviceResult, DeviceSource, FileEntry};
use crate::events::SourceKind;

pub use crate::devices::wpd_helpers::*;

pub struct WpdSource {
    pnp_id: String,
    friendly_name: String,
}

impl WpdSource {
    pub fn new(pnp_id: impl Into<String>, friendly_name: impl Into<String>) -> Self {
        Self {
            pnp_id: pnp_id.into(),
            friendly_name: friendly_name.into(),
        }
    }
}

pub type StreamChunk = Result<Vec<u8>, std::io::Error>;

pub static WPD_RELEASE_COUNT: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

impl DeviceSource for WpdSource {
    fn id(&self) -> String {
        crate::devices::normalize_device_id(&self.pnp_id)
    }

    fn kind(&self) -> SourceKind {
        SourceKind::Mtp
    }

    fn name(&self) -> String {
        self.friendly_name.clone()
    }

    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        unavailable()
    }

    fn open_head(&self, _id: &str, _max: u64) -> DeviceResult<Vec<u8>> {
        unavailable()
    }

    fn stream(&self, _id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        unavailable()
    }

    fn delete(&self, _id: &str) -> DeviceResult<()> {
        unavailable()
    }
}

pub fn enumerate_mtp_devices() -> DeviceResult<Vec<(String, String)>> {
    Ok(Vec::new())
}

pub fn worker_ping(_pnp: &str) -> Option<bool> {
    Some(false)
}

pub fn invalidate_device(pnp: &str) {
    let _ = pnp;
}

#[doc(hidden)]
#[allow(dead_code)]
pub fn panic_probe() -> DeviceResult<()> {
    unavailable()
}

fn unavailable<T>() -> DeviceResult<T> {
    Err(crate::devices::DeviceError::NotSupported(
        "WPD 仅在 Windows 可用".into(),
    ))
}
