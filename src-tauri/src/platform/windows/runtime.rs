use std::path::PathBuf;

pub(crate) fn font_candidates() -> Vec<PathBuf> {
    ["msyh.ttc", "simhei.ttf", "simsun.ttc", "arial.ttf"]
        .into_iter()
        .map(|name| PathBuf::from(r"C:\Windows\Fonts").join(name))
        .collect()
}

pub(crate) fn configure_background_command(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}
pub(crate) fn capabilities() -> super::super::PlatformCapabilities {
    super::super::PlatformCapabilities {
        filesystem_roots: true,
        volume_devices: true,
        portable_devices: true,
        hotplug: true,
        system_open: true,
        file_clipboard: true,
        file_reveal: true,
        document_uris: false,
    }
}

pub(crate) fn inference_plan(
    preference: super::super::AccelerationPreference,
) -> super::super::InferencePlan {
    use super::super::{AccelerationPreference, InferenceBackend, InferencePlan};
    if matches!(
        preference,
        AccelerationPreference::Cpu | AccelerationPreference::CoreMl
    ) {
        InferencePlan::cpu()
    } else {
        InferencePlan {
            backend: InferenceBackend::DirectMl,
            optimization: ort::session::builder::GraphOptimizationLevel::Level1,
            prefer_batch: true,
        }
    }
}

pub(crate) fn execution_providers(
    backend: super::super::InferenceBackend,
) -> Vec<ort::ep::ExecutionProviderDispatch> {
    use super::super::InferenceBackend;
    match backend {
        InferenceBackend::DirectMl => vec![
            ort::ep::DirectML::default().build(),
            ort::ep::CPU::default().build(),
        ],
        _ => vec![ort::ep::CPU::default().build()],
    }
}
pub(crate) fn sony_helper_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("sony/photographer-sony.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(root).join("Photographer/sony-sdk/bridge-build/Release/photographer-sony.exe"),
        );
    }
    candidates
}
