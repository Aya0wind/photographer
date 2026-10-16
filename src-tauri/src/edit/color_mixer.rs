//! Eight-band mixing uses pinned upstream hue weights and CameraRaw HSL.
use super::{recipe::{HslRange,BlackWhite},pixel_lookup};
use std::collections::BTreeMap;
use photocraft_engine::doc::Adjustment;
pub(super) const BANDS:[&str;8]=["reds","oranges","yellows","greens","cyans","blues","purples","magentas"];
pub(super) fn hsl(ranges:&BTreeMap<String,HslRange>)->Result<Adjustment,String>{
 let mut p=photocraft_algo::camera_raw::CameraRaw::default();
 for (i,name) in BANDS.iter().enumerate(){if let Some(v)=ranges.get(*name){p.hsl_hue[i]=v.hue as f32;p.hsl_sat[i]=v.saturation as f32;p.hsl_lum[i]=v.lightness as f32;}}
 let key=format!("hsl8:{}",serde_json::to_string(ranges).map_err(|e|e.to_string())?);
 pixel_lookup::sample(key,|pixels,w,h|photocraft_algo::camera_raw::develop(pixels,w,h,&p,false))
}
pub(super) fn black_white(value:&BlackWhite)->Result<Adjustment,String>{
 let weights=value.weights_eight.ok_or("八色黑白参数缺失")?;
 let tint=value.tint.as_ref().and_then(|v|super::recipe::parse_color(v)).map(|c|c.map(|v|v as f32/255.0));
 let key=format!("bw8:{}",serde_json::to_string(value).map_err(|e|e.to_string())?);
 pixel_lookup::sample(key,|pixels,_,_|{for p in pixels{
  let rgb=[p[0],p[1],p[2]];let (h,_,_)=photocraft_compose::adjust::rgb_to_hsl(rgb);
  let bands=photocraft_algo::camera_raw::band_weights(h*360.0);
  let weight=bands.iter().zip(weights).map(|(a,b)|a*b as f32/100.0).sum::<f32>();
  let min=rgb.into_iter().fold(f32::INFINITY,f32::min);let max=rgb.into_iter().fold(f32::NEG_INFINITY,f32::max);
  let gray=(min+(max-min)*weight).clamp(0.0,1.0);
  for c in 0..3{p[c]=tint.map_or(gray,|t|(gray*t[c]*2.0).clamp(0.0,1.0)*0.5+gray*0.5);}
 }})
}
