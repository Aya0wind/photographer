//! 卷设备适配占位：不注册或枚举任何设备。
pub fn enumerate_empty_readers() -> Vec<String> {
    Vec::new()
}
pub fn probe_volume(_: &str) -> Option<String> {
    None
}
pub fn enumerate_present_volumes() -> Vec<(String, String)> {
    Vec::new()
}
