use std::path::{Path, PathBuf};

pub(crate) fn gphoto_library_name() -> &'static str {
    "libgphoto2.6.dylib"
}

pub(crate) fn gphoto_port_library_name() -> &'static str {
    "libgphoto2_port.12.dylib"
}

pub(crate) fn gphoto_driver_marker(iolib: bool) -> &'static str {
    if iolib {
        "usb1.so"
    } else {
        "ptp2.so"
    }
}

pub(crate) fn development_library_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for prefix in ["/opt/homebrew", "/usr/local"] {
        paths.push(
            PathBuf::from(prefix)
                .join("opt/libgphoto2/lib")
                .join(gphoto_library_name()),
        );
        paths.push(
            PathBuf::from(prefix)
                .join("lib")
                .join(gphoto_library_name()),
        );
    }
    if let Some(home) = std::env::var_os("HOME") {
        paths.push(
            PathBuf::from(home)
                .join(".local")
                .join("lib")
                .join(gphoto_library_name()),
        );
    }
    paths
}

pub(crate) fn locate_development_driver_dir(
    library_path: &Path,
    library_dir: &str,
    marker: &str,
) -> Option<PathBuf> {
    // Caller passes the containing lib directory, not the dylib file.
    let root = library_path.join(library_dir);
    let mut hits = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            path.join(marker).is_file().then_some(path)
        })
        .collect::<Vec<_>>();
    hits.sort();
    hits.pop()
}

pub(crate) fn preload_dependencies(_: &Path) {}

pub(crate) fn crt_putenv(entry: &str) {
    if let Some((key, value)) = entry.split_once('=') {
        std::env::set_var(key, value);
    }
}

pub(crate) unsafe fn load_bundle_library(path: &Path) -> Result<libloading::Library, String> {
    libloading::Library::new(path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drivers_are_resolved_under_the_library_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = tmp.path().join("opt/libgphoto2/lib");
        let drivers = lib.join("libgphoto2/2.5.34");
        std::fs::create_dir_all(&drivers).unwrap();
        std::fs::write(drivers.join("ptp2.so"), b"fixture").unwrap();
        assert_eq!(
            locate_development_driver_dir(&lib, "libgphoto2", "ptp2.so"),
            Some(drivers)
        );
        assert!(locate_development_driver_dir(&lib, "libgphoto2_port", "usb1.so").is_none());
    }
}
