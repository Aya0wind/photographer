// SPDX-License-Identifier: GPL-3.0-only
//! Fixed RawLab core behind a trusted float bridge. No rendering algorithms are
//! duplicated here; app hosts own library loading, source leases and budgets.
use libloading::Library;
use std::{
    ffi::{c_char, c_void, CStr, CString, OsStr},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};
type Cancel = extern "C" fn(*mut c_void) -> i32;
#[repr(C)]
struct Options {
    version: u32,
    half_size: u32,
    max_pixels: u64,
    white_balance: [f32; 4],
    wb_mode:u32,
    temperature:f32,
    tint:f32,
    cancelled: Cancel,
    userdata: *mut c_void,
}
#[repr(C)]
struct FilmOptions {
    version: u32,
    lut_path: *const c_char,
    strength: f32,
    gpu_mode: u32,
    cancelled: Cancel,
    userdata: *mut c_void,
    exposure_ev: f32,
    relative_white_balance: [f32; 3],
}
#[repr(C)]
#[derive(Default)]
pub struct LutInfo {
    pub size: u32,
    /// RawLab transfer enum; preserve unknown future values for diagnostics.
    pub input_transfer: u32,
    pub output_transfer: u32,
}
#[repr(C)]
struct Output {
    width: u32,
    height: u32,
    source_width: u32,
    source_height: u32,
    color_contract: u32,
    gpu_used: u32,
    baseline_ev: f32,
    rgb: *const f32,
    samples: usize,
    owner: *mut c_void,
    error: [c_char; 256],
}
type Decode = unsafe extern "C" fn(*const c_char, *const Options, *mut Output) -> i32;
type Render = unsafe extern "C" fn(*const f32, u32, u32, *const FilmOptions, *mut Output) -> i32;
type CreateSession =
    unsafe extern "C" fn(*const f32, u32, u32, *mut *mut c_void, *mut Output) -> i32;
type RenderSession = unsafe extern "C" fn(*mut c_void, *const FilmOptions, *mut Output) -> i32;
type DestroySession = unsafe extern "C" fn(*mut c_void);
type InspectLut = unsafe extern "C" fn(*const c_char, *mut LutInfo, *mut Output) -> i32;
type Free = unsafe extern "C" fn(*mut c_void);
pub struct Backend {
    _library: Library,
    decode: Decode,
    render: Render,
    free: Free,
    create_session: CreateSession,
    render_session: RenderSession,
    destroy_session: DestroySession,
    inspect_lut: InspectLut,
}
#[derive(Clone, Copy)]
pub struct DevelopOptions {
    pub half_size: bool,
    pub max_pixels: u64,
    pub white_balance: [f32; 4],
    pub auto_white_balance:bool,
    pub temperature:Option<[f32;2]>,
}
impl Default for DevelopOptions {
    fn default() -> Self {
        Self {
            half_size: false,
            max_pixels: 128_000_000,
            white_balance: [0.0; 4],
            auto_white_balance:false,
            temperature:None,
        }
    }
}
#[derive(Clone, Copy)]
#[repr(u32)]
pub enum GpuMode {
    Off = 0,
    Auto = 1,
    Force = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    LinearSrgb,
    DisplaySrgb,
}
extern "C" fn cancelled(data: *mut c_void) -> i32 {
    unsafe { (&*data.cast::<AtomicBool>()).load(Ordering::Relaxed) as i32 }
}
fn userdata(cancel: &AtomicBool) -> *mut c_void {
    (cancel as *const AtomicBool).cast_mut().cast()
}
fn empty_output() -> Output {
    Output {
        width: 0,
        height: 0,
        source_width: 0,
        source_height: 0,
        color_contract: 0,
        gpu_used: 0,
        baseline_ev: 0.0,
        rgb: std::ptr::null(),
        samples: 0,
        owner: std::ptr::null_mut(),
        error: [0; 256],
    }
}
impl Backend {
    /// # Safety
    /// `path` must identify a trusted native/rawlab artifact for this ABI/target.
    /// Dynamic library constructors execute code at load time.
    pub unsafe fn load(path: impl AsRef<OsStr>) -> Result<Self, String> {
        let library = unsafe { Library::new(path) }.map_err(|e| e.to_string())?;
        let version =
            unsafe { library.get::<unsafe extern "C" fn() -> u32>(b"pc_raw_abi_version\0") }
                .map_err(|e| e.to_string())?;
        if unsafe { version() } != 3 {
            return Err("Unsupported RawLab bridge ABI".into());
        }
        let decode =
            *unsafe { library.get::<Decode>(b"pc_raw_decode\0") }.map_err(|e| e.to_string())?;
        let render =
            *unsafe { library.get::<Render>(b"pc_raw_render\0") }.map_err(|e| e.to_string())?;
        let free = *unsafe { library.get::<Free>(b"pc_raw_free\0") }.map_err(|e| e.to_string())?;
        let create_session = *unsafe { library.get::<CreateSession>(b"pc_color_session_create\0") }
            .map_err(|e| e.to_string())?;
        let render_session = *unsafe { library.get::<RenderSession>(b"pc_color_session_render\0") }
            .map_err(|e| e.to_string())?;
        let destroy_session =
            *unsafe { library.get::<DestroySession>(b"pc_color_session_destroy\0") }
                .map_err(|e| e.to_string())?;
        let inspect_lut = *unsafe { library.get::<InspectLut>(b"pc_raw_lut_info\0") }
            .map_err(|e| e.to_string())?;
        Ok(Self {
            create_session,
            render_session,
            destroy_session,
            inspect_lut,
            _library: library,
            decode,
            render,
            free,
        })
    }
    /// UTF-8, caller-authorized local file lease. The upstream processor opens
    /// and unpacks it; keep the source stable until this synchronous call ends.
    pub fn develop<'a>(
        &'a self,
        path: &str,
        options: DevelopOptions,
        cancel: &AtomicBool,
    ) -> Result<Image<'a>, String> {
        let path = CString::new(path).map_err(|_| "RAW source path contains NUL")?;
        let options = Options {
            version: 2,
            half_size: options.half_size as u32,
            max_pixels: options.max_pixels,
            white_balance: options.white_balance,
            wb_mode:if options.temperature.is_some(){2}else if options.auto_white_balance{1}else{0},
            temperature:options.temperature.map_or(6500.0,|v|v[0]),
            tint:options.temperature.map_or(0.0,|v|v[1]),
            cancelled,
            userdata: userdata(cancel),
        };
        let mut output = empty_output();
        let status = unsafe { (self.decode)(path.as_ptr(), &options, &mut output) };
        self.finish(output, status, cancel, Encoding::LinearSrgb)
    }
    /// linear input only; GPU Auto retains upstream CPU fallback. This prototype
    /// reuses upstream Metal/D3D11/GLES photo renderers; Linux falls back to CPU.
    pub fn render<'a>(
        &'a self,
        rgb: &[f32],
        dimensions: [u32; 2],
        lut_path: Option<&str>,
        strength: f32,
        gpu: GpuMode,
        cancel: &AtomicBool,
    ) -> Result<Image<'a>, String> {
        let [width, height] = dimensions;
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3));
        if expected != Some(rgb.len()) {
            return Err("Invalid linear RGB dimensions".into());
        }
        let path = lut_path
            .map(CString::new)
            .transpose()
            .map_err(|_| "Film path contains NUL")?;
        let options = FilmOptions {
            version: 2,
            exposure_ev: 0.0,
            relative_white_balance: [1.0; 3],
            lut_path: path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            strength,
            gpu_mode: gpu as u32,
            cancelled,
            userdata: userdata(cancel),
        };
        let mut output = empty_output();
        let status = unsafe { (self.render)(rgb.as_ptr(), width, height, &options, &mut output) };
        self.finish(output, status, cancel, Encoding::DisplaySrgb)
    }
    /// Copies a linear source once. Its GPU context must stay on this thread.
    pub fn color_session(
        &self,
        rgb: &[f32],
        dimensions: [u32; 2],
    ) -> Result<ColorSession<'_>, String> {
        let [width, height] = dimensions;
        if (width as usize)
            .checked_mul(height as usize)
            .and_then(|n| n.checked_mul(3))
            != Some(rgb.len())
        {
            return Err("Invalid linear RGB dimensions".into());
        }
        let mut pointer = std::ptr::null_mut();
        let mut diagnostics = empty_output();
        let status = unsafe {
            (self.create_session)(rgb.as_ptr(), width, height, &mut pointer, &mut diagnostics)
        };
        // Own any returned context even if a future bridge reports a partial failure.
        let session = NonNull::new(pointer).map(|pointer| ColorSession {
            backend: self,
            pointer,
            _thread: PhantomData,
        });
        diagnostic_status(status, &diagnostics)?;
        session.ok_or_else(|| "RawLab returned an empty color session".into())
    }
    /// Inspect the original CUBE, including upstream Gamma/Gamut declarations.
    pub fn lut_info(&self, path: &str) -> Result<LutInfo, String> {
        let path = CString::new(path).map_err(|_| "Film path contains NUL")?;
        let mut info = LutInfo::default();
        let mut diagnostics = empty_output();
        let status = unsafe { (self.inspect_lut)(path.as_ptr(), &mut info, &mut diagnostics) };
        diagnostic_status(status, &diagnostics)?;
        Ok(info)
    }
    fn finish<'a>(
        &'a self,
        output: Output,
        status: i32,
        cancel: &AtomicBool,
        encoding: Encoding,
    ) -> Result<Image<'a>, String> {
        let image = Image {
            backend: self,
            output,
            encoding,
        }; // own even partial failures
        if status != 0 {
            let mut error = image.output.error;
            error[255] = 0;
            return Err(unsafe { CStr::from_ptr(error.as_ptr()) }
                .to_string_lossy()
                .into_owned());
        }
        let o = &image.output;
        let expected = (o.width as usize)
            .checked_mul(o.height as usize)
            .and_then(|n| n.checked_mul(3));
        let contract = if encoding == Encoding::LinearSrgb {
            1
        } else {
            2
        };
        if o.width == 0
            || o.height == 0
            || o.color_contract != contract
            || expected != Some(o.samples)
            || o.rgb.is_null()
            || o.owner.is_null()
            || o.samples > isize::MAX as usize / std::mem::size_of::<f32>()
        {
            return Err("Invalid RawLab bridge pixel result".into());
        }
        if cancel.load(Ordering::Relaxed) {
            return Err("RawLab request cancelled".into());
        }
        Ok(image)
    }
}
/// Native-owned RGB floats. Backend outlives pixels, freed by the same library.
pub struct Image<'a> {
    backend: &'a Backend,
    output: Output,
    encoding: Encoding,
}
impl Image<'_> {
    pub fn dimensions(&self) -> [u32; 2] {
        [self.output.width, self.output.height]
    }
    /// Half-size special geometry has no exact source size in this prototype.
    pub fn source_dimensions(&self) -> Option<[u32; 2]> {
        if self.output.source_width == 0 || self.output.source_height == 0 {
            None
        } else {
            Some([self.output.source_width, self.output.source_height])
        }
    }
    pub fn baseline_ev(&self) -> f32 {
        self.output.baseline_ev
    }
    pub fn encoding(&self) -> Encoding {
        self.encoding
    }
    pub fn gpu_used(&self) -> bool {
        self.output.gpu_used != 0
    }
    pub fn rgb(&self) -> &[f32] {
        unsafe { std::slice::from_raw_parts(self.output.rgb, self.output.samples) }
    }
}
impl Drop for Image<'_> {
    fn drop(&mut self) {
        if !self.output.owner.is_null() {
            unsafe { (self.backend.free)(self.output.owner) };
        }
    }
}

/// Reusable immutable source plus platform device. !Send/!Sync intentionally:
/// create, render and release on the same dedicated host worker thread.
///
/// ```compile_fail
/// use photographer_media::rawlab::ColorSession;
/// fn requires_send<T: Send>() {}
/// requires_send::<ColorSession<'static>>();
/// ```
pub struct ColorSession<'a> {
    backend: &'a Backend,
    pointer: NonNull<c_void>,
    _thread: PhantomData<Rc<()>>,
}
#[derive(Clone, Copy)]
pub struct ColorOptions<'a> {
    pub lut_path: Option<&'a str>,
    pub strength: f32,
    pub gpu: GpuMode,
    pub exposure_ev: f32,
    /// Relative gains on developed linear sRGB, not sensor Kelvin multipliers.
    pub relative_white_balance: [f32; 3],
}
impl Default for ColorOptions<'_> {
    fn default() -> Self {
        Self {
            lut_path: None,
            strength: 1.0,
            gpu: GpuMode::Auto,
            exposure_ev: 0.0,
            relative_white_balance: [1.0; 3],
        }
    }
}
impl<'a> ColorSession<'a> {
    pub fn render(
        &mut self,
        options: ColorOptions<'_>,
        cancel: &AtomicBool,
    ) -> Result<Image<'a>, String> {
        let path = options
            .lut_path
            .map(CString::new)
            .transpose()
            .map_err(|_| "Film path contains NUL")?;
        let options = FilmOptions {
            version: 2,
            lut_path: path.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            strength: options.strength,
            gpu_mode: options.gpu as u32,
            cancelled,
            userdata: userdata(cancel),
            exposure_ev: options.exposure_ev,
            relative_white_balance: options.relative_white_balance,
        };
        let mut output = empty_output();
        let status =
            unsafe { (self.backend.render_session)(self.pointer.as_ptr(), &options, &mut output) };
        self.backend
            .finish(output, status, cancel, Encoding::DisplaySrgb)
    }
}
impl Drop for ColorSession<'_> {
    fn drop(&mut self) {
        unsafe { (self.backend.destroy_session)(self.pointer.as_ptr()) };
    }
}
fn diagnostic_status(status: i32, output: &Output) -> Result<(), String> {
    if status == 0 {
        return Ok(());
    }
    let mut error = output.error;
    error[255] = 0;
    Err(unsafe { CStr::from_ptr(error.as_ptr()) }
        .to_string_lossy()
        .into_owned())
}
