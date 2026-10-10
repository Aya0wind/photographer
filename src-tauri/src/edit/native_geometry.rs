//! Inverse display sampling: reuse PhotoCraft's canvas bounds; no pixel processing.
use super::recipe::EditRecipe;
use photocraft_engine::doc::Size;

pub(super) fn mapping(recipe:&EditRecipe,size:Size)->[f32;8] {
    let quarter=recipe.rotate_quarter;
    let base=if quarter%2==1 {Size::new(size.height,size.width)} else {size};
    let g=recipe.geometry.clone().unwrap_or_default();
    let plane=photocraft_engine::mode_cmds::rotated_size(base,g.angle);
    let (x,y,w,h)=if let Some(crop)=recipe.crop {
        let x=(crop.x*f64::from(plane.width)).floor();let y=(crop.y*f64::from(plane.height)).floor();
        let right=((crop.x+crop.w)*f64::from(plane.width)).ceil().min(f64::from(plane.width));
        let bottom=((crop.y+crop.h)*f64::from(plane.height)).ceil().min(f64::from(plane.height));
        (x,y,(right-x).max(1.0),(bottom-y).max(1.0))
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
        let x=(crop.x*f64::from(plane.width)).floor();let y=(crop.y*f64::from(plane.height)).floor();
        let right=((crop.x+crop.w)*f64::from(plane.width)).ceil().min(f64::from(plane.width));
        let bottom=((crop.y+crop.h)*f64::from(plane.height)).ceil().min(f64::from(plane.height));
        Size::new((right-x).max(1.0) as u32,(bottom-y).max(1.0) as u32)
    } else {plane}
}

#[cfg(test)]
mod tests {
    use super::*;
    fn point(m:[f32;8],u:f32,v:f32)->[f32;2] {[m[0]*u+m[1]*v+m[2],m[4]*u+m[5]*v+m[6]]}
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
