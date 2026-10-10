//! Source-space mask coverage shared by native preview and full-resolution export.
use std::collections::HashMap;
use photocraft_engine::{Session,doc::{Document,Layer,LayerContent,LayerMask,ColorMode,SampleType,Size}};
use serde_json::json;
use super::{photocraft,preview_renderer::Renderer,recipe::{EditRecipe,LocalMask,MaskKind}};

struct Entry {
    shape: LocalMask,
    size: Size,
    normal: LayerMask,
    inverse: LayerMask,
    renderer: Box<Renderer>,
    template: Document,
}

#[derive(Default)]
pub(super) struct MaskCache {entries:HashMap<String,Entry>}

fn shape_key(mask:&LocalMask)->LocalMask {
    let mut shape=mask.clone();
    shape.name.clear();shape.enabled=true;shape.inverted=false;
    shape.density=100.0;shape.feather=0.0;shape.adjustments=None;shape.advanced=None;
    shape
}

fn coverage(mask:&LocalMask,size:Size)->Result<LayerMask,String> {
    let mut session=Session::new();
    let template=Document::new("Mask",size,ColorMode::Rgb,SampleType::U8);
    let area=template.bounds();
    session.add_document(template,None);
    match mask.kind {
        MaskKind::Brush=>{
            photocraft::execute(&mut session,"layer.new.layer",json!({"name":"Coverage"}))?;
            photocraft::execute(&mut session,"layer.layerMask.hideAll",json!({}))?;
            for stroke in &mask.strokes {
                let points:Vec<_>=stroke.points.iter().map(|p|json!([p.x*f64::from(size.width),p.y*f64::from(size.height),1.0])).collect();
                photocraft::execute(&mut session,"paint.stroke",json!({"target":"mask","points":points,"size":(stroke.width_rel*f64::from(size.width.min(size.height))).clamp(0.5,5000.0),"hardness":stroke.hardness,"opacity":stroke.opacity,"flow":stroke.flow,"spacing":0.15,"color":if stroke.erase {"#000000"} else {"#ffffff"}}))?;
            }
            let doc=photocraft::active_document(&session)?;
            doc.layers.last().and_then(|l|l.mask.clone()).ok_or("蒙版画笔未生成覆盖区域".into())
        }
        MaskKind::Linear|MaskKind::Radial=>{
            photocraft::execute(&mut session,"gradient.fill.create",json!({
                "from":[mask.from.x*f64::from(size.width),mask.from.y*f64::from(size.height)],
                "to":[mask.to.x*f64::from(size.width),mask.to.y*f64::from(size.height)],
                "style":if mask.kind==MaskKind::Linear {"linear"} else {"radial"},
                "stops":[[0.0,"#ffffff"],[1.0,"#000000"]],"dither":false
            }))?;
            let doc=photocraft::active_document(&session)?;
            let Some(LayerContent::Fill(fill))=doc.layers.last().map(|l|&l.content) else {return Err("渐变蒙版未生成".into());};
            let mut fill=fill.clone();
            if let photocraft_engine::doc::Fill::Gradient{style,angle,scale,offset,..}=&mut fill {
                // Mask handles use their full range; the photographic gradient
                // tool's 10–150% UI clamp must not displace a saved small mask.
                let (a,s,o)=photocraft_compose::gradient_fill::from_handles(*style,
                    [(mask.from.x*f64::from(size.width)) as f32,(mask.from.y*f64::from(size.height)) as f32],
                    [(mask.to.x*f64::from(size.width)) as f32,(mask.to.y*f64::from(size.height)) as f32],area,*angle);
                *angle=a;*scale=s;*offset=o;
            }
            let pixels=photocraft_compose::gradient_fill::render(&fill,area,area);
            let values:Vec<_>=pixels.iter().map(|p|p[0]).collect();
            Ok(LayerMask{surface:photocraft_algo::selection::mask_to_surface(&values,area),..LayerMask::reveal_all()})
        }
    }
}

fn inverted(mask:&LayerMask,size:Size)->Result<LayerMask,String> {
    let mut session=Session::new();
    session.add_document(Document::new("Inverse mask",size,ColorMode::Rgb,SampleType::U8),None);
    photocraft::execute(&mut session,"layer.new.layer",json!({}))?;
    let state=session.active_mut().ok_or("蒙版文档不存在")?;
    std::sync::Arc::make_mut(&mut state.doc).layers.last_mut().unwrap().mask=Some(mask.clone());
    photocraft::execute(&mut session,"image.adjustments.invert",json!({"target":"mask"}))?;
    photocraft::active_document(&session)?.layers.last().and_then(|l|l.mask.clone()).ok_or("反转蒙版未生成".into())
}

impl MaskCache {
    fn entry(&mut self,base:&Document,mask:&LocalMask)->Result<&mut Entry,String> {
        let key=shape_key(mask);
        if !self.entries.get(&mask.id).is_some_and(|e|e.shape==key && e.size==base.size) {
            let normal=coverage(mask,base.size)?;
            let inverse=inverted(&normal,base.size)?;
            let mut template=Document::new("Local adjustments",base.size,ColorMode::Rgb,SampleType::U8);
            template.icc_profile=base.icc_profile.clone();template.resolution_dpi=base.resolution_dpi;
            let renderer=Box::new(Renderer::new(&template));
            self.entries.insert(mask.id.clone(),Entry{shape:key,size:base.size,normal,inverse,renderer,template});
        }
        Ok(self.entries.get_mut(&mask.id).unwrap())
    }
    pub fn overlay(&mut self,base:&Document,recipe:&EditRecipe,id:&str)->Result<Document,String> {
        use photocraft_engine::doc::Color;
        let mask=recipe.masks.iter().find(|m|m.id==id).ok_or("所选蒙版不存在")?;
        let entry=self.entry(base,mask)?;
        let mut coverage=if mask.inverted {entry.inverse.clone()} else {entry.normal.clone()};
        coverage.feather=(mask.feather/100.0*f64::from(base.size.width.min(base.size.height))) as f32;
        let mut overlay=Document::with_background("Mask overlay",base.size,ColorMode::Rgb,SampleType::U8,Color::rgba(1.0,0.2,0.2,1.0));
        overlay.layers[0].mask=Some(coverage);
        let mut session=Session::new();session.add_document(overlay,None);
        photocraft::geometry(&mut session,recipe)?;
        photocraft::active_document(&session)
    }
    pub fn layers(&mut self,base:&Document,recipe:&EditRecipe)->Result<Vec<Layer>,String> {
        self.entries.retain(|id,_|recipe.masks.iter().any(|m|&m.id==id));
        let mut layers=Vec::new();
        for mask in recipe.masks.iter().filter(|m|m.enabled && m.density>0.0) {
            let entry=self.entry(base,mask)?;
            let mut local=recipe.clone();
            local.legacy_adjustments=None;
            local.masks.clear();local.adjustments=mask.adjustments;local.advanced=mask.advanced.clone();
            local.rotate_quarter=0;local.geometry=None;local.crop=None;local.text_layers.clear();local.brush_strokes.clear();
            let doc=entry.renderer.canvas_document(&entry.template,&local)?;
            let mut coverage=if mask.inverted {entry.inverse.clone()} else {entry.normal.clone()};
            coverage.feather=(mask.feather/100.0*f64::from(base.size.width.min(base.size.height))) as f32;
            for mut layer in doc.layers {
                layer.mask=Some(coverage.clone());
                // Density attenuates the adjustment, never reveals the inverse area.
                layer.opacity*= (mask.density/100.0) as f32;
                layers.push(layer);
            }
        }
        Ok(layers)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn recipe(kind:&str)->EditRecipe {
        super::super::recipe::parse_recipe(&json!({"version":1,"renderer":"photocraft","masks":[{
            "id":"mask","name":"Local light","kind":kind,"enabled":true,"inverted":false,"density":100,"feather":0,
            "from":{"x":0.2,"y":0.5},"to":{"x":0.8,"y":0.5},
            "strokes":[{"points":[{"x":0.3,"y":0.5},{"x":0.7,"y":0.5}],"widthRel":0.2,"hardness":1,"opacity":1,"flow":1,"erase":false}],
            "adjustments":{"brightness":40,"contrast":0,"saturation":0}
        }]})).unwrap()
    }
    #[test]
    fn brush_and_gradient_coverage_inverse_and_empty_brush_are_native_masks() {
        for kind in ["brush","linear","radial"] {
            let r=recipe(kind);let size=Size::new(64,48);
            let normal=coverage(&r.masks[0],size).unwrap();
            let inverse=inverted(&normal,size).unwrap();
            for (x,y) in [(0,0),(32,24),(63,47)] {
                assert!((normal.value(x,y)+inverse.value(x,y)-1.0).abs()<0.01);
            }
            if kind=="brush" {assert!(normal.value(32,24)>0.9);assert!(normal.value(0,0)<0.01);}
        }
        let mut r=recipe("brush");r.masks[0].strokes.clear();
        let normal=coverage(&r.masks[0],Size::new(64,48)).unwrap();
        assert_eq!(normal.value(32,24),0.0);
    }
    #[test]
    fn local_adjustments_reuse_coverage_and_layer_ids_and_match_export() {
        use photocraft_engine::doc::Color;
        let base=Document::with_background("Source",Size::new(64,48),ColorMode::Rgb,SampleType::U8,Color::rgba(0.2,0.3,0.4,1.0));
        let mut recipe=recipe("linear");let mut cache=MaskCache::default();
        let first=cache.layers(&base,&recipe).unwrap();
        let coverage_id=cache.entries["mask"].normal.surface.clone();
        recipe.masks[0].adjustments.as_mut().unwrap().brightness=60.0;recipe.masks[0].name="Renamed".into();
        let next=cache.layers(&base,&recipe).unwrap();
        assert_eq!(first[0].id,next[0].id);
        assert_eq!(coverage_id,cache.entries["mask"].normal.surface);
        let mut actual=base.clone();actual.layers.extend(next);
        let expected=photocraft::adjusted_document(&base,&recipe).unwrap();
        assert_eq!(photocraft_compose::flatten(&actual).px,photocraft_compose::flatten(&expected).px);
        recipe.masks[0].enabled=false;assert!(cache.layers(&base,&recipe).unwrap().is_empty());
        recipe.masks[0].enabled=true;recipe.masks[0].density=0.0;
        assert!(cache.layers(&base,&recipe).unwrap().is_empty(),"zero density must not apply to the whole image");
    }
    #[test]
    fn overlapping_masks_survive_save_and_full_size_export_without_rewriting_source() {
        use photocraft_engine::doc::Color;
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("original.png");
        image::RgbImage::from_pixel(2400,1600,image::Rgb([50,80,120])).save(&path).unwrap();
        let original=std::fs::read(&path).unwrap();
        let mut r=recipe("radial");r.masks[0].from=super::super::recipe::NormPoint{x:0.5,y:0.5};
        r.masks[0].to=super::super::recipe::NormPoint{x:0.8,y:0.5};
        let mut second=r.masks[0].clone();second.id="second".into();second.kind=MaskKind::Linear;
        second.density=50.0;second.inverted=true;second.adjustments=Some(super::super::recipe::Adjustments{brightness:-20.0,contrast:0.0,saturation:0.0});
        second.from=super::super::recipe::NormPoint{x:0.2,y:0.5};second.to=super::super::recipe::NormPoint{x:0.8,y:0.5};r.masks.push(second);
        let saved=serde_json::to_vec(&r).unwrap();std::fs::write(dir.path().join("recipe.json"),&saved).unwrap();
        let restored=super::super::recipe::parse_recipe(&serde_json::from_slice(&std::fs::read(dir.path().join("recipe.json")).unwrap()).unwrap()).unwrap();
        assert_eq!(restored,r);
        let proxy=Document::with_background("Proxy",Size::new(600,400),ColorMode::Rgb,SampleType::U8,Color::rgba(50.0/255.0,80.0/255.0,120.0/255.0,1.0));
        let displayed=Renderer::new(&proxy).canvas_document(&proxy,&restored).unwrap();
        let (jpeg,w,h)=photocraft::export_native(&path,&restored,None,100).unwrap();
        assert_eq!((w,h),(2400,1600));let output=image::load_from_memory(&jpeg).unwrap().to_rgb8();
        for (x,y) in [(0.1,0.2),(0.5,0.5),(0.8,0.7)] {
            let px=(x*600.0) as i32;let py=(y*400.0) as i32;
            let sample=photocraft_compose::render(&displayed,photocraft_engine::doc::Rect::from_xywh(px,py,1,1)).px[0];
            let actual=output.get_pixel((x*2400.0) as u32,(y*1600.0) as u32);
            for ch in 0..3 {assert!((sample[ch]*255.0-f32::from(actual[ch])).abs()<4.0,"proxy/full export mismatch at {x},{y}, channel {ch}");}
        }
        assert_ne!(*output.get_pixel(1200,800),image::Rgb([50,80,120]));
        assert_eq!(std::fs::read(&path).unwrap(),original);
    }
    #[test]
    fn overlay_retains_transparency_and_uses_the_export_geometry() {
        let base=Document::new("Source",Size::new(64,48),ColorMode::Rgb,SampleType::U8);
        let mut r=recipe("brush");let mut cache=MaskCache::default();
        let overlay=cache.overlay(&base,&r,"mask").unwrap();
        let flat=photocraft_compose::flatten(&overlay);
        assert!(flat.px[24*64+32][3]>0.9);assert!(flat.px[0][3]<0.01);
        r.rotate_quarter=1;r.geometry=Some(super::super::recipe::Geometry{angle:30.0,..Default::default()});
        r.crop=Some(super::super::recipe::CropRect{x:0.1,y:0.1,w:0.8,h:0.8});
        let changed=cache.overlay(&base,&r,"mask").unwrap();
        assert_eq!(changed.size,super::super::native_geometry::canvas_size(&r,base.size));
        let flat=photocraft_compose::flatten(&changed);
        assert!(flat.px.iter().any(|p|p[3]<0.01));assert!(flat.px.iter().any(|p|p[3]>0.9));
    }
    #[test]
    fn rejects_duplicate_ids_and_empty_strokes_and_preserves_source_coordinates() {
        let r=recipe("brush");let mut v=serde_json::to_value(&r).unwrap();
        let duplicate=v["masks"][0].clone();v["masks"].as_array_mut().unwrap().push(duplicate);
        assert!(super::super::recipe::parse_recipe(&v).is_err());
        let mut v=serde_json::to_value(&r).unwrap();v["masks"][0]["strokes"][0]["points"]=json!([]);
        assert!(super::super::recipe::parse_recipe(&v).is_err());
        let mut v=serde_json::to_value(&r).unwrap();v["rotateQuarter"]=json!(1);v["geometry"]=json!({"angle":30});
        assert_eq!(super::super::recipe::parse_recipe(&v).unwrap().masks[0].from,r.masks[0].from);
    }
}
