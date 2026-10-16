//! Versioned creative color operations. These are not camera calibration matrices.
use super::{pixel_lookup,recipe::{AdvancedAdjustments,PointColor,PrimaryShift}};
use photocraft_engine::doc::Adjustment;
use std::collections::BTreeMap;

pub(super) fn normalize(a:&mut AdvancedAdjustments)->Result<(),String>{
    if a.point_colors.len()>8{return Err("点颜色最多保存 8 个颜色".into());}
    let mut ids=std::collections::HashSet::new();
    for point in &mut a.point_colors {
        if point.id.is_empty()||point.id.len()>96||!ids.insert(point.id.clone())||super::recipe::parse_color(&point.color).is_none(){return Err("点颜色标识或颜色无效".into());}
        for (value,min,max) in [(&mut point.hue_range,1.0,180.0),(&mut point.saturation_range,1.0,100.0),(&mut point.luminance_range,1.0,100.0),(&mut point.hue,-180.0,180.0),(&mut point.saturation,-100.0,100.0),(&mut point.luminance,-100.0,100.0)]{
            if !value.is_finite(){return Err("点颜色参数无效".into());}*value=value.clamp(min,max);
        }
    }
    if let Some(primaries)=&mut a.primaries {
        for (name,value) in primaries {
            if !["red","green","blue"].contains(&name.as_str())||!value.hue.is_finite()||!value.saturation.is_finite(){return Err("原色调色参数无效".into());}
            value.hue=value.hue.clamp(-30.0,30.0);value.saturation=value.saturation.clamp(-100.0,100.0);
        }
    }
    Ok(())
}
fn shift(value:f32,amount:f32)->f32{(value+if amount>=0.0{(1.0-value)*amount}else{value*amount}).clamp(0.0,1.0)}
pub(super) fn points(points:&[PointColor])->Result<Adjustment,String>{
    use photocraft_compose::adjust::{rgb_to_hsl,hsl_to_rgb};
    let references:Vec<_>=points.iter().map(|point|{
        let rgb=super::recipe::parse_color(&point.color).expect("validated point color").map(|v|v as f32/255.0);
        (point,rgb_to_hsl(rgb))
    }).collect();
    pixel_lookup::sample(format!("point-color-v1:{}",serde_json::to_string(points).map_err(|e|e.to_string())?),|pixels,_,_|{
        for p in pixels {
            for (point,(rh,rs,rl)) in &references {
                if !point.enabled{continue;}
                let (h,s,l)=rgb_to_hsl([p[0],p[1],p[2]]);
                let dh=(h-rh).abs();let dh=dh.min(1.0-dh)*360.0/point.hue_range;
                let ds=(s-rs).abs()*100.0/point.saturation_range;
                let dl=(l-rl).abs()*100.0/point.luminance_range;
                let weight=(1.0-dh.max(ds).max(dl)).clamp(0.0,1.0);let weight=weight*weight*(3.0-2.0*weight);
                let rgb=hsl_to_rgb((h+point.hue/360.0*weight).rem_euclid(1.0),shift(s,point.saturation/100.0*weight),shift(l,point.luminance/100.0*weight));
                p[..3].copy_from_slice(&rgb);
            }
        }
    })
}
pub(super) fn primaries(values:&BTreeMap<String,PrimaryShift>)->Result<Adjustment,String>{
    let columns:Vec<_>=["red","green","blue"].iter().enumerate().map(|(i,name)|{
        let (hue,saturation)=values.get(*name).map_or((0.0,0.0),|v|(v.hue,v.saturation));
        photocraft_compose::adjust::hsl_to_rgb((i as f32/3.0+hue/360.0).rem_euclid(1.0),1.0+saturation/100.0,0.5)
    }).collect();
    let rows:[[f32;3];3]=std::array::from_fn(|row|{
        let sum=columns.iter().map(|p|p[row]).sum::<f32>().max(0.05);
        std::array::from_fn(|column|columns[column][row]/sum)
    });
    pixel_lookup::sample(format!("rgb-primaries-v1:{}",serde_json::to_string(values).map_err(|e|e.to_string())?),|pixels,_,_|{
        for p in pixels {let rgb=[p[0],p[1],p[2]];for row in 0..3{p[row]=(0..3).map(|c|rows[row][c]*rgb[c]).sum::<f32>().clamp(0.0,1.0);}}
    })
}
