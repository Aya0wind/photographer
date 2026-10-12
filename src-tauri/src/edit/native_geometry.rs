//! Inverse display sampling: reuse PhotoCraft's canvas bounds; no pixel processing.
use super::recipe::EditRecipe;
use photocraft_engine::doc::Size;
fn edge(v:f64)->f64{if (v-v.round()).abs()<1e-7 {v.round()}else{v}}
pub(super) fn crop_bounds(crop:super::recipe::CropRect,size:Size)->[u32;4]{
    let w=f64::from(size.width);let h=f64::from(size.height);
    let x=edge(crop.x*w).floor().clamp(0.0,(w-1.0).max(0.0));let y=edge(crop.y*h).floor().clamp(0.0,(h-1.0).max(0.0));
    let right=edge((crop.x+crop.w)*w).ceil().clamp(x,w);let bottom=edge((crop.y+crop.h)*h).ceil().clamp(y,h);
    [x as u32,y as u32,(right-x).max(1.0) as u32,(bottom-y).max(1.0) as u32]
}

pub(super) fn mapping(recipe:&EditRecipe,size:Size)->[f32;8] {
    let quarter=recipe.rotate_quarter;
    let base=if quarter%2==1 {Size::new(size.height,size.width)} else {size};
    let g=recipe.geometry.clone().unwrap_or_default();
    let plane=photocraft_engine::mode_cmds::rotated_size(base,g.angle);
    let (x,y,w,h)=if let Some(crop)=recipe.crop {
        let b=crop_bounds(crop,plane);(f64::from(b[0]),f64::from(b[1]),f64::from(b[2]),f64::from(b[3]))
    } else {(0.0,0.0,f64::from(plane.width),f64::from(plane.height))};
    let a=g.angle.to_radians();let (sin,cos)=a.sin_cos();
    let inverse=|u:f64,v:f64| {
        let px=x+u*w-f64::from(plane.width)/2.0;let py=y+v*h-f64::from(plane.height)/2.0;
        let mut x=(cos*px+sin*py)/f64::from(base.width)+0.5;
        let mut y=(-sin*px+cos*py)/f64::from(base.height)+0.5;
        if g.flip_horizontal {x=1.0-x;}if g.flip_vertical {y=1.0-y;}
        match quarter {1=>[y,1.0-x],2=>[1.0-x,1.0-y],3=>[1.0-y,x],_=>[x,y]}
    };
    let zero=inverse(0.0,0.0);let horizontal=inverse(1.0,0.0);let vertical=inverse(0.0,1.0);
    [(horizontal[0]-zero[0]) as f32,(vertical[0]-zero[0]) as f32,zero[0] as f32,0.0,
     (horizontal[1]-zero[1]) as f32,(vertical[1]-zero[1]) as f32,zero[1] as f32,0.0]
}
// lib 内由 native_presenter 消费；native_canvas_probe 示例单独 #[path]
// 编译本模块时未消费。
#[allow(dead_code)]
pub(super) fn canvas_size(recipe:&EditRecipe,size:Size)->Size {
    let base=if recipe.rotate_quarter%2==1 {Size::new(size.height,size.width)}else{size};
    let plane=photocraft_engine::mode_cmds::rotated_size(base,recipe.geometry.as_ref().map(|g|g.angle).unwrap_or(0.0));
    if let Some(crop)=recipe.crop {
        let bounds=crop_bounds(crop,plane);Size::new(bounds[2],bounds[3])
    } else {plane}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(m:[f32;8],u:f32,v:f32)->[f32;2] {[m[0]*u+m[1]*v+m[2],m[4]*u+m[5]*v+m[6]]}
    #[test]
    fn integer_pixel_crops_do_not_gain_rounding_pixels_in_display_or_export() {
        use photocraft_engine::doc::{Document,ColorMode,SampleType,Color};
        let size=Size::new(601,400);
        let mut r=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft"})).unwrap();
        r.crop=Some(super::super::recipe::CropRect{x:123.0/601.0,y:101.0/400.0,w:17.0/601.0,h:7.0/400.0});
        assert_eq!(crop_bounds(r.crop.unwrap(),size),[123,101,17,7]);assert_eq!(canvas_size(&r,size),Size::new(17,7));
        let base=Document::with_background("Crop",size,ColorMode::Rgb,SampleType::U8,Color::rgba(0.3,0.2,0.4,1.0));
        assert_eq!(super::super::photocraft::render_document(&base,&r).unwrap().size,Size::new(17,7));
        r.crop=Some(super::super::recipe::CropRect{x:0.5,y:0.5,w:1.0/601.0,h:1.0/400.0});
        r.crop.as_mut().unwrap().x=300.0/601.0;
        assert_eq!(super::super::photocraft::render_document(&base,&r).unwrap().size,Size::new(1,1));
    }
    #[test]
    fn matches_rotation_flip_and_exact_integer_crop_boundaries() {
        let plain=super::super::recipe::parse_recipe(&serde_json::json!({"version":1})).unwrap();
        assert_eq!(mapping(&plain,Size::new(400,300)),[1.0,0.0,0.0,0.0,0.0,1.0,0.0,0.0]);
        let crop=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"rotateQuarter":1,"geometry":{"flipHorizontal":true},"crop":{"x":0.2,"y":0.25,"w":0.6,"h":0.5}})).unwrap();
        let mapped=point(mapping(&crop,Size::new(400,300)),0.0,0.0);
        assert!((mapped[0]-0.25).abs()<1e-6);assert!((mapped[1]-0.2).abs()<1e-6);
    }
    #[test]
    fn arbitrary_rotation_keeps_the_source_center_and_expands_corners() {
        let recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"geometry":{"angle":30}})).unwrap();
        let matrix=mapping(&recipe,Size::new(400,300));
        let center=point(matrix,0.5,0.5);assert!((center[0]-0.5).abs()<1e-6 && (center[1]-0.5).abs()<1e-6);
        let corner=point(matrix,0.0,0.0);assert!(corner[0]<0.0 || corner[1]<0.0);
    }
}
