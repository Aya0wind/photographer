use std::ffi::{c_char, c_int, CString};
use std::path::{Path, PathBuf};
pub(crate) fn preload_dependencies(dir: &Path) {
    for dep in [
        "libwinpthread-1.dll",
        "libiconv-2.dll",
        "libtre-5.dll",
        "zlib1.dll",
        "libusb-1.0.dll",
        "libexif-12.dll",
        "libintl-8.dll",
        "libsystre-0.dll",
        "libjpeg-8.dll",
        "libxml2-16.dll",
        "libltdl-7.dll",
        "libgphoto2_port-12.dll",
    ] {
        let dep_path = dir.join(dep);
        if dep_path.is_file() {
            unsafe {
                if let Ok(h) = load_bundle_library(&dep_path) {
                    std::mem::forget(h);
                }
            }
        }
    }
}
pub(crate) unsafe fn load_bundle_library(path: &Path) -> Result<libloading::Library, String> {
    type RawLib = libloading::os::windows::Library;
    RawLib::load_with_flags(path, libloading::os::windows::LOAD_WITH_ALTERED_SEARCH_PATH)
        .map(|raw| raw.into())
        .map_err(|error| error.to_string())
}
pub(crate) fn crt_putenv(entry: &str) {
    let Ok(c) = CString::new(entry) else {
        return;
    };
    unsafe {
        type PutenvFn = unsafe extern "C" fn(*const c_char) -> c_int;
        let ucrt = match libloading::Library::new("ucrtbase.dll") {
            Ok(h) => h,
            Err(_) => return,
        };
        let putenv: libloading::Symbol<PutenvFn> = match ucrt.get(b"_putenv\0") {
            Ok(f) => f,
            Err(_) => return,
        };
        putenv(c.as_ptr());
        std::mem::forget(ucrt); // 进程内常驻（本就已在内存）
    }
}
pub(crate) fn gphoto_library_name() -> &'static str {
    "libgphoto2-6.dll"
}
pub(crate) fn gphoto_port_library_name() -> &'static str {
    "libgphoto2_port-12.dll"
}
pub(crate) fn gphoto_driver_marker(iolib: bool) -> &'static str {
    if iolib {
        "usb1.dll"
    } else {
        "ptp2.dll"
    }
}
pub(crate) fn development_library_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(root) = std::env::var_os("LOCALAPPDATA") {
        out.push(
            PathBuf::from(root)
                .join("Photographer")
                .join("gphoto")
                .join(gphoto_library_name()),
        );
    }
    out.push(PathBuf::from(r"C:\msys64\ucrt64\bin").join(gphoto_library_name()));
    out
}
pub(crate) fn locate_development_driver_dir(
    dll_dir: &Path,
    library_dir: &str,
    marker: &str,
) -> Option<PathBuf> {
    let lib_dir = dll_dir
        .parent()
        .map(|p| p.join("lib"))
        .unwrap_or_else(|| PathBuf::from(r"C:\msys64\ucrt64\lib"))
        .join(library_dir);
    let mut hits: Vec<PathBuf> = std::fs::read_dir(&lib_dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().join(marker).is_file())
        .map(|e| e.path())
        .collect();
    hits.pop()
}
