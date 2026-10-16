use super::*;
use std::sync::Arc;
#[test]
fn sampled_grading_matches_upstream_pixels_and_reuses_the_lookup(){
 let g:ColorGrading=serde_json::from_value(serde_json::json!({"shadows":{"hue":220,"sat":35,"lum":-10},"midtones":{"hue":40,"sat":15,"lum":10},"highlights":{"hue":55,"sat":20,"lum":5},"global":{"hue":120,"sat":5,"lum":0},"blending":65,"balance":-20})).unwrap();
 let first=adjustment(&g).unwrap();let second=adjustment(&g).unwrap();
 if let (Adjustment::ColorLookup{lut:Some(a),..},Adjustment::ColorLookup{lut:Some(b),..})=(&first,&second){assert!(Arc::ptr_eq(a,b));}else{panic!("grading lookup missing");}
 let mut seed=7123u32;
 let mut next=||{seed=seed.wrapping_mul(1664525).wrapping_add(1013904223);(seed>>8) as f32/16777215.0};
 let mut expected:Vec<[f32;4]>=(0..4096).map(|_|[next(),next(),next(),1.0]).collect();
 let original=expected.clone();photocraft_algo::camera_raw::develop(&mut expected,64,64,&params(&g),false);
 let mut buffer=photocraft_compose::Buffer{rect:photocraft_engine::doc::Rect::from_xywh(0,0,64,64),px:original};
 photocraft_compose::adjust::apply(&first,&mut buffer);
 let max=buffer.px.iter().zip(&expected).flat_map(|(a,b)|(0..3).map(move|ch|(a[ch]-b[ch]).abs())).fold(0.0,f32::max);
 assert!(max<0.008,"grading lookup parity error {max}");
}
#[test]
fn disabled_hue_and_unused_balance_reuse_effective_lookup(){
 let mut g=ColorGrading::default();g.global.lum=10.0;
 let a=adjustment(&g).unwrap();g.global.hue=200.0;g.blending=90.0;g.balance=-70.0;let b=adjustment(&g).unwrap();
 if let (Adjustment::ColorLookup{lut:Some(a),..},Adjustment::ColorLookup{lut:Some(b),..})=(&a,&b){assert!(Arc::ptr_eq(a,b));}else{panic!("lookup missing");}
}
