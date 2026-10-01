//! libgphoto2 联拍后端（进程内，2026-09-29 定案）：运行时动态加载
//! 平台对应的 libgphoto2 动态库（libloading 取符号表），直调 C API——零 C++ 桥、零
//! 子进程、零构建系统集成（不需要 pkg-config / import lib，绕开 MinGW↔MSVC
//! 工具链墙；gphoto2-rs crate 卡在 pkg-config + MSVC import 库，故自持薄 FFI）。
//!
//! 库解析顺序：`PHOTO_HUB_GPHOTO_DLL`（绝对路径）→ 应用资源 `gphoto/` →
//! 平台开发机路径。Windows 的 DLL 依赖会先预加载；macOS 组装脚本把
//! 依赖重写为 `@loader_path` 并保留 camlibs/iolibs。
//!
//! libgphoto2 的 `Camera*` 非线程安全：本后端所有相机操作过同一把
//! `camera_mutex` 串行（会话 poller 线程与用户操作并发）。锁序恒为
//! camera_mutex → connections（包括 disconnect，查连接和释放句柄不能交错）。
//! 真机基线（A7R V / ILCE-7RM5 PC Control，2026-09-29）：枚举/参数读写/
//! 取景帧/拍摄+下载全通；光圈位在镜头睡眠时只读（writable 随刷新恢复）。

use std::collections::HashMap;
use std::ffi::{c_char, c_int, c_ulong, CStr, CString};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;

use super::backend::{
    CameraBackend, CameraInfo, CameraSetting, CameraSettingOption, Capabilities, CapturedObject,
    SettingKind, TetherError,
};

/// FFI 类型与符号（仅本后端用到的 C API 子集；取值对照 gphoto2 2.5 头文件）。
// 类型名沿用 C 符号原名（字段名=符号名的加载宏依赖此命名）。
#[allow(non_camel_case_types)]
mod gpt {
    use std::ffi::{c_char, c_int, c_ulong};

    // CameraFileType：PREVIEW=0, NORMAL=1, RAW=2, …（gphoto2-file.h）
    pub const GP_FILE_TYPE_NORMAL: c_int = 1;
    // CameraCaptureType：IMAGE=0, MOVIE=1, SOUND=2（gphoto2-camera.h）
    pub const GP_CAPTURE_IMAGE: c_int = 0;
    // CameraEventType：UNKNOWN=0, TIMEOUT=1, FILE_ADDED=2, …（gphoto2-camera.h）
    pub const GP_EVENT_FILE_ADDED: c_int = 2;
    pub const GP_EVENT_UNKNOWN: c_int = 0;

    /// 不透明句柄（C 侧为指向内部结构的指针）。
    macro_rules! opaque {
        ($($name:ident),*) => {
            $(
                #[repr(C)]
                pub struct $name(#[allow(dead_code)] pub *mut std::ffi::c_void);
            )*
        };
    }
    opaque!(
        GPPortInfo,
        GPPortInfoList,
        CameraList,
        Camera,
        CameraFile,
        CameraWidget
    );

    #[repr(C)]
    pub struct CameraFilePath {
        pub name: [c_char; 128],
        pub folder: [c_char; 1024],
    }

    // 函数指针类型：字段名即 libgphoto2 符号名，供 libloading 按名解析。
    pub type gp_context_new = unsafe extern "C" fn() -> *mut std::ffi::c_void;
    pub type gp_context_unref = unsafe extern "C" fn(*mut std::ffi::c_void);
    pub type gp_context_set_error_func = unsafe extern "C" fn(
        *mut std::ffi::c_void,
        unsafe extern "C" fn(*mut std::ffi::c_void, *const c_char, *mut std::ffi::c_void),
        *mut std::ffi::c_void,
    );
    pub type gp_list_new = unsafe extern "C" fn(*mut *mut CameraList) -> c_int;
    pub type gp_list_free = unsafe extern "C" fn(*mut CameraList) -> c_int;
    pub type gp_list_count = unsafe extern "C" fn(*const CameraList) -> c_int;
    pub type gp_list_get_name =
        unsafe extern "C" fn(*const CameraList, c_int, *mut *const c_char) -> c_int;
    pub type gp_list_get_value =
        unsafe extern "C" fn(*const CameraList, c_int, *mut *const c_char) -> c_int;
    pub type gp_camera_autodetect =
        unsafe extern "C" fn(*mut CameraList, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_new = unsafe extern "C" fn(*mut *mut Camera) -> c_int;
    pub type gp_camera_free = unsafe extern "C" fn(*mut Camera) -> c_int;
    pub type gp_camera_init = unsafe extern "C" fn(*mut Camera, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_exit = unsafe extern "C" fn(*mut Camera, *mut std::ffi::c_void) -> c_int;
    /// abilities 出参按 `char*` 缓冲传（CameraAbilities.model 是首成员，
    /// 只读机型名，不依赖完整结构布局）。
    pub type gp_camera_get_abilities =
        unsafe extern "C" fn(*mut Camera, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_get_config =
        unsafe extern "C" fn(*mut Camera, *mut *mut CameraWidget, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_set_config =
        unsafe extern "C" fn(*mut Camera, *mut CameraWidget, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_capture_preview =
        unsafe extern "C" fn(*mut Camera, *mut CameraFile, *mut std::ffi::c_void) -> c_int;
    pub type gp_camera_capture = unsafe extern "C" fn(
        *mut Camera,
        c_int,
        *mut CameraFilePath,
        *mut std::ffi::c_void,
    ) -> c_int;
    pub type gp_camera_file_get = unsafe extern "C" fn(
        *mut Camera,
        *const c_char,
        *const c_char,
        c_int,
        *mut CameraFile,
        *mut std::ffi::c_void,
    ) -> c_int;
    pub type gp_camera_wait_for_event = unsafe extern "C" fn(
        *mut Camera,
        c_int,
        *mut c_int,
        *mut *mut std::ffi::c_void,
        *mut std::ffi::c_void,
    ) -> c_int;
    pub type gp_port_info_list_new = unsafe extern "C" fn(*mut *mut GPPortInfoList) -> c_int;
    pub type gp_port_info_list_free = unsafe extern "C" fn(*mut GPPortInfoList) -> c_int;
    pub type gp_port_info_list_load = unsafe extern "C" fn(*mut GPPortInfoList) -> c_int;
    pub type gp_port_info_list_lookup_path =
        unsafe extern "C" fn(*const GPPortInfoList, *const c_char) -> c_int;
    pub type gp_port_info_list_get_info =
        unsafe extern "C" fn(*const GPPortInfoList, c_int, *mut GPPortInfo) -> c_int;
    pub type gp_camera_set_port_info = unsafe extern "C" fn(*mut Camera, GPPortInfo) -> c_int;
    pub type gp_file_new = unsafe extern "C" fn(*mut *mut CameraFile) -> c_int;
    pub type gp_file_free = unsafe extern "C" fn(*mut CameraFile) -> c_int;
    pub type gp_file_get_data_and_size =
        unsafe extern "C" fn(*mut CameraFile, *mut *const c_char, *mut c_ulong) -> c_int;
    pub type gp_widget_free = unsafe extern "C" fn(*mut CameraWidget) -> c_int;
    pub type gp_widget_get_child_by_name =
        unsafe extern "C" fn(*mut CameraWidget, *const c_char, *mut *mut CameraWidget) -> c_int;
    pub type gp_widget_get_child =
        unsafe extern "C" fn(*mut CameraWidget, c_int, *mut *mut CameraWidget) -> c_int;
    pub type gp_widget_count_children = unsafe extern "C" fn(*mut CameraWidget) -> c_int;
    pub type gp_widget_get_type = unsafe extern "C" fn(*mut CameraWidget, *mut c_int) -> c_int;
    pub type gp_widget_get_label =
        unsafe extern "C" fn(*mut CameraWidget, *mut *const c_char) -> c_int;
    pub type gp_widget_get_name =
        unsafe extern "C" fn(*mut CameraWidget, *mut *const c_char) -> c_int;
    pub type gp_widget_get_value =
        unsafe extern "C" fn(*mut CameraWidget, *mut std::ffi::c_void) -> c_int;
    pub type gp_widget_set_value =
        unsafe extern "C" fn(*mut CameraWidget, *const std::ffi::c_void) -> c_int;
    pub type gp_widget_get_readonly = unsafe extern "C" fn(*mut CameraWidget, *mut c_int) -> c_int;
    pub type gp_widget_get_range =
        unsafe extern "C" fn(*mut CameraWidget, *mut f32, *mut f32, *mut f32) -> c_int;
    pub type gp_widget_count_choices = unsafe extern "C" fn(*mut CameraWidget) -> c_int;
    pub type gp_widget_get_choice =
        unsafe extern "C" fn(*mut CameraWidget, c_int, *mut *const c_char) -> c_int;
}

/// 运行时解析的符号表（字段名 = 符号名；任一缺失视为库不可用）。
#[allow(non_snake_case)]
struct Symbols {
    gp_context_new: gpt::gp_context_new,
    gp_context_unref: gpt::gp_context_unref,
    gp_context_set_error_func: gpt::gp_context_set_error_func,
    gp_list_new: gpt::gp_list_new,
    gp_list_free: gpt::gp_list_free,
    gp_list_count: gpt::gp_list_count,
    gp_list_get_name: gpt::gp_list_get_name,
    gp_list_get_value: gpt::gp_list_get_value,
    gp_camera_autodetect: gpt::gp_camera_autodetect,
    gp_camera_new: gpt::gp_camera_new,
    gp_camera_free: gpt::gp_camera_free,
    gp_camera_init: gpt::gp_camera_init,
    gp_camera_exit: gpt::gp_camera_exit,
    gp_camera_get_abilities: gpt::gp_camera_get_abilities,
    gp_camera_get_config: gpt::gp_camera_get_config,
    gp_camera_set_config: gpt::gp_camera_set_config,
    gp_camera_capture_preview: gpt::gp_camera_capture_preview,
    gp_camera_capture: gpt::gp_camera_capture,
    gp_camera_file_get: gpt::gp_camera_file_get,
    gp_camera_wait_for_event: gpt::gp_camera_wait_for_event,
    gp_port_info_list_new: gpt::gp_port_info_list_new,
    gp_port_info_list_free: gpt::gp_port_info_list_free,
    gp_port_info_list_load: gpt::gp_port_info_list_load,
    gp_port_info_list_lookup_path: gpt::gp_port_info_list_lookup_path,
    gp_port_info_list_get_info: gpt::gp_port_info_list_get_info,
    gp_camera_set_port_info: gpt::gp_camera_set_port_info,
    gp_file_new: gpt::gp_file_new,
    gp_file_free: gpt::gp_file_free,
    gp_file_get_data_and_size: gpt::gp_file_get_data_and_size,
    gp_widget_free: gpt::gp_widget_free,
    gp_widget_get_child_by_name: gpt::gp_widget_get_child_by_name,
    gp_widget_get_child: gpt::gp_widget_get_child,
    gp_widget_count_children: gpt::gp_widget_count_children,
    gp_widget_get_type: gpt::gp_widget_get_type,
    gp_widget_get_label: gpt::gp_widget_get_label,
    gp_widget_get_name: gpt::gp_widget_get_name,
    gp_widget_get_value: gpt::gp_widget_get_value,
    gp_widget_set_value: gpt::gp_widget_set_value,
    gp_widget_get_readonly: gpt::gp_widget_get_readonly,
    gp_widget_get_range: gpt::gp_widget_get_range,
    gp_widget_count_choices: gpt::gp_widget_count_choices,
    gp_widget_get_choice: gpt::gp_widget_get_choice,
}

impl Symbols {
    /// 逐符号在 libgphoto2 与 libgphoto2_port 两库间解析（port API
    /// ——gp_context/gp_port_info_list*——只由 port 库导出；任一符号两处
    /// 皆无则视为库不可用）。
    unsafe fn load(lib: &libloading::Library, port: &libloading::Library) -> Result<Self, String> {
        macro_rules! sym {
            ($name:ident) => {{
                let n = concat!(stringify!($name), "\0").as_bytes();
                match lib.get::<gpt::$name>(n) {
                    Ok(v) => *v,
                    Err(_) => *port
                        .get::<gpt::$name>(n)
                        .map_err(|e| format!("{}: {e}", stringify!($name)))?,
                }
            }};
        }
        Ok(Symbols {
            gp_context_new: sym!(gp_context_new),
            gp_context_unref: sym!(gp_context_unref),
            gp_context_set_error_func: sym!(gp_context_set_error_func),
            gp_list_new: sym!(gp_list_new),
            gp_list_free: sym!(gp_list_free),
            gp_list_count: sym!(gp_list_count),
            gp_list_get_name: sym!(gp_list_get_name),
            gp_list_get_value: sym!(gp_list_get_value),
            gp_camera_autodetect: sym!(gp_camera_autodetect),
            gp_camera_new: sym!(gp_camera_new),
            gp_camera_free: sym!(gp_camera_free),
            gp_camera_init: sym!(gp_camera_init),
            gp_camera_exit: sym!(gp_camera_exit),
            gp_camera_get_abilities: sym!(gp_camera_get_abilities),
            gp_camera_get_config: sym!(gp_camera_get_config),
            gp_camera_set_config: sym!(gp_camera_set_config),
            gp_camera_capture_preview: sym!(gp_camera_capture_preview),
            gp_camera_capture: sym!(gp_camera_capture),
            gp_camera_file_get: sym!(gp_camera_file_get),
            gp_camera_wait_for_event: sym!(gp_camera_wait_for_event),
            gp_port_info_list_new: sym!(gp_port_info_list_new),
            gp_port_info_list_free: sym!(gp_port_info_list_free),
            gp_port_info_list_load: sym!(gp_port_info_list_load),
            gp_port_info_list_lookup_path: sym!(gp_port_info_list_lookup_path),
            gp_port_info_list_get_info: sym!(gp_port_info_list_get_info),
            gp_camera_set_port_info: sym!(gp_camera_set_port_info),
            gp_file_new: sym!(gp_file_new),
            gp_file_free: sym!(gp_file_free),
            gp_file_get_data_and_size: sym!(gp_file_get_data_and_size),
            gp_widget_free: sym!(gp_widget_free),
            gp_widget_get_child_by_name: sym!(gp_widget_get_child_by_name),
            gp_widget_get_child: sym!(gp_widget_get_child),
            gp_widget_count_children: sym!(gp_widget_count_children),
            gp_widget_get_type: sym!(gp_widget_get_type),
            gp_widget_get_label: sym!(gp_widget_get_label),
            gp_widget_get_name: sym!(gp_widget_get_name),
            gp_widget_get_value: sym!(gp_widget_get_value),
            gp_widget_set_value: sym!(gp_widget_set_value),
            gp_widget_get_readonly: sym!(gp_widget_get_readonly),
            gp_widget_get_range: sym!(gp_widget_get_range),
            gp_widget_count_choices: sym!(gp_widget_count_choices),
            gp_widget_get_choice: sym!(gp_widget_get_choice),
        })
    }
}

struct GphotoLib {
    #[allow(dead_code)]
    handle: libloading::Library,
    /// port 库独立句柄：libgphoto2 依赖它但符号解析要直接 GetProcAddress。
    #[allow(dead_code)]
    port_handle: libloading::Library,
    symbols: Symbols,
}

static LIB: OnceLock<Result<GphotoLib, String>> = OnceLock::new();

fn lib() -> Result<&'static GphotoLib, TetherError> {
    LIB.get_or_init(|| {
        let mut errors: Vec<String> = Vec::new();
        for path in dll_candidates() {
            if !path.is_file() {
                errors.push(format!("{} 不存在", path.display()));
                continue;
            }
            if let Some(dir) = path.parent() {
                if let Err(error) = driver_environment(dir) {
                    errors.push(format!("{}: {error}", path.display()));
                    continue;
                }
                crate::platform::preload_dependencies(dir);
            }
            unsafe {
                match crate::platform::load_bundle_library(&path) {
                    Ok(handle) => {
                        let port_path = path
                            .parent()
                            .map(|d| d.join(crate::platform::gphoto_port_library_name()))
                            .ok_or_else(|| "路径无父目录".to_string());
                        let loaded = match port_path {
                            Ok(p) if p.is_file() => {
                                crate::platform::load_bundle_library(&p).map_err(|e| e.to_string())
                            }
                            Ok(p) => Err(format!(
                                "{} 缺少配套 {}（找 {}）",
                                path.display(),
                                crate::platform::gphoto_port_library_name(),
                                p.display()
                            )),
                            Err(e) => Err(e),
                        }
                        .and_then(|port| {
                            Symbols::load(&handle, &port).map(|symbols| (handle, port, symbols))
                        });
                        match loaded {
                            Ok((handle, port_handle, symbols)) => {
                                if let Some(dir) = path.parent() {
                                    set_driver_env(dir);
                                }
                                crate::devices::diagnostics::record(format!(
                                    "tethering libgphoto2 loaded={} CAMLIBS={} IOLIBS={}",
                                    path.display(),
                                    std::env::var("CAMLIBS").unwrap_or_default(),
                                    std::env::var("IOLIBS").unwrap_or_default()
                                ));
                                return Ok(GphotoLib {
                                    handle,
                                    port_handle,
                                    symbols,
                                });
                            }
                            Err(e) => errors.push(format!("{} 符号解析失败：{e}", path.display())),
                        }
                    }
                    Err(e) => errors.push(format!("{}: {e}", path.display())),
                }
            }
        }
        Err(format!("libgphoto2 不可用（{}）", errors.join("; ")))
    })
    .as_ref()
    .map_err(|e| TetherError::Other(e.clone()))
}

fn dll_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("PHOTO_HUB_GPHOTO_DLL") {
        out.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("gphoto").join(bundle_dll_name()));
        }
    }
    out.extend(crate::platform::development_library_paths());
    out
}

/// 随包主库文件名，由平台适配定义。
pub fn bundle_dll_name() -> &'static str {
    crate::platform::gphoto_library_name()
}

fn locate_driver_dir(dll_dir: &Path, kind: DriverKind) -> Option<PathBuf> {
    let (bundle, library_dir) = match kind {
        DriverKind::Iolib => ("iolibs", "libgphoto2_port"),
        DriverKind::Camlib => ("camlibs", "libgphoto2"),
    };
    let marker = crate::platform::gphoto_driver_marker(matches!(kind, DriverKind::Iolib));
    if dll_dir.join(bundle).join(marker).is_file() {
        return Some(dll_dir.join(bundle));
    }
    crate::platform::locate_development_driver_dir(dll_dir, library_dir, marker)
}

enum DriverKind {
    Iolib,
    Camlib,
}

fn driver_environment(dll_dir: &Path) -> Result<Vec<(&'static str, String)>, String> {
    [
        ("IOLIBS", DriverKind::Iolib),
        ("CAMLIBS", DriverKind::Camlib),
    ]
    .into_iter()
    .map(|(key, kind)| {
        let dir = std::env::var_os(key)
            .map(PathBuf::from)
            .or_else(|| locate_driver_dir(dll_dir, kind))
            .ok_or_else(|| format!("缺少 {key} 协议模块，请安装完整的 libgphoto2 运行库"))?;
        if !dir.is_dir() {
            return Err(format!("{key} 模块目录不存在：{}", dir.display()));
        }
        Ok((key, driver_path_string(&dir)))
    })
    .collect()
}

fn driver_path_string(dir: &Path) -> String {
    let path = dir.to_string_lossy().into_owned();
    // Tauri 的资源路径可带 Win32 扩展前缀，MinGW 的 ltdl 目录扫描不接受它。
    #[cfg(windows)]
    {
        if let Some(unc) = path.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(local) = path.strip_prefix(r"\\?\") {
            return local.to_string();
        }
    }
    path
}

/// 设置 IOLIBS/CAMLIBS（仅在用户未设置时；进程内首次加载时调用一次）。
fn set_driver_env(dll_dir: &Path) {
    if let Ok(environment) = driver_environment(dll_dir) {
        for (env_key, value) in environment {
            std::env::set_var(env_key, &value);
            // Rust 的进程环境与 DLL 的 UCRT getenv 需要同步，用户预设路径也一样。
            crate::platform::crt_putenv(&format!("{env_key}={value}"));
        }
    }
}

#[cfg(test)]
mod driver_tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires an assembled arm64 runtime and PHOTO_HUB_GPHOTO_DLL; run alone"]
    fn packaged_runtime_loads_rust_ffi_without_opening_camera() {
        let path = std::env::var_os("PHOTO_HUB_GPHOTO_DLL")
            .map(PathBuf::from)
            .expect("set PHOTO_HUB_GPHOTO_DLL to the assembled library");
        assert!(path.is_absolute() && path.is_file());
        let loaded = lib().expect("all Rust FFI symbols must resolve");
        // Allocate/free only a context, never enumerate or open USB cameras.
        unsafe {
            let context = context_new(&loaded.symbols);
            assert!(!context.is_null());
            (loaded.symbols.gp_context_unref)(context);
        }
        for (key, directory) in [("CAMLIBS", "camlibs"), ("IOLIBS", "iolibs")] {
            assert_eq!(
                PathBuf::from(std::env::var_os(key).expect("driver environment")),
                path.parent().unwrap().join(directory)
            );
        }
    }

    #[cfg(windows)]
    #[test]
    fn module_paths_remove_extended_windows_prefix() {
        assert_eq!(
            driver_path_string(Path::new(r"\\?\I:\Photo Hub\gphoto\camlibs")),
            r"I:\Photo Hub\gphoto\camlibs"
        );
        assert_eq!(
            driver_path_string(Path::new(r"\\?\UNC\server\share\camlibs")),
            r"\\server\share\camlibs"
        );
        assert_eq!(
            driver_path_string(Path::new(r"I:\Photo Hub\gphoto\camlibs")),
            r"I:\Photo Hub\gphoto\camlibs"
        );
    }

    #[test]
    fn partial_bundle_does_not_resolve_camera_drivers() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("gphoto");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(bundle_dll_name()), []).unwrap();
        assert!(locate_driver_dir(&dir, DriverKind::Camlib).is_none());
        assert!(locate_driver_dir(&dir, DriverKind::Iolib).is_none());
    }

    #[test]
    fn complete_bundle_resolves_its_own_modules() {
        let root = tempfile::tempdir().unwrap();
        for (folder, iolib, kind) in [
            ("camlibs", false, DriverKind::Camlib),
            ("iolibs", true, DriverKind::Iolib),
        ] {
            let dir = root.path().join(folder);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join(crate::platform::gphoto_driver_marker(iolib)), []).unwrap();
            assert_eq!(locate_driver_dir(root.path(), kind), Some(dir));
        }
    }
}

unsafe fn cstr(raw: *const c_char) -> String {
    if raw.is_null() {
        String::new()
    } else {
        CStr::from_ptr(raw).to_string_lossy().into_owned()
    }
}

unsafe extern "C" fn context_error(
    _context: *mut std::ffi::c_void,
    text: *const c_char,
    _data: *mut std::ffi::c_void,
) {
    crate::devices::diagnostics::record(format!("tethering gphoto2 error: {}", cstr(text)));
}

unsafe fn context_new(s: &Symbols) -> *mut std::ffi::c_void {
    let context = (s.gp_context_new)();
    (s.gp_context_set_error_func)(context, context_error, std::ptr::null_mut());
    context
}

fn check(code: c_int) -> Result<c_int, TetherError> {
    if code < 0 {
        return Err(TetherError::Other(format!("gphoto2 错误 {code}")));
    }
    Ok(code)
}

/// 单台相机的连接态。
struct GphotoConnection {
    camera: *mut gpt::Camera,
    /// 收片暂存目录（下载落盘 + open_captured 白名单根）。
    incoming: PathBuf,
    /// 确认设备消失后置否；短暂 busy/timeout 不等于断开。
    connected: AtomicBool,
    settings_revision: AtomicU64,
}
unsafe impl Send for GphotoConnection {}
unsafe impl Sync for GphotoConnection {}

struct GphotoEventData(*mut std::ffi::c_void);
impl Drop for GphotoEventData {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                extern "C" {
                    fn free(data: *mut std::ffi::c_void);
                }
                free(self.0); // libgphoto2 与 MSVC 使用同一 UCRT 堆。
            }
        }
    }
}

/// 联拍窗口契约三参数 → 前端置顶排序键（其余按 label 排序）。
const CORE_SETTING_IDS: &[&str] = &["shutterspeed", "f-number", "iso"];

/// gphoto widget 类型值（gphoto2-widget.h，x64 下 enum = int）。
mod widget_type {
    use std::ffi::c_int;
    pub const TEXT: c_int = 2;
    pub const RANGE: c_int = 3;
    pub const TOGGLE: c_int = 4;
    pub const RADIO: c_int = 5;
    pub const MENU: c_int = 6;
    pub const BUTTON: c_int = 7;
}

/// 展示为「动作按钮」的 TOGGLE（触发型，非状态开关）。
const ACTION_NAMES: &[&str] = &["autofocus"];
/// 不暴露的危险/内部项（bulb/movie 长按语义危险，capture 与快门按钮重复，
/// opcode 是 PTP 调试后门，remotekey* 是遥控键模拟）。
const EXCLUDED_NAMES: &[&str] = &[
    "opcode",
    "bulb",
    "movie",
    "capture",
    "remotekeyup",
    "remotekeydown",
    "remotekeyleft",
    "remotekeyright",
];
/// 只读状态文本的杂音面（serialnumber/manufacturer 等）不进参数面板。
const EXCLUDED_SECTIONS: &[&str] = &["status"];

/// 厂商裸码名（/main/other 的 4 位十六进制，无语义标签）。
fn is_vendor_code_name(name: &str) -> bool {
    name.len() == 4 && name.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub struct GphotoBackend {
    connections: Mutex<HashMap<String, Arc<GphotoConnection>>>,
    /// 相机级全局串行锁（Camera 非线程安全；枚举/连接同样串行）。
    camera_mutex: Mutex<()>,
    /// 相机锁等待超时（测试注入短值；生产 CAMERA_LOCK_TIMEOUT）。
    lock_timeout: Duration,
}

impl Default for GphotoBackend {
    fn default() -> Self {
        Self {
            connections: Mutex::new(HashMap::new()),
            camera_mutex: Mutex::new(()),
            lock_timeout: CAMERA_LOCK_TIMEOUT,
        }
    }
}

/// 拿相机锁的超时（拍摄+下载合法占锁可达数秒；超时说明持锁调用大概率
/// 已因 USB 卡死挂住——快速失败，避免后续命令在 blocking 池无限堆积）。
const CAMERA_LOCK_TIMEOUT: Duration = Duration::from_secs(8);

impl GphotoBackend {
    /// 相机锁（限时 + 中毒恢复）。gphoto 的 C 调用可能因 USB/相机状态
    /// 卡死永不返回：若此处无限等锁，帧轮询（30/s）与参数轮询（3s）会
    /// 持续把 spawn_blocking 线程堆在锁上，最终耗尽线程池让整个应用
    /// IPC 饿死（联拍窗口卡死的根因，2026-09-29）。超时返回「相机忙」，
    /// UI 层把错误透传给用户而非冻结。
    fn lock_camera(&self) -> Result<std::sync::MutexGuard<'_, ()>, TetherError> {
        let deadline = std::time::Instant::now() + self.lock_timeout;
        loop {
            match self.camera_mutex.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                    // 持锁线程 panic 遗毒：恢复互斥量（锁本身无 invariant）
                    return Ok(poisoned.into_inner());
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(TetherError::Other(
                            "相机忙：上一次相机操作无响应，请稍候或断开重连".into(),
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
    }

    fn connection(&self, id: &str) -> Result<Arc<GphotoConnection>, TetherError> {
        self.connections
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(TetherError::Disconnected)
    }

    /// Download only. Camera originals are never deleted by tethered capture.
    #[allow(clippy::too_many_arguments)]
    unsafe fn download(
        s: &Symbols,
        ctx: *mut std::ffi::c_void,
        conn: &GphotoConnection,
        folder: &CStr,
        name: &CStr,
    ) -> Result<CapturedObject, TetherError> {
        let mut file: *mut gpt::CameraFile = std::ptr::null_mut();
        check((s.gp_file_new)(&mut file))?;
        let result = (|| {
            check((s.gp_camera_file_get)(
                conn.camera,
                folder.as_ptr(),
                name.as_ptr(),
                gpt::GP_FILE_TYPE_NORMAL,
                file,
                ctx,
            ))?;
            let mut data: *const c_char = std::ptr::null();
            let mut size: c_ulong = 0;
            check((s.gp_file_get_data_and_size)(file, &mut data, &mut size))?;
            if data.is_null() || size == 0 {
                return Err(TetherError::Other("相机文件为空".into()));
            }
            let bytes = std::slice::from_raw_parts(data as *const u8, size as usize).to_vec();
            let name_str = cstr(name.as_ptr());
            super::staging::persist(&conn.incoming, &name_str, &bytes)
        })();
        (s.gp_file_free)(file);
        result
    }

    /// 在 /main 各 section 里按叶子名查 widget（list_config 的单配置名与
    /// 树中叶子名一致；两级查找）。
    unsafe fn find_widget_by_leaf_name(
        s: &Symbols,
        root: *mut gpt::CameraWidget,
        name: &str,
    ) -> Result<*mut gpt::CameraWidget, TetherError> {
        let name_c = CString::new(name).unwrap();
        let sections = (s.gp_widget_count_children)(root);
        let mut found: *mut gpt::CameraWidget = std::ptr::null_mut();
        for i in 0..sections {
            let mut section: *mut gpt::CameraWidget = std::ptr::null_mut();
            if (s.gp_widget_get_child)(root, i, &mut section) < 0 {
                continue;
            }
            let mut leaf: *mut gpt::CameraWidget = std::ptr::null_mut();
            if (s.gp_widget_get_child_by_name)(section, name_c.as_ptr(), &mut leaf) >= 0 {
                found = leaf;
                break;
            }
        }
        if found.is_null() {
            return Err(TetherError::Other(format!("相机没有配置项 {name}")));
        }
        Ok(found)
    }

    /// 单个 widget → CameraSetting（type 决定 kind/取值/选项/范围）。
    unsafe fn widget_to_setting(
        s: &Symbols,
        name: &str,
        widget: *mut gpt::CameraWidget,
    ) -> Option<CameraSetting> {
        let mut wtype: c_int = 0;
        (s.gp_widget_get_type)(widget, &mut wtype);
        let mut label_ptr: *const c_char = std::ptr::null();
        (s.gp_widget_get_label)(widget, &mut label_ptr);
        let label = cstr(label_ptr);
        let mut readonly: c_int = 0;
        (s.gp_widget_get_readonly)(widget, &mut readonly);
        let writable = readonly == 0;
        match wtype {
            widget_type::RADIO | widget_type::MENU => {
                let mut current: *const c_char = std::ptr::null();
                let value_ok = (s.gp_widget_get_value)(
                    widget,
                    &mut current as *mut _ as *mut std::ffi::c_void,
                ) >= 0;
                let current_str = if value_ok {
                    cstr(current)
                } else {
                    String::new()
                };
                let count = (s.gp_widget_count_choices)(widget);
                let mut options = Vec::new();
                let mut has_current = false;
                for i in 0..count {
                    let mut choice: *const c_char = std::ptr::null();
                    if (s.gp_widget_get_choice)(widget, i, &mut choice) < 0 || choice.is_null() {
                        continue;
                    }
                    let option_label = cstr(choice);
                    if option_label == current_str {
                        has_current = true;
                    }
                    options.push(CameraSettingOption {
                        value: option_label.clone(),
                        label: option_label,
                    });
                }
                if !has_current && !current_str.is_empty() {
                    options.push(CameraSettingOption {
                        value: current_str.clone(),
                        label: current_str.clone(),
                    });
                }
                Some(CameraSetting {
                    id: name.to_string(),
                    label,
                    kind: SettingKind::Choice,
                    current: current_str,
                    writable,
                    options,
                    min: None,
                    max: None,
                    step: None,
                })
            }
            widget_type::RANGE => {
                let mut value: f32 = 0.0;
                let value_ok =
                    (s.gp_widget_get_value)(widget, &mut value as *mut _ as *mut std::ffi::c_void)
                        >= 0;
                let mut lo: f32 = 0.0;
                let mut hi: f32 = 0.0;
                let mut step: f32 = 0.0;
                (s.gp_widget_get_range)(widget, &mut lo, &mut hi, &mut step);
                Some(CameraSetting {
                    id: name.to_string(),
                    label,
                    kind: SettingKind::Range,
                    current: trim_float(if value_ok { value } else { lo }),
                    writable,
                    options: Vec::new(),
                    min: Some(lo as f64),
                    max: Some(hi as f64),
                    step: Some((if step > 0.0 { step } else { 1.0 }) as f64),
                })
            }
            widget_type::TOGGLE | widget_type::BUTTON => {
                let mut value: c_int = 0;
                let value_ok =
                    (s.gp_widget_get_value)(widget, &mut value as *mut _ as *mut std::ffi::c_void)
                        >= 0;
                let current = if value_ok {
                    value.to_string()
                } else {
                    "0".into()
                };
                let is_action = ACTION_NAMES.contains(&name);
                let options = if is_action {
                    Vec::new()
                } else {
                    vec![
                        CameraSettingOption {
                            value: "1".into(),
                            label: "On".into(),
                        },
                        CameraSettingOption {
                            value: "0".into(),
                            label: "Off".into(),
                        },
                    ]
                };
                Some(CameraSetting {
                    id: name.to_string(),
                    label,
                    kind: if is_action {
                        SettingKind::Action
                    } else {
                        SettingKind::Toggle
                    },
                    current,
                    writable,
                    options,
                    min: None,
                    max: None,
                    step: None,
                })
            }
            // TEXT（含 spotfocusarea，由 focus_at 专用路径使用）/DATE 不进面板
            _ => None,
        }
    }

    /// 本地遍历已经取回的配置树，不为每个参数再次发送 USB 请求。
    unsafe fn collect_settings(
        s: &Symbols,
        widget: *mut gpt::CameraWidget,
        out: &mut Vec<CameraSetting>,
    ) {
        let mut name_ptr: *const c_char = std::ptr::null();
        (s.gp_widget_get_name)(widget, &mut name_ptr);
        let name = cstr(name_ptr);
        if EXCLUDED_SECTIONS.contains(&name.as_str()) {
            return;
        }
        let count = (s.gp_widget_count_children)(widget);
        if count > 0 {
            for index in 0..count {
                let mut child = std::ptr::null_mut();
                if (s.gp_widget_get_child)(widget, index, &mut child) >= 0 && !child.is_null() {
                    Self::collect_settings(s, child, out);
                }
            }
        } else if !name.is_empty()
            && !is_vendor_code_name(&name)
            && !EXCLUDED_NAMES.contains(&name.as_str())
        {
            if let Some(setting) = Self::widget_to_setting(s, &name, widget) {
                out.push(setting);
            }
        }
    }

    /// 值按 widget 类型写入（Choice/Text 传 char* 本体——RADIO 的 set_value
    /// 约定与 get 不对称，传 &ptr 写入的是指针字节，相机会静默忽略；真机
    /// probe6 实测，2026-09-29）。
    unsafe fn write_widget_value(
        s: &Symbols,
        widget: *mut gpt::CameraWidget,
        wtype: c_int,
        value: &str,
    ) -> Result<(), TetherError> {
        match wtype {
            widget_type::RADIO | widget_type::MENU | widget_type::TEXT => {
                let value_c = CString::new(value).unwrap();
                check((s.gp_widget_set_value)(
                    widget,
                    value_c.as_ptr() as *const std::ffi::c_void,
                ))?;
            }
            widget_type::TOGGLE | widget_type::BUTTON => {
                let parsed: c_int = value
                    .parse()
                    .map_err(|_| TetherError::Other("开关参数值无效".into()))?;
                check((s.gp_widget_set_value)(
                    widget,
                    &parsed as *const c_int as *const std::ffi::c_void,
                ))?;
            }
            widget_type::RANGE => {
                let mut parsed: f32 = value
                    .parse()
                    .map_err(|_| TetherError::Other("数值参数无效".into()))?;
                let mut lo: f32 = 0.0;
                let mut hi: f32 = 0.0;
                let mut step: f32 = 0.0;
                (s.gp_widget_get_range)(widget, &mut lo, &mut hi, &mut step);
                parsed = parsed.clamp(lo.min(hi), lo.max(hi));
                check((s.gp_widget_set_value)(
                    widget,
                    &parsed as *const f32 as *const std::ffi::c_void,
                ))?;
            }
            _ => return Err(TetherError::Other("该配置项不支持设置".into())),
        }
        Ok(())
    }

    /// 通用写路径：get_config → 按名找叶 → 按类型写值 → set_config(root)。
    /// set_single_config 在 ptp2 上返回 -2（真机 probe5 实测），故走全树。
    unsafe fn apply_setting_by_name(
        s: &Symbols,
        ctx: *mut std::ffi::c_void,
        camera: *mut gpt::Camera,
        name: &str,
        value: &str,
    ) -> Result<(), TetherError> {
        let mut root: *mut gpt::CameraWidget = std::ptr::null_mut();
        check((s.gp_camera_get_config)(camera, &mut root, ctx))?;
        let inner = (|| {
            let widget = Self::find_widget_by_leaf_name(s, root, name)?;
            let mut wtype: c_int = 0;
            (s.gp_widget_get_type)(widget, &mut wtype);
            // Choice 值必须在候选内（Range 由 write 钳制；Toggle/Action 限 0/1）
            if wtype == widget_type::RADIO || wtype == widget_type::MENU {
                let count = (s.gp_widget_count_choices)(widget);
                let mut allowed = false;
                for i in 0..count {
                    let mut choice: *const c_char = std::ptr::null();
                    if (s.gp_widget_get_choice)(widget, i, &mut choice) >= 0
                        && !choice.is_null()
                        && cstr(choice) == value
                    {
                        allowed = true;
                        break;
                    }
                }
                if !allowed {
                    return Err(TetherError::Other("该参数值在当前相机模式下不可用".into()));
                }
            }
            if (wtype == widget_type::TOGGLE || wtype == widget_type::BUTTON)
                && !matches!(value, "0" | "1")
            {
                return Err(TetherError::Other("开关参数值无效".into()));
            }
            Self::write_widget_value(s, widget, wtype, value)?;
            check((s.gp_camera_set_config)(camera, root, ctx))?;
            // Sony 属性异步生效：立即回读会拿到旧值（真机 probe6 实测），
            // 等 180ms 再让上层刷新。
            std::thread::sleep(std::time::Duration::from_millis(180));
            Ok(())
        })();
        (s.gp_widget_free)(root);
        inner
    }
}

/// f32 整数值去掉无意义的 .0（ISO/对焦步进显示用）。
fn trim_float(v: f32) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

impl CameraBackend for GphotoBackend {
    fn id(&self) -> &'static str {
        "gphoto"
    }
    fn name(&self) -> &'static str {
        "libgphoto2"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::FILE_TRANSFER
            .union(Capabilities::STANDARD_CAPTURE)
            .union(Capabilities::OBJECT_ADDED_EVENTS)
            .union(Capabilities::LIVE_VIEW)
    }

    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        let _guard = self.lock_camera()?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let mut list: *mut gpt::CameraList = std::ptr::null_mut();
            let result = (|| {
                check((s.gp_list_new)(&mut list))?;
                let code = (s.gp_camera_autodetect)(list, ctx);
                if code < 0 {
                    (s.gp_list_free)(list);
                    return Err(TetherError::Other(format!("gphoto2 错误 {code}")));
                }
                let count = (s.gp_list_count)(list);
                let mut out = Vec::new();
                for i in 0..count {
                    let mut model: *const c_char = std::ptr::null();
                    let mut port: *const c_char = std::ptr::null();
                    (s.gp_list_get_name)(list, i, &mut model);
                    (s.gp_list_get_value)(list, i, &mut port);
                    out.push(CameraInfo {
                        pnp_id: format!("gphoto:{}", cstr(port)),
                        name: cstr(model),
                        capabilities: self.capabilities(),
                    });
                }
                (s.gp_list_free)(list);
                Ok(out)
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn connect(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        let lib = lib()?;
        let s = &lib.symbols;
        let port = pnp_id.strip_prefix("gphoto:").unwrap_or(pnp_id).to_string();
        let _guard = self.lock_camera()?;
        if self.connections.lock().unwrap().contains_key(pnp_id) {
            return Err(TetherError::Other("相机已有连接，请先结束当前会话".into()));
        }
        unsafe {
            let ctx = context_new(s);
            let result = (|| -> Result<CameraInfo, TetherError> {
                let mut ports: *mut gpt::GPPortInfoList = std::ptr::null_mut();
                check((s.gp_port_info_list_new)(&mut ports))?;
                let inner = {
                    let r = (|| -> Result<CameraInfo, TetherError> {
                        check((s.gp_port_info_list_load)(ports))?;
                        let path_c = CString::new(&*port).unwrap();
                        let index =
                            check((s.gp_port_info_list_lookup_path)(ports, path_c.as_ptr()))?;
                        let mut info = std::mem::zeroed::<gpt::GPPortInfo>();
                        check((s.gp_port_info_list_get_info)(ports, index, &mut info))?;
                        let mut camera: *mut gpt::Camera = std::ptr::null_mut();
                        check((s.gp_camera_new)(&mut camera))?;
                        if let Err(e) = check((s.gp_camera_set_port_info)(camera, info))
                            .and_then(|_| check((s.gp_camera_init)(camera, ctx)))
                        {
                            (s.gp_camera_free)(camera);
                            return Err(e);
                        }
                        // 机型名取 abilities 首成员（model[128]）——init 后不可
                        // 再跑 autodetect（会与本连接争抢 USB 接口声明）。
                        let mut abilities = vec![0u8; 4096];
                        let name = if (s.gp_camera_get_abilities)(
                            camera,
                            abilities.as_mut_ptr() as *mut std::ffi::c_void,
                        ) >= 0
                            && abilities[0] != 0
                        {
                            cstr(abilities.as_ptr() as *const c_char)
                        } else {
                            port.clone()
                        };
                        let incoming = std::env::temp_dir()
                            .join("PhotoHub-gphoto")
                            .join(uuid::Uuid::new_v4().to_string());
                        if let Err(error) = std::fs::create_dir_all(&incoming) {
                            let _ = (s.gp_camera_exit)(camera, ctx);
                            (s.gp_camera_free)(camera);
                            return Err(TetherError::Other(error.to_string()));
                        }
                        let conn = Arc::new(GphotoConnection {
                            camera,
                            incoming,
                            connected: AtomicBool::new(true),
                            settings_revision: AtomicU64::new(0),
                        });
                        self.connections
                            .lock()
                            .unwrap()
                            .insert(pnp_id.to_string(), conn);
                        Ok(CameraInfo {
                            pnp_id: pnp_id.to_string(),
                            name,
                            capabilities: self.capabilities(),
                        })
                    })();
                    (s.gp_port_info_list_free)(ports);
                    r
                };
                inner
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn disconnect(&self, pnp_id: &str) {
        // Bounded locking preserves dev's responsiveness. A timed-out native
        // call still owns its handle: retain it (and its receipts), never free
        // it concurrently or allow a replacement connection with the same id.
        let Ok(_guard) = self.lock_camera() else {
            eprintln!("相机仍忙，保留连接和未入库文件: {pnp_id}");
            return;
        };
        let conn = self.connections.lock().unwrap().remove(pnp_id);
        if let Some(conn) = conn {
            conn.connected.store(false, Ordering::Release);
            if let Ok(lib) = lib() {
                unsafe {
                    let ctx = context_new(&lib.symbols);
                    let _ = (lib.symbols.gp_camera_exit)(conn.camera, ctx);
                    (lib.symbols.gp_camera_free)(conn.camera);
                    (lib.symbols.gp_context_unref)(ctx);
                }
            }
            super::staging::disconnect(&conn.incoming);
        }
    }

    /// 全参数面板：一次 get_config，再在内存中遍历全部参数。
    /// 过滤：厂商裸码（/main/other 十六进制名）、EXCLUDED_NAMES 危险项、
    /// status 只读杂音（TEXT 类型天然跳过）。核心三参置顶。
    fn settings(&self, pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let result = (|| {
                let mut root: *mut gpt::CameraWidget = std::ptr::null_mut();
                check((s.gp_camera_get_config)(conn.camera, &mut root, ctx))?;
                let mut out: Vec<CameraSetting> = Vec::new();
                Self::collect_settings(s, root, &mut out);
                (s.gp_widget_free)(root);
                out.sort_by_key(|setting| {
                    CORE_SETTING_IDS
                        .iter()
                        .position(|id| *id == setting.id)
                        .unwrap_or(CORE_SETTING_IDS.len())
                });
                Ok(out)
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn set_setting(&self, pnp_id: &str, id: &str, value: &str) -> Result<(), TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let result = Self::apply_setting_by_name(s, ctx, conn.camera, id, value);
            (s.gp_context_unref)(ctx);
            result
        }
    }

    /// 点击对焦：spotfocusarea（PTP_DPC_SONY_AFAreaPosition，live view
    /// 640×480 坐标系，格式 "x,y"）+ 触发一次 autofocus。相机需处于 AF
    /// 模式且对焦区域支持定点（否则相机侧忽略）。
    fn focus_at(&self, pnp_id: &str, x: f64, y: f64) -> Result<(), TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let result = (|| {
                let px = (x.clamp(0.0, 1.0) * 639.0).round() as i32;
                let py = (y.clamp(0.0, 1.0) * 479.0).round() as i32;
                Self::apply_setting_by_name(
                    s,
                    ctx,
                    conn.camera,
                    "spotfocusarea",
                    &format!("{px},{py}"),
                )?;
                Self::apply_setting_by_name(s, ctx, conn.camera, "autofocus", "1")?;
                Ok(())
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn live_view_frame(&self, pnp_id: &str) -> Result<Vec<u8>, TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let mut file: *mut gpt::CameraFile = std::ptr::null_mut();
            let result = (|| {
                check((s.gp_file_new)(&mut file))?;
                let inner = (|| {
                    check((s.gp_camera_capture_preview)(conn.camera, file, ctx))?;
                    let mut data: *const c_char = std::ptr::null();
                    let mut size: c_ulong = 0;
                    check((s.gp_file_get_data_and_size)(file, &mut data, &mut size))?;
                    if data.is_null() || size == 0 || size > 32 * 1024 * 1024 {
                        return Err(TetherError::Other("取景帧不可用".into()));
                    }
                    Ok(std::slice::from_raw_parts(data as *const u8, size as usize).to_vec())
                })();
                (s.gp_file_free)(file);
                inner
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn trigger_capture(&self, pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let result = (|| {
                let mut path = std::mem::zeroed::<gpt::CameraFilePath>();
                check((s.gp_camera_capture)(
                    conn.camera,
                    gpt::GP_CAPTURE_IMAGE,
                    &mut path,
                    ctx,
                ))?;
                let folder = CStr::from_ptr(path.folder.as_ptr()).to_owned();
                let name = CStr::from_ptr(path.name.as_ptr()).to_owned();
                let object = Self::download(s, ctx, &conn, &folder, &name)?;
                Ok(vec![object])
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn poll_objects(&self, pnp_id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        let _guard = self.lock_camera()?;
        let conn = self.connection(pnp_id)?;
        if !conn.connected.load(Ordering::Acquire) {
            return Err(TetherError::Disconnected);
        }
        let lib = lib()?;
        let s = &lib.symbols;
        unsafe {
            let ctx = context_new(s);
            let result = (|| -> Result<Vec<CapturedObject>, TetherError> {
                let mut eventtype: c_int = 0;
                let mut eventdata: *mut std::ffi::c_void = std::ptr::null_mut();
                let code = (s.gp_camera_wait_for_event)(
                    conn.camera,
                    50,
                    &mut eventtype,
                    &mut eventdata,
                    ctx,
                );
                let _event_data = GphotoEventData(eventdata);
                if matches!(code, -10 | -110) {
                    // 空轮超时或相机暂忙，继续会话，不误报拔线。
                    return Ok(Vec::new());
                }
                if code == -52 {
                    conn.connected.store(false, Ordering::Release);
                    return Err(TetherError::Disconnected);
                }
                if code < 0 {
                    return Err(TetherError::Other(format!("gphoto2 错误 {code}")));
                }
                let mut out = Vec::new();
                if eventtype == gpt::GP_EVENT_UNKNOWN && !eventdata.is_null() {
                    let description = cstr(eventdata as *const c_char);
                    if description.starts_with("PTP Property ")
                        || description.starts_with("PTP Deviceinfo changed")
                    {
                        conn.settings_revision.fetch_add(1, Ordering::AcqRel);
                    }
                }
                if eventtype == gpt::GP_EVENT_FILE_ADDED && !eventdata.is_null() {
                    let path = &*(eventdata as *const gpt::CameraFilePath);
                    let folder = CStr::from_ptr(path.folder.as_ptr()).to_owned();
                    let name = CStr::from_ptr(path.name.as_ptr()).to_owned();
                    // 相机端自拍：保留卡上原件（用户自己按的快门）
                    out.push(Self::download(s, ctx, &conn, &folder, &name)?);
                }
                Ok(out)
            })();
            (s.gp_context_unref)(ctx);
            result
        }
    }

    fn open_captured(
        &self,
        pnp_id: &str,
        object: &CapturedObject,
    ) -> Result<Box<dyn std::io::Read + Send>, TetherError> {
        let conn = self.connection(pnp_id)?;
        let path = super::staging::validated_path(&conn.incoming, object)?;
        std::fs::File::open(path)
            .map(|f| Box::new(f) as Box<dyn std::io::Read + Send>)
            .map_err(|e| TetherError::Other(e.to_string()))
    }

    fn settings_revision(&self, pnp_id: &str) -> u64 {
        self.connection(pnp_id)
            .map(|connection| connection.settings_revision.load(Ordering::Acquire))
            .unwrap_or(0)
    }
    fn acknowledge_captured(
        &self,
        pnp_id: &str,
        object: &CapturedObject,
    ) -> Result<(), TetherError> {
        let conn = self.connection(pnp_id)?;
        super::staging::acknowledge(&conn.incoming, object)
    }

    fn capture_still(
        &self,
        pnp_id: &str,
        timeout: Duration,
    ) -> Result<CapturedObject, TetherError> {
        let _ = timeout; // gp_camera_capture 同步阻塞至拍完，无轮询窗口
        self.trigger_capture(pnp_id)?
            .into_iter()
            .next()
            .ok_or(TetherError::Timeout)
    }
}

#[cfg(test)]
mod lock_tests {
    use super::*;

    #[test]
    fn busy_disconnect_retains_connection_and_unacknowledged_receipts() {
        let backend = GphotoBackend {
            lock_timeout: Duration::from_millis(20),
            ..Default::default()
        };
        let incoming = tempfile::tempdir().unwrap();
        let receipt =
            super::super::staging::persist(incoming.path(), "capture.jpg", b"original").unwrap();
        let connection = Arc::new(GphotoConnection {
            // No FFI call is permitted while the camera lock is busy.
            camera: std::ptr::null_mut(),
            incoming: incoming.path().to_owned(),
            connected: AtomicBool::new(true),
            settings_revision: AtomicU64::new(0),
        });
        backend
            .connections
            .lock()
            .unwrap()
            .insert("busy".into(), connection);
        let held = backend.camera_mutex.lock().unwrap();
        backend.disconnect("busy");
        assert!(backend.connections.lock().unwrap().contains_key("busy"));
        assert_eq!(std::fs::read(receipt.object_id).unwrap(), b"original");
        // These methods must return a bounded busy error BEFORE a handle lookup.
        for error in [
            backend.settings("busy").unwrap_err(),
            backend.live_view_frame("busy").unwrap_err(),
            backend.poll_objects("busy").unwrap_err(),
        ] {
            assert!(matches!(error, TetherError::Other(ref message) if message.contains("相机忙")));
        }
        drop(held);
    }

    /// 持锁超过超时 → 快速失败「相机忙」，不再无限堆积等锁线程
    ///（联拍连续拍摄卡死窗口的根因回归）。
    #[test]
    fn lock_camera_times_out_when_held() {
        let mut backend = GphotoBackend::default();
        backend.lock_timeout = Duration::from_millis(50);
        let _held = backend.camera_mutex.lock().unwrap();
        let started = std::time::Instant::now();
        let err = backend.lock_camera().unwrap_err();
        assert!(
            started.elapsed() >= Duration::from_millis(40),
            "应等待到超时"
        );
        assert!(
            matches!(&err, TetherError::Other(msg) if msg.contains("相机忙")),
            "{err:?}"
        );
    }

    /// 互斥量中毒（持锁线程 panic）恢复而非连锁 panic。
    #[test]
    fn lock_camera_recovers_from_poison() {
        let backend = GphotoBackend::default();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                // catch 住 panic：scope 会把子线程 panic 向外传播
                let _ = std::panic::catch_unwind(|| {
                    let _guard = backend.camera_mutex.lock().unwrap();
                    panic!("持锁 panic 制造中毒");
                });
            });
        });
        // 中毒后 lock_camera 应恢复互斥量正常返回
        let recovered = backend.lock_camera();
        assert!(recovered.is_ok());
    }
}
