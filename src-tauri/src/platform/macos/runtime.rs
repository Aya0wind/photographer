use std::path::PathBuf;

pub(crate) fn font_candidates() -> Vec<PathBuf> {
    let mut paths = vec![
        PathBuf::from("/System/Library/Fonts/PingFang.ttc"),
        PathBuf::from("/System/Library/Fonts/Hiragino Sans GB.ttc"),
        PathBuf::from("/Library/Fonts/Arial.ttf"),
        PathBuf::from("/System/Library/Fonts/Supplemental/Arial.ttf"),
    ];
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(PathBuf::from(home).join("Library/Fonts/NotoSansCJK-Regular.ttc"));
    }
    paths
}

pub(crate) fn configure_background_command(_: &mut std::process::Command) {}

pub(crate) fn capabilities() -> super::super::PlatformCapabilities {
    super::super::PlatformCapabilities {
        filesystem_roots: true,
        volume_devices: true,
        portable_devices: false,
        hotplug: false,
        system_open: true,
        file_clipboard: true,
        file_reveal: true,
        document_uris: false,
    }
}

pub(crate) fn inference_plan(
    preference: super::super::AccelerationPreference,
) -> super::super::InferencePlan {
    if matches!(
        preference,
        super::super::AccelerationPreference::Auto | super::super::AccelerationPreference::CoreMl
    ) {
        super::super::InferencePlan {
            backend: super::super::InferenceBackend::CoreMl,
            optimization: ort::session::builder::GraphOptimizationLevel::Level3,
            prefer_batch: false,
        }
    } else {
        super::super::InferencePlan::cpu()
    }
}

pub(crate) fn execution_providers(
    backend: super::super::InferenceBackend,
) -> Vec<ort::ep::ExecutionProviderDispatch> {
    match backend {
        super::super::InferenceBackend::CoreMl => {
            vec![
                ort::ep::CoreML::default().build().error_on_failure(),
                ort::ep::CPU::default().build(),
            ]
        }
        _ => vec![ort::ep::CPU::default().build()],
    }
}

pub(crate) fn sony_helper_candidates() -> Vec<PathBuf> {
    Vec::new()
}
