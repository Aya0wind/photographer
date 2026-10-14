// SPDX-License-Identifier: GPL-3.0-only
//! Fixed RawLab core behind a trusted float bridge. No rendering algorithms are
//! duplicated here; app hosts own library loading, source leases and budgets.
use libloading::Library;
use std::{
    ffi::{c_char, c_void, CStr, CString, OsStr},
    sync::atomic::{AtomicBool, Ordering},
};
type Cancel = extern "C" fn(*mut c_void) -> i32;
#[repr(C)]
struct Options {
    version: u32,
    half_size: u32,
    max_pixels: u64,
    white_balance: [f32; 4],
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
type Free = unsafe extern "C" fn(*mut c_void);
pub struct Backend {
    _library: Library,
    decode: Decode,
    render: Render,
    free: Free,
}
#[derive(Clone, Copy)]
pub struct DevelopOptions {
    pub half_size: bool,
    pub max_pixels: u64,
    pub white_balance: [f32; 4],
}
impl Default for DevelopOptions {
    fn default() -> Self {
        Self {
            half_size: false,
            max_pixels: 128_000_000,
            white_balance: [0.0; 4],
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
        if unsafe { version() } != 1 {
            return Err("Unsupported RawLab bridge ABI".into());
        }
        let decode =
            *unsafe { library.get::<Decode>(b"pc_raw_decode\0") }.map_err(|e| e.to_string())?;
        let render =
            *unsafe { library.get::<Render>(b"pc_raw_render\0") }.map_err(|e| e.to_string())?;
        let free = *unsafe { library.get::<Free>(b"pc_raw_free\0") }.map_err(|e| e.to_string())?;
        Ok(Self {
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
            version: 1,
            half_size: options.half_size as u32,
            max_pixels: options.max_pixels,
            white_balance: options.white_balance,
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
            version: 1,
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
