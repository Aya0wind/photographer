//! Sample the upstream grading operation into a bounded GPU color lookup.
//! No grading mathematics are duplicated; parameters remain the saved truth.
use photocraft_engine::doc::Adjustment;
use super::recipe::ColorGrading;
pub(super) fn params(g:&ColorGrading)->photocraft_algo::camera_raw::CameraRaw{
 photocraft_algo::camera_raw::CameraRaw{grade_shadows:g.shadows,grade_midtones:g.midtones,grade_highlights:g.highlights,grade_global:g.global,grade_blending:g.blending as f32,grade_balance:g.balance as f32,..Default::default()}
}
pub(super) fn adjustment(g:&ColorGrading)->Result<Adjustment,String>{
 let mut effective=g.clone();
 for w in [&mut effective.shadows,&mut effective.midtones,&mut effective.highlights,&mut effective.global]{if w.sat==0.0{w.hue=0.0;}}
 if [effective.shadows,effective.midtones,effective.highlights].iter().all(|w|w.sat==0.0&&w.lum==0.0){effective.blending=50.0;effective.balance=0.0;}
 let key=format!("grading:{}",serde_json::to_string(&effective).map_err(|e|e.to_string())?);
 super::pixel_lookup::sample(key,|pixels,w,h|photocraft_algo::camera_raw::develop(pixels,w,h,&params(&effective),false))
}
#[cfg(test)]
#[path="grading_tests.rs"]
mod tests;
