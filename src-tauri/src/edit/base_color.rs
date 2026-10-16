//! Version 2 presentation profiles. Source data stays linear and unmodified.
//! Curve interpolation and the RGB look use the pinned PhotoCraft algorithms.
use super::recipe::BaseProfile;

const RANGE: f32 = 16.0;
const SAMPLES: usize = 16385;

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

pub(super) fn render(
    linear: &[f32], size: [u32;2], profile: BaseProfile, exposure: f32, wb: [f32;3],
) -> Vec<f32> {
    debug_assert_eq!(linear.len(), size[0] as usize * size[1] as usize * 3);
    let lut = curve(profile);
    let gain = exposure.exp2();
    let (contrast,vibrance) = match profile {
        BaseProfile::Standard => (8.0, 5.0),
        BaseProfile::Soft => (-8.0, -3.0),
        BaseProfile::Neutral => (0.0, 0.0),
    };
    let settings = photocraft_algo::camera_raw::CameraRaw { contrast, vibrance, ..Default::default() };
    let mut output = Vec::with_capacity(linear.len());
    // These profiles contain only pixel-local controls. Bounded chunks avoid
    // allocating an additional full-size RGBA buffer for large RAW exports.
    for chunk in linear.chunks(4096 * 3) {
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
        output.extend(pixels.into_iter().flat_map(|p| [p[0],p[1],p[2]]));
    }
    output
}
