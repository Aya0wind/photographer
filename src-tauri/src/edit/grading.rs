//! Sample the upstream grading operation into a bounded GPU color lookup.
//! No grading mathematics are duplicated; parameters remain the saved truth.
use std::{collections::VecDeque,sync::{Arc,Mutex,OnceLock}};
use photocraft_engine::doc::Adjustment;
use super::recipe::ColorGrading;
const SIZE:usize=65;
type Cache=VecDeque<(String,Arc<Vec<f32>>)>;
fn cache()->&'static Mutex<Cache>{static CACHE:OnceLock<Mutex<Cache>>=OnceLock::new();CACHE.get_or_init(Default::default)}
pub(super) fn params(g:&ColorGrading)->photocraft_algo::camera_raw::CameraRaw{
 photocraft_algo::camera_raw::CameraRaw{grade_shadows:g.shadows,grade_midtones:g.midtones,grade_highlights:g.highlights,grade_global:g.global,grade_blending:g.blending as f32,grade_balance:g.balance as f32,..Default::default()}
}
pub(super) fn adjustment(g:&ColorGrading)->Result<Adjustment,String>{
 let mut effective=g.clone();
 for w in [&mut effective.shadows,&mut effective.midtones,&mut effective.highlights,&mut effective.global]{if w.sat==0.0{w.hue=0.0;}}
 if [effective.shadows,effective.midtones,effective.highlights].iter().all(|w|w.sat==0.0&&w.lum==0.0){effective.blending=50.0;effective.balance=0.0;}
 let key=serde_json::to_string(&effective).map_err(|e|e.to_string())?;
 let found=cache().lock().map_err(|_|"分级缓存不可用")?.iter().find(|(k,_)|k==&key).map(|(_,v)|v.clone());
 let lut=if let Some(lut)=found {lut}else{
  let mut rgba=Vec::with_capacity(SIZE*SIZE*SIZE);
  for b in 0..SIZE{for green in 0..SIZE{for r in 0..SIZE{rgba.push([r as f32/(SIZE-1) as f32,green as f32/(SIZE-1) as f32,b as f32/(SIZE-1) as f32,1.0]);}}}
  photocraft_algo::camera_raw::develop(&mut rgba,SIZE*SIZE,SIZE,&params(&effective),false);
  let lut:Arc<Vec<f32>>=Arc::new(rgba.into_iter().flat_map(|p|[p[0],p[1],p[2]]).collect());
  let mut cache=cache().lock().map_err(|_|"分级缓存不可用")?;cache.push_front((key,lut.clone()));while cache.len()>4{cache.pop_back();}lut
 };
 Ok(Adjustment::ColorLookup{name:"Color grading".into(),lut:Some(lut),size:SIZE as u32,tetrahedral:true,dither:false})
}
#[cfg(test)]
#[path="grading_tests.rs"]
mod tests;
