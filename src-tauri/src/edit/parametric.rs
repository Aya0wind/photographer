//! The upstream region curve is channel-separable. Sample its real algorithm
//! at the upstream 1024 tone levels into native GPU curves, preserving its
//! split controls and precision between eight-bit display levels.
use photocraft_engine::doc::{Adjustment,adjust::{CurvePoint,ToneSpace}};
use super::recipe::ParametricCurve;
pub(super) fn params(c:&ParametricCurve)->photocraft_algo::camera_raw::CameraRaw {
 photocraft_algo::camera_raw::CameraRaw{curve_highlights:c.highlights as f32,curve_lights:c.lights as f32,curve_darks:c.darks as f32,curve_shadows:c.shadows as f32,curve_splits:c.splits.map(|v|v as f32),..Default::default()}
}
pub(super) fn adjustment(c:&ParametricCurve)->Adjustment {
 const SAMPLES:usize=1024;
 let mut pixels:Vec<_>=(0..SAMPLES).map(|i|[i as f32/(SAMPLES-1) as f32,i as f32/(SAMPLES-1) as f32,i as f32/(SAMPLES-1) as f32,1.0]).collect();
 photocraft_algo::camera_raw::develop(&mut pixels,SAMPLES,1,&params(c),false);
 Adjustment::Curves{master:(0..SAMPLES).map(|i|CurvePoint{input:i as f32/(SAMPLES-1) as f32,output:pixels[i][0]}).collect(),per_channel:Default::default(),space:ToneSpace::Rgb,black:Vec::new()}
}
#[cfg(test)]
mod tests{
 use super::*;
 #[test]
 fn sampled_curve_preserves_dense_sixteen_bit_tones_at_extreme_settings(){
  for (amounts,splits) in [([100.0,-100.0,100.0,-100.0],[25.0,50.0,75.0]),([-100.0,100.0,-100.0,100.0],[5.0,6.0,7.0]),([100.0,100.0,-100.0,-100.0],[93.0,94.0,95.0])]{
   let c=ParametricCurve{shadows:amounts[0],darks:amounts[1],lights:amounts[2],highlights:amounts[3],splits};
   let original:Vec<[f32;4]>=(0..65536).map(|i|[i as f32/65535.0,((i*73)%65536) as f32/65535.0,((i*37)%65536) as f32/65535.0,1.0]).collect();
   let mut expected=original.clone();photocraft_algo::camera_raw::develop(&mut expected,65536,1,&params(&c),false);
   let mut output=photocraft_compose::Buffer{rect:photocraft_engine::doc::Rect::from_xywh(0,0,65536,1),px:original};photocraft_compose::adjust::apply(&adjustment(&c),&mut output);
   let max=output.px.iter().zip(&expected).flat_map(|(a,b)|(0..3).map(move|ch|(a[ch]-b[ch]).abs())).fold(0.0,f32::max);
   assert!(max<0.0002,"dense tone parity: error {max}, splits {splits:?}");
  }
 }
 #[test]
 fn sampled_curve_matches_upstream_full_rgb_at_all_display_levels(){
  let c=ParametricCurve{shadows:40.0,darks:-30.0,lights:25.0,highlights:-35.0,splits:[20.0,45.0,80.0]};
  let mut expected:Vec<[f32;4]>=(0..256).map(|i|[i as f32/255.0,((i*73)%256) as f32/255.0,((i*37)%256) as f32/255.0,1.0]).collect();
  let original=expected.clone();photocraft_algo::camera_raw::develop(&mut expected,256,1,&params(&c),false);
  let mut output=photocraft_compose::Buffer{rect:photocraft_engine::doc::Rect::from_xywh(0,0,256,1),px:original};photocraft_compose::adjust::apply(&adjustment(&c),&mut output);
  let max=output.px.iter().zip(&expected).flat_map(|(a,b)|(0..3).map(move|ch|(a[ch]-b[ch]).abs())).fold(0.0,f32::max);assert!(max<0.00001,"parametric parity {max}");
 }
}
