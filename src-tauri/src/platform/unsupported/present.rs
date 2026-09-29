//! 卷设备适配占位：不注册或枚举任何设备。
pub fn enumerate_empty_readers() -> crate::devices::DeviceResult<Vec<String>> {
    Err(crate::devices::DeviceError::NotSupported("卷枚举".into()))
}
pub fn probe_volume(_: &str) -> Option<String> {
    None
}
pub fn enumerate_present_volumes() -> crate::devices::DeviceResult<Vec<(String, String)>> {
    Err(crate::devices::DeviceError::NotSupported("卷枚举".into()))
}
