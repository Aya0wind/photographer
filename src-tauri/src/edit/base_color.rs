//! Version 2 presentation profiles. Source data stays linear and unmodified.
//! Curve interpolation and the RGB look use the pinned PhotoCraft algorithms.
use super::recipe::BaseProfile;
use rayon::prelude::*;
use std::sync::OnceLock;

const RANGE: f32 = 16.0;
const SAMPLES: usize = 16385;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parallel_profiles_preserve_every_channel_exactly(){
        let rgb:Vec<f32>=(0..513*257*3).map(|i|(i%1024) as f32/128.0-0.02).collect();
        for profile in [BaseProfile::Standard,BaseProfile::Soft,BaseProfile::Neutral]{
            assert_eq!(render_impl(&rgb,[513,257],profile,0.7,[0.8,1.0,1.2],true),render_impl(&rgb,[513,257],profile,0.7,[0.8,1.0,1.2],false));
        }
    }
    #[test]
    #[ignore="explicit preview performance measurement"]
    fn profile_preview_timing(){
        let rgb:Vec<f32>=(0..1600*1067*3).map(|i|(i%1024) as f32/512.0).collect();
        let start=std::time::Instant::now();
        for exposure in [0.1,0.2,0.3]{std::hint::black_box(render(&rgb,[1600,1067],BaseProfile::Standard,exposure,[1.0;3]));}
        eprintln!("[profile preview] {:.2} ms/frame",start.elapsed().as_secs_f64()*1000.0/3.0);
    }
}

fn curve(profile: BaseProfile) -> Vec<f32> {
    // Scene-linear input stops above diffuse white roll into a fixed shoulder.
    // Fixed curves (no image statistics) keep proxy and export semantics equal.
    let shoulder = match profile {
        BaseProfile::Standard => [0.76, 0.91, 0.98],
        BaseProfile::Soft => [0.70, 0.87, 0.96],
        BaseProfile::Neutral => [0.80, 0.93, 0.985],
    };
    let points = [
        [0.0, 0.0], [0.02, 0.02], [0.18, 0.18], [0.5, 0.46],
        [1.0, shoulder[0]], [2.0, shoulder[1]], [4.0, shoulder[2]], [RANGE, 1.0],
    ].map(|[x,y]| [x / RANGE * 255.0, y * 255.0]);
    photocraft_algo::camera_raw::curve_lut(&points, SAMPLES)
}

fn encode(value: f32) -> f32 {
    if value <= 0.0031308 { 12.92 * value } else { 1.055 * value.powf(1.0 / 2.4) - 0.055 }
}

fn cached_curve(profile:BaseProfile)->&'static [f32]{
    static STANDARD:OnceLock<Vec<f32>>=OnceLock::new();
    static SOFT:OnceLock<Vec<f32>>=OnceLock::new();
    static NEUTRAL:OnceLock<Vec<f32>>=OnceLock::new();
    match profile {
        BaseProfile::Standard=>STANDARD.get_or_init(||curve(profile)),
        BaseProfile::Soft=>SOFT.get_or_init(||curve(profile)),
        BaseProfile::Neutral=>NEUTRAL.get_or_init(||curve(profile)),
    }
}

pub(super) fn render(
    linear: &[f32], size: [u32;2], profile: BaseProfile, exposure: f32, wb: [f32;3],
) -> Vec<f32> {
    render_impl(linear,size,profile,exposure,wb,true)
}
fn render_impl(linear:&[f32],size:[u32;2],profile:BaseProfile,exposure:f32,wb:[f32;3],parallel:bool)->Vec<f32>{
    debug_assert_eq!(linear.len(), size[0] as usize * size[1] as usize * 3);
    let lut = cached_curve(profile);
    let gain = exposure.exp2();
    let (contrast,vibrance) = match profile {
        BaseProfile::Standard => (8.0, 5.0),
        BaseProfile::Soft => (-8.0, -3.0),
        BaseProfile::Neutral => (0.0, 0.0),
    };
    let settings = photocraft_algo::camera_raw::CameraRaw { contrast, vibrance, ..Default::default() };
    let mut output = vec![0.0;linear.len()];
    // These profiles contain only pixel-local controls. Bounded chunks avoid
    // allocating an additional full-size RGBA buffer for large RAW exports.
    let write=|(destination,chunk):(&mut [f32],&[f32])| {
        let mut pixels: Vec<[f32;4]> = chunk.chunks_exact(3).map(|rgb| {
            let c: [f32;3] = std::array::from_fn(|i| rgb[i] * gain * wb[i]);
            let luma = (0.2126*c[0] + 0.7152*c[1] + 0.0722*c[2]).max(0.0);
            let index = (luma / RANGE).clamp(0.0,1.0) * (SAMPLES-1) as f32;
            let lo = index.floor() as usize;
            let hi = (lo+1).min(SAMPLES-1);
            let mapped = lut[lo] + (lut[hi]-lut[lo]) * (index-lo as f32);
            let ratio = if luma > 1e-8 { mapped/luma } else { 1.0 };
            // Explicit SDR presentation boundary; source tiles stay unchanged.
            [encode((c[0]*ratio).clamp(0.0,1.0)), encode((c[1]*ratio).clamp(0.0,1.0)),
             encode((c[2]*ratio).clamp(0.0,1.0)), 1.0]
        }).collect();
        let count = pixels.len();
        if contrast != 0.0 || vibrance != 0.0 {
            photocraft_algo::camera_raw::develop(&mut pixels,count,1,&settings,false);
        }
        for (out,pixel) in destination.chunks_exact_mut(3).zip(pixels){out.copy_from_slice(&pixel[..3]);}
    };
    static POOL:OnceLock<Option<rayon::ThreadPool>>=OnceLock::new();
    let pool=POOL.get_or_init(||rayon::ThreadPoolBuilder::new().num_threads(num_cpus::get().clamp(1,4)).thread_name(|i|format!("editor-color-{i}")).build().ok());
    if let Some(pool)=pool.as_ref().filter(|_|parallel&&linear.len()>=65536*3){
        pool.install(||output.par_chunks_mut(4096*3).zip(linear.par_chunks(4096*3)).for_each(write));
    }else{
        output.chunks_mut(4096*3).zip(linear.chunks(4096*3)).for_each(write);
    }
    output
}
