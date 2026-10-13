//! Device diagnostics share the application's Tauri log plugin and rotation policy.

pub fn record(message: impl AsRef<str>) {
    log::info!(target: "photographer::devices", "{}", message.as_ref());
}
