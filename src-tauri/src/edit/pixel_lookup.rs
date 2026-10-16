//! Bounded color-only LUT cache shared by CPU composition and GPU preview.
use std::{collections::VecDeque,sync::{Arc,Mutex,OnceLock}};
use photocraft_engine::doc::Adjustment;
const SIZE:usize=65;
type Cache=VecDeque<(String,Arc<Vec<f32>>)>;
fn cache()->&'static Mutex<Cache>{static CACHE:OnceLock<Mutex<Cache>>=OnceLock::new();CACHE.get_or_init(Default::default)}
pub(super) fn sample(key:String,apply:impl FnOnce(&mut [[f32;4]],usize,usize))->Result<Adjustment,String>{
 let found=cache().lock().map_err(|_|"色彩缓存不可用")?.iter().find(|(k,_)|k==&key).map(|(_,v)|v.clone());
 let lut=if let Some(lut)=found{lut}else{
  let mut rgba=Vec::with_capacity(SIZE*SIZE*SIZE);
  for b in 0..SIZE{for g in 0..SIZE{for r in 0..SIZE{rgba.push([r as f32/(SIZE-1) as f32,g as f32/(SIZE-1) as f32,b as f32/(SIZE-1) as f32,1.0]);}}}
  apply(&mut rgba,SIZE*SIZE,SIZE);
  let lut=Arc::new(rgba.into_iter().flat_map(|p|[p[0],p[1],p[2]]).collect::<Vec<f32>>());
  let mut cache=cache().lock().map_err(|_|"色彩缓存不可用")?;cache.push_front((key,lut.clone()));while cache.len()>8{cache.pop_back();}lut
 };
 Ok(Adjustment::ColorLookup{name:"Editor color operation".into(),lut:Some(lut),size:SIZE as u32,tetrahedral:true,dither:false})
}
