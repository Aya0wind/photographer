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
pub(crate) fn supports_directml() -> bool {
    true
}
pub(crate) fn execution_providers(use_gpu: bool) -> Vec<ort::ep::ExecutionProviderDispatch> {
    if use_gpu {
        vec![
            ort::ep::DirectML::default().build(),
            ort::ep::CPU::default().build(),
        ]
    } else {
        vec![ort::ep::CPU::default().build()]
    }
}
pub(crate) fn sony_helper_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("sony/photo-hub-sony.exe"));
        }
    }
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(root).join("PhotoHub/sony-sdk/bridge-build/Release/photo-hub-sony.exe"),
        );
    }
    candidates
}
