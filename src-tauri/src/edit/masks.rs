//! Source-space mask coverage shared by native preview and full-resolution export.
use std::collections::HashMap;
use photocraft_engine::{Session,doc::{Document,Layer,LayerContent,LayerMask,ColorMode,SampleType,Size,BlendMode}};
use serde_json::json;
use super::{photocraft,preview_renderer::Renderer,recipe::{EditRecipe,LocalMask,MaskKind}};

struct Entry {
    shape: LocalMask,
    source_revision:u64,
    size: Size,
    normal: LayerMask,
    inverse: Option<LayerMask>,
    brush_prefix:Option<(usize,LayerMask)>,
    live_brush:Option<photocraft_engine::brush_cmds::LiveStroke>,
    renderer: Box<Renderer>,
    template: Document,
    input:Option<(Document,Document)>,
    group:Option<Layer>,
}

pub(super) struct MaskCache {entries:HashMap<String,Entry>,source_scale:f32,source:Option<Document>,source_revision:u64,pixels:Option<Vec<[f32;4]>>}
impl Default for MaskCache {fn default()->Self{Self{entries:HashMap::new(),source_scale:1.0,source:None,source_revision:0,pixels:None}}}

fn shape_key(mask:&LocalMask)->LocalMask {
    let mut shape=mask.clone();
    shape.name.clear();shape.enabled=true;shape.inverted=false;
    shape.density=100.0;shape.feather=0.0;shape.adjustments=None;shape.advanced=None;shape.overlay_color=None;
    for component in &mut shape.components{let old=&component.mask;let mut key=shape_key(old);key.enabled=old.enabled;key.inverted=old.inverted;key.density=old.density;key.feather=old.feather;component.mask=Box::new(key);}
    shape
}
/// GPU-native fill with the same upstream handle layout and ramp as coverage().
/// Reusing document/layer identities keeps the constant ramp resident while
/// dragging changes only the fill's geometry; no source raster is constructed.
pub(super) fn gradient_overlay_source(base:&Document,mask:&LocalMask,previous:Option<&Document>)->Result<Document,String>{
    use photocraft_engine::doc::{Fill,Color};
    if !mask.components.is_empty()||!matches!(mask.kind,MaskKind::Linear|MaskKind::Radial)||mask.feather!=0.0{return Err("渐变填充覆盖参数无效".into());}
    let rgb=overlay_color(mask);
    let mut session=Session::new();session.add_document(Document::new("Gradient",base.size,ColorMode::Rgb,SampleType::U8),None);
    photocraft::execute(&mut session,"gradient.fill.create",json!({"from":[mask.from.x*f64::from(base.size.width),mask.from.y*f64::from(base.size.height)],"to":[mask.to.x*f64::from(base.size.width),mask.to.y*f64::from(base.size.height)],"style":if mask.kind==MaskKind::Linear{"linear"}else{"radial"},"stops":[[0.0,"#ffffff"],[1.0,"#000000"]],"dither":false}))?;
    let doc=photocraft::active_document(&session)?;let Some(LayerContent::Fill(mut fill))=doc.layers.last().map(|l|l.content.clone())else{return Err("渐变填充未生成".into());};
    if let Fill::Gradient{style,angle,scale,offset,stops,..}=&mut fill {
        let (a,s,o)=photocraft_compose::gradient_fill::from_handles(*style,[(mask.from.x*f64::from(base.size.width))as f32,(mask.from.y*f64::from(base.size.height))as f32],[(mask.to.x*f64::from(base.size.width))as f32,(mask.to.y*f64::from(base.size.height))as f32],base.bounds(),*angle);*angle=a;*scale=s;*offset=o;
        for (_,color) in stops{let value=color.to_rgb()[0];*color=Color::rgba(rgb[0],rgb[1],rgb[2],if mask.inverted{1.0-value}else{value});}
    }
    let mut overlay=previous.filter(|d|d.size==base.size).cloned().unwrap_or_else(||{let mut d=Document::new("Mask fill overlay",base.size,ColorMode::Rgb,SampleType::U8);d.layers.push(Layer::new("Coverage",LayerContent::Fill(fill.clone())));d});
    overlay.layers[0].content=LayerContent::Fill(fill);Ok(overlay)
}

fn overlay_color(mask:&LocalMask)->[f32;3]{mask.overlay_color.as_ref().and_then(|v|super::recipe::parse_color(v)).map(|v|v.map(|c|c as f32/255.0)).unwrap_or([1.0,0.2,0.2])}
fn coverage(mask:&LocalMask,size:Size)->Result<LayerMask,String>{coverage_with_pixels(mask,size,None)}
fn coverage_with_pixels(mask:&LocalMask,size:Size,pixels:Option<&[[f32;4]]>)->Result<LayerMask,String>{
    use photocraft_algo::selection::{mask_from_surface,combine,feather};
    let mut result=primitive_coverage(mask,size,pixels)?;let area=Document::new("Coverage",size,ColorMode::Rgb,SampleType::U8).bounds();
    for component in &mask.components{
        if !component.mask.enabled{continue;}
        let shape=coverage_with_pixels(&component.mask,size,pixels)?;
        let mut values=mask_from_surface(Some(&shape.surface),area);
        if component.mask.inverted{for value in &mut values{*value=1.0-*value;}}
        if component.mask.feather>0.0{values=feather(&values,size.width as usize,size.height as usize,(component.mask.feather/100.0*f64::from(size.width.min(size.height))) as f32);}
        for value in &mut values{*value*=(component.mask.density/100.0) as f32;}
        result.surface=combine(Some(&result.surface),&values,area,component.operation).unwrap_or_else(||photocraft_algo::selection::mask_to_surface(&vec![0.0;values.len()],area));
    }
    Ok(result)
}
fn primitive_coverage(mask:&LocalMask,size:Size,pixels:Option<&[[f32;4]]>)->Result<LayerMask,String> {
    let mut session=Session::new();
    let template=Document::new("Mask",size,ColorMode::Rgb,SampleType::U8);
    let area=template.bounds();
    session.add_document(template,None);
    match mask.kind {
        MaskKind::Range=>{
            let range=mask.range.as_ref().ok_or("范围蒙版参数缺失")?;let pixels=pixels.ok_or("范围蒙版来源缺失")?;
            let values=if range.kind=="color" {let color=super::recipe::parse_color(&range.color).ok_or("范围颜色无效")?.map(|c|c as f32/255.0);photocraft_algo::selection::color_range(pixels,color,range.tolerance)}else{photocraft_algo::selection::tone_range(pixels,range.low,range.high,range.smoothness)};
            Ok(LayerMask{surface:photocraft_algo::selection::mask_to_surface(&values,area),..LayerMask::reveal_all()})
        }
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
fn paint_tail(base:&LayerMask,strokes:&[super::recipe::MaskStroke],size:Size)->Result<LayerMask,String>{
    if strokes.is_empty(){return Ok(base.clone());}
    let mut session=Session::new();session.add_document(Document::new("Brush tail",size,ColorMode::Rgb,SampleType::U8),None);
    photocraft::execute(&mut session,"layer.new.layer",json!({}))?;
    std::sync::Arc::make_mut(&mut session.active_mut().ok_or("蒙版文档不存在")?.doc).layers.last_mut().ok_or("蒙版图层不存在")?.mask=Some(base.clone());
    for stroke in strokes {
        let points:Vec<_>=stroke.points.iter().map(|p|json!([p.x*f64::from(size.width),p.y*f64::from(size.height),1.0])).collect();
        photocraft::execute(&mut session,"paint.stroke",json!({"target":"mask","points":points,"size":(stroke.width_rel*f64::from(size.width.min(size.height))).clamp(0.5,5000.0),"hardness":stroke.hardness,"opacity":stroke.opacity,"flow":stroke.flow,"spacing":0.15,"color":if stroke.erase{"#000000"}else{"#ffffff"}}))?;
        // Cached coverage owns history; the temporary engine must not retain
        // every prior document while replaying a long imported tail.
        session.journal.clear();session.active_mut().ok_or("蒙版文档不存在")?.history=Default::default();
    }
    photocraft::active_document(&session)?.layers.last().and_then(|l|l.mask.clone()).ok_or("蒙版画笔未生成覆盖区域".into())
}
fn cached_brush_coverage(mut old:Option<&mut Entry>,mask:&LocalMask,size:Size)->Result<(LayerMask,Option<(usize,LayerMask)>,Option<photocraft_engine::brush_cmds::LiveStroke>),String>{
    if mask.strokes.is_empty(){return Ok((coverage(mask,size)?,None,None));}
    let count=mask.strokes.len()-1;
    if let Some(entry)=old.as_deref_mut().filter(|e|e.size==size&&e.shape.kind==MaskKind::Brush&&e.shape.strokes.len()==mask.strokes.len()) {
        let before=&entry.shape.strokes[count];let after=&mask.strokes[count];
        if entry.shape.strokes[..count]==mask.strokes[..count]&&before.width_rel==after.width_rel&&before.hardness==after.hardness&&before.opacity==after.opacity&&before.flow==after.flow&&before.erase==after.erase&&after.points.starts_with(&before.points){
            if let Some(mut live)=entry.live_brush.take(){
                if after.points.len()>before.points.len(){
                    let added:Vec<_>=after.points[before.points.len()..].iter().map(|p|json!([p.x*f64::from(size.width),p.y*f64::from(size.height),1.0])).collect();
                    let points=photocraft_engine::brush_cmds::parse_points(&json!({"points":added}),"paint.stroke").map_err(|e|e.to_string())?;
                    live.push(&points).map_err(|e|e.to_string())?;
                }
                let normal=live.doc.layers.last().and_then(|l|l.mask.clone()).ok_or("实时画笔未生成覆盖区域")?;
                return Ok((normal,entry.brush_prefix.clone(),Some(live)));
            }
        }
    }
    let usable=old.as_deref().filter(|e|e.size==size&&e.shape.kind==MaskKind::Brush);
    let seed=usable.and_then(|entry|{
        let n=entry.shape.strokes.len();
        if n<=count&&mask.strokes[..n]==entry.shape.strokes[..]{return Some((n,&entry.normal));}
        entry.brush_prefix.as_ref().filter(|(n,_)|*n<=count&&mask.strokes[..*n]==entry.shape.strokes[..*n]).map(|(n,coverage)|(*n,coverage))
    });
    let prefix=if let Some((n,base))=seed {paint_tail(base,&mask.strokes[n..count],size)?}else{let mut prefix_mask=mask.clone();prefix_mask.strokes.truncate(count);coverage(&prefix_mask,size)?};
    let mut session=Session::new();session.add_document(Document::new("Live brush",size,ColorMode::Rgb,SampleType::U8),None);photocraft::execute(&mut session,"layer.new.layer",json!({}))?;
    std::sync::Arc::make_mut(&mut session.active_mut().ok_or("蒙版文档不存在")?.doc).layers.last_mut().ok_or("蒙版图层不存在")?.mask=Some(prefix.clone());
    let live=photocraft_engine::brush_cmds::LiveStroke::begin(&session,&brush_params(&mask.strokes[count],size)).map_err(|e|e.to_string())?;
    let normal=live.doc.layers.last().and_then(|l|l.mask.clone()).ok_or("实时画笔未生成覆盖区域")?;
    Ok((normal,Some((count,prefix)),Some(live)))
}
fn brush_params(stroke:&super::recipe::MaskStroke,size:Size)->serde_json::Value{
    let points:Vec<_>=stroke.points.iter().map(|p|json!([p.x*f64::from(size.width),p.y*f64::from(size.height),1.0])).collect();
    json!({"target":"mask","points":points,"size":(stroke.width_rel*f64::from(size.width.min(size.height))).clamp(0.5,5000.0),"hardness":stroke.hardness,"opacity":stroke.opacity,"flow":stroke.flow,"spacing":0.15,"color":if stroke.erase{"#000000"}else{"#ffffff"}})
}

fn inverted(mask:&LayerMask,_size:Size)->Result<LayerMask,String> {
    use photocraft_engine::doc::{Adjustment,Rect};
    // Apply the upstream operation to tile batches rather than going through
    // document/history and per-pixel target conversion. Invert the infinite
    // default too, and retain every stored tile including off-canvas paint.
    let value=mask.surface.default_pixel().first().copied().unwrap_or(0.0);
    let mut buffer=photocraft_compose::Buffer{rect:Rect::from_xywh(0,0,1,1),px:vec![[value,value,value,1.0]]};
    photocraft_compose::adjust::apply(&Adjustment::Invert,&mut buffer);
    let mut surface=photocraft_engine::doc::Surface::with_default(mask.surface.format(),&[buffer.px[0][0]]);
    let mut values=Vec::new();
    for (coordinate,_) in mask.surface.tiles(){
        let area=coordinate.rect();buffer.rect=area;buffer.px.resize(area.width() as usize*area.height() as usize,[0.0;4]);mask.surface.read_rgba_into(area,&mut buffer.px);
        photocraft_compose::adjust::apply(&Adjustment::Invert,&mut buffer);
        values.clear();values.extend(buffer.px.iter().map(|p|p[0]));surface.write_region(area,&values);
    }
    Ok(LayerMask{surface,..mask.clone()})
}
fn selected_coverage(entry:&mut Entry,inverse:bool)->Result<LayerMask,String>{
    if !inverse{return Ok(entry.normal.clone());}
    if entry.inverse.is_none(){entry.inverse=Some(inverted(&entry.normal,entry.size)?);}
    entry.inverse.clone().ok_or_else(||"反转蒙版未生成".into())
}

impl MaskCache {
    pub(super) fn set_source(&mut self,source:&Document){
        if self.source.as_ref().is_some_and(|old|old.id==source.id&&old.layers==source.layers&&old.size==source.size){return;}
        self.source=Some(source.clone());self.source_revision=self.source_revision.wrapping_add(1);self.pixels=None;
    }
    pub fn set_source_scale(&mut self,scale:f32){self.source_scale=scale;}
    fn entry(&mut self,base:&Document,mask:&LocalMask)->Result<&mut Entry,String> {
        let key=shape_key(mask);
        let dependent=has_range(mask);
        if dependent&&self.pixels.is_none(){self.pixels=Some(photocraft_compose::flatten(self.source.as_ref().unwrap_or(base)).px);}
        let revision=if dependent{self.source_revision}else{0};
        if !self.entries.get(&mask.id).is_some_and(|e|e.shape==key && e.size==base.size&&e.source_revision==revision) {
            let (normal,brush_prefix,live_brush)=if mask.kind==MaskKind::Brush&&mask.components.is_empty(){cached_brush_coverage(self.entries.get_mut(&mask.id).filter(|entry|entry.shape.components.is_empty()),mask,base.size)?}else{(coverage_with_pixels(mask,base.size,self.pixels.as_deref())?,None,None)};
            if let Some(entry)=self.entries.get_mut(&mask.id).filter(|e|e.size==base.size) {
                entry.shape=key;entry.source_revision=revision;entry.normal=normal;entry.inverse=None;entry.brush_prefix=brush_prefix;entry.live_brush=live_brush;
                return Ok(self.entries.get_mut(&mask.id).unwrap());
            }
            let mut template=Document::new("Local adjustments",base.size,ColorMode::Rgb,SampleType::U8);
            template.icc_profile=base.icc_profile.clone();template.resolution_dpi=base.resolution_dpi;
            let renderer=Box::new(Renderer::new(&template).with_pixel_scale(self.source_scale));
            self.entries.insert(mask.id.clone(),Entry{shape:key,source_revision:revision,size:base.size,normal,inverse:None,brush_prefix,live_brush,renderer,template,input:None,group:None});
        }
        Ok(self.entries.get_mut(&mask.id).unwrap())
    }
    pub fn overlay(&mut self,base:&Document,recipe:&EditRecipe,id:&str)->Result<Document,String> {
        use photocraft_engine::doc::Color;
        self.entries.retain(|id,_|recipe.masks.iter().any(|m|&m.id==id));
        let mask=recipe.masks.iter().find(|m|m.id==id).ok_or("所选蒙版不存在")?;
        let entry=self.entry(base,mask)?;
        let mut coverage=selected_coverage(entry,mask.inverted)?;
        coverage.feather=(mask.feather/100.0*f64::from(base.size.width.min(base.size.height))) as f32;
        let mut overlay=Document::with_background("Mask overlay",base.size,ColorMode::Rgb,SampleType::U8,{let c=overlay_color(mask);Color::rgba(c[0],c[1],c[2],1.0)});
        overlay.layers[0].mask=Some(coverage);
        let mut session=Session::new();session.add_document(overlay,None);
        photocraft::geometry(&mut session,recipe)?;
        photocraft::active_document(&session)
    }
    pub fn layers(&mut self,base:&Document,recipe:&EditRecipe)->Result<Vec<Layer>,String> {
        self.entries.retain(|id,_|recipe.masks.iter().any(|m|&m.id==id));
        let mut layers=Vec::new();
        let mut working=base.clone();
        for mask in recipe.masks.iter().filter(|m|m.enabled && m.density>0.0) {
            let entry=self.entry(base,mask)?;
            let mut local=recipe.clone();
            local.color_pipeline=None;local.lens=None;local.retouch.clear();
            local.masks.clear();local.adjustments=mask.adjustments;local.advanced=mask.advanced.clone();
            local.rotate_quarter=0;local.geometry=None;local.crop=None;local.text_layers.clear();local.brush_strokes.clear();
            let spatial=mask.advanced.as_ref().and_then(|a|a.development.as_ref()).is_some_and(|d|d.active());
            let input=if spatial {
                let same=entry.input.as_ref().is_some_and(|(old,_)|old.id==working.id&&old.size==working.size&&old.mode==working.mode&&old.depth==working.depth&&old.icc_profile==working.icc_profile&&old.layers==working.layers);
                if !same {
                    let flat=photocraft::processing_input(&working)?;
                    entry.input=Some((working.clone(),flat));
                }
                &entry.input.as_ref().unwrap().1
            } else {&entry.template};
            let doc=entry.renderer.canvas_document(input,&local)?;
            let mut coverage=selected_coverage(entry,mask.inverted)?;
            coverage.feather=(mask.feather/100.0*f64::from(base.size.width.min(base.size.height))) as f32;
            let output=if !doc.layers.is_empty() {
                let mut group=entry.group.clone().unwrap_or_else(||Layer::group("Local development",Vec::new()));
                group.content=LayerContent::Group(photocraft_engine::doc::Group{children:doc.layers,expanded:true,artboard:None});
                group.blend=if spatial {BlendMode::Normal}else{BlendMode::PassThrough};group.mask=Some(coverage.clone());group.opacity=(mask.density/100.0) as f32;
                entry.group=Some(group.clone());vec![group]
            } else {Vec::new()};
            for layer in output {
                working.layers.push(layer.clone());
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
    fn normal_local_adjustments_apply_mask_strength_once_to_the_complete_group() {
        use photocraft_engine::doc::Color;
        let base=Document::with_background("Source",Size::new(32,24),ColorMode::Rgb,SampleType::U8,Color::rgba(0.3,0.2,0.5,1.0));
        let mut r=recipe("brush");r.masks[0].inverted=true;r.masks[0].strokes.clear();r.masks[0].density=50.0;r.masks[0].adjustments.as_mut().unwrap().saturation=70.0;
        let layers=MaskCache::default().layers(&base,&r).unwrap();assert_eq!(layers[0].blend,BlendMode::PassThrough);
        let mut actual=base.clone();actual.layers.extend(layers);
        let mut local=r.clone();local.masks.clear();local.adjustments=r.masks[0].adjustments;
        let full=photocraft::adjusted_document(&base,&local).unwrap();
        let before=photocraft_compose::flatten(&base);let after=photocraft_compose::flatten(&full);let mixed=photocraft_compose::flatten(&actual);
        for ((a,b),p) in before.px.iter().zip(&after.px).zip(&mixed.px){for ch in 0..3{assert!((p[ch]-(a[ch]+b[ch])*0.5).abs()<0.001);}}
    }
    #[test]
    fn local_development_uses_lower_stack_once_and_keeps_independent_caches() {
        use photocraft_engine::doc::Color;
        let base=Document::with_background("Source",Size::new(64,48),ColorMode::Rgb,SampleType::U8,Color::rgba(0.3,0.2,0.5,1.0));
        let below=super::super::recipe::parse_recipe(&json!({"version":1,"renderer":"photocraft","adjustments":{"brightness":15,"contrast":0,"saturation":0}})).unwrap();
        let mut working=photocraft::adjusted_document(&base,&below).unwrap();
        let mut r=recipe("brush");r.masks[0].inverted=true;r.masks[0].strokes.clear();r.masks[0].adjustments=None;
        r.masks[0].density=50.0;r.masks[0].advanced=Some(serde_json::from_value(json!({"development":{"shadows":40,"clarity":20}})).unwrap());
        let mut cache=MaskCache::default();cache.set_source_scale(0.1);
        let layers=cache.layers(&working,&r).unwrap();assert_eq!(layers.len(),1);assert!(layers[0].is_group());
        let mut actual=working.clone();actual.layers.extend(layers.clone());
        let input=cache.entries["mask"].input.as_ref().unwrap().1.clone();let input_id=input.id;
        // Compare the visible group against an independently executed upstream filter.
        let mut session=Session::new();session.add_document(input.clone(),None);
        let mut local=r.clone();local.masks.clear();local.advanced=r.masks[0].advanced.clone();local.adjustments=None;
        photocraft::execute(&mut session,"filter.cameraRaw",serde_json::to_value(photocraft::camera_params(&local,0.1)).unwrap()).unwrap();
        let expected=photocraft::active_document(&session).unwrap();
        let before=photocraft_compose::flatten(&working);let after=photocraft_compose::flatten(&expected);let result=photocraft_compose::flatten(&actual);
        for ((a,b),p) in before.px.iter().zip(&after.px).zip(&result.px){for ch in 0..3{assert!((p[ch]-(a[ch]+b[ch])*0.5).abs()<0.01,"mask density must blend the complete local pipeline once");}}
        r.masks[0].advanced.as_mut().unwrap().lookup=Some(super::super::recipe::LutSelection{id:"builtin:warm".into(),amount:40.0,enabled:true});
        cache.layers(&working,&r).unwrap();assert_eq!(cache.entries["mask"].input.as_ref().unwrap().1.id,input_id);assert!(cache.entries["mask"].renderer.timings.camera_cache_hit);
        r.masks[0].strokes=recipe("brush").masks[0].strokes.clone();
        let changed=cache.layers(&working,&r).unwrap();assert_eq!(changed[0].id,layers[0].id);assert_eq!(cache.entries["mask"].input.as_ref().unwrap().1.id,input_id);assert!(cache.entries["mask"].renderer.timings.camera_cache_hit);
        assert!(changed[0].mask.as_ref().unwrap().value(32,24)<0.01);
        working.layers.last_mut().unwrap().opacity=0.3;
        cache.layers(&working,&r).unwrap();assert_ne!(cache.entries["mask"].input.as_ref().unwrap().1.id,input_id);assert!(!cache.entries["mask"].renderer.timings.camera_cache_hit);
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
    fn overlay_cache_releases_deleted_mask_shapes(){
        let base=Document::new("Source",Size::new(32,24),ColorMode::Rgb,SampleType::U8);let mut r=recipe("linear");let mut cache=MaskCache::default();
        cache.overlay(&base,&r,"mask").unwrap();assert_eq!(cache.entries.len(),1);
        r.masks[0].id="replacement".into();cache.overlay(&base,&r,"replacement").unwrap();assert_eq!(cache.entries.len(),1);assert!(!cache.entries.contains_key("mask"));
    }
    #[test]
    #[ignore="coverage stage timing; run explicitly"]
    fn gradient_coverage_stage_timing(){
        let size=Size::new(1600,1067);let mut r=recipe("linear");let mut forward=0.0;let mut inverse=0.0;
        for n in 0..5{r.masks[0].to.x=0.7+n as f64*0.01;let start=std::time::Instant::now();let normal=coverage(&r.masks[0],size).unwrap();forward+=start.elapsed().as_secs_f64();let start=std::time::Instant::now();inverted(&normal,size).unwrap();inverse+=start.elapsed().as_secs_f64();}
        eprintln!("[gradient stages] coverage {:.2} ms; inverse {:.2} ms",forward*1000.0/5.0,inverse*1000.0/5.0);
    }
    #[test]
    fn inverse_is_generated_only_when_selected_and_invalidated_after_shape_changes(){
        let base=Document::new("Source",Size::new(32,24),ColorMode::Rgb,SampleType::U8);let mut r=recipe("linear");let mut cache=MaskCache::default();
        cache.overlay(&base,&r,"mask").unwrap();assert!(cache.entries["mask"].inverse.is_none());
        r.masks[0].inverted=true;cache.overlay(&base,&r,"mask").unwrap();let first=cache.entries["mask"].inverse.clone().unwrap();cache.overlay(&base,&r,"mask").unwrap();assert_eq!(first,cache.entries["mask"].inverse.clone().unwrap());
        r.masks[0].inverted=false;r.masks[0].to.x=0.65;cache.overlay(&base,&r,"mask").unwrap();assert!(cache.entries["mask"].inverse.is_none());
    }
    #[test]
    fn batched_inverse_matches_upstream_command_including_all_levels_and_off_canvas_pixels(){
        use photocraft_engine::doc::Rect;
        let size=Size::new(256,2);let mut mask=LayerMask::reveal_all();
        mask.surface.write_region(Rect::from_xywh(0,0,256,1),&(0..256).map(|v|v as f32/255.0).collect::<Vec<_>>());mask.surface.write_pixel(-5,1,&[0.25]);
        let actual=inverted(&mask,size).unwrap();let mut session=Session::new();session.add_document(Document::new("Reference",size,ColorMode::Rgb,SampleType::U8),None);photocraft::execute(&mut session,"layer.new.layer",json!({})).unwrap();
        std::sync::Arc::make_mut(&mut session.active_mut().unwrap().doc).layers.last_mut().unwrap().mask=Some(mask);
        photocraft::execute(&mut session,"image.adjustments.invert",json!({"target":"mask"})).unwrap();let doc=photocraft::active_document(&session).unwrap();let expected=doc.layers.last().unwrap().mask.as_ref().unwrap();
        let area=Rect::from_xywh(-256,0,768,256);assert_eq!(actual.surface.read_region(area),expected.surface.read_region(area));assert_eq!(actual.surface.default_pixel(),expected.surface.default_pixel());
    }
    #[test]
    fn brush_prefix_cache_matches_full_replay_for_growth_erase_cancel_undo_and_prior_edits(){
        let base=Document::new("Source",Size::new(96,64),ColorMode::Rgb,SampleType::U8);let mut r=recipe("brush");let mut cache=MaskCache::default();
        r.masks[0].strokes[0].opacity=0.4;r.masks[0].strokes[0].flow=0.3;r.masks[0].strokes[0].hardness=0.5;
        let assert_equal=|cache:&mut MaskCache,r:&EditRecipe|{let expected=coverage(&r.masks[0],base.size).unwrap();let actual=cache.entry(&base,&r.masks[0]).unwrap().normal.clone();let area=base.bounds();assert_eq!(actual.surface.read_region(area),expected.surface.read_region(area));};
        assert_equal(&mut cache,&r);let committed=r.clone();
        let mut stroke=r.masks[0].strokes[0].clone();stroke.erase=true;stroke.points=vec![super::super::recipe::NormPoint{x:0.3,y:0.4}];r.masks[0].strokes.push(stroke);assert_equal(&mut cache,&r);
        r.masks[0].strokes[1].points.push(super::super::recipe::NormPoint{x:0.6,y:0.5});assert_equal(&mut cache,&r);
        r=committed.clone();assert_equal(&mut cache,&r);r.masks[0].strokes.push(committed.masks[0].strokes[0].clone());assert_equal(&mut cache,&r);
        r.masks[0].strokes[0].opacity=0.8;assert_equal(&mut cache,&r);r.masks[0].strokes.pop();assert_equal(&mut cache,&r);r.masks[0].strokes.clear();assert_equal(&mut cache,&r);
    }
    #[test]
    fn live_brush_growth_matches_complete_stroke_across_batches_and_direction_changes(){
        let base=Document::new("Source",Size::new(160,120),ColorMode::Rgb,SampleType::U8);let mut r=recipe("brush");let mut cache=MaskCache::default();
        let stroke=&mut r.masks[0].strokes[0];stroke.points=vec![super::super::recipe::NormPoint{x:0.2,y:0.4}];stroke.opacity=0.35;stroke.flow=0.4;stroke.hardness=0.6;
        for (x,y) in [(0.2,0.4),(0.4,0.5),(0.6,0.3),(0.4,0.6),(0.3,0.7)]{
            if r.masks[0].strokes[0].points.last().unwrap().x!=x{r.masks[0].strokes[0].points.push(super::super::recipe::NormPoint{x,y});}
            let actual=cache.entry(&base,&r.masks[0]).unwrap().normal.clone();let expected=coverage(&r.masks[0],base.size).unwrap();assert_eq!(actual.surface.read_region(base.bounds()),expected.surface.read_region(base.bounds()));
        }
    }
    #[test]
    #[ignore="live brush append timing; run explicitly"]
    fn live_brush_append_timing(){
        let base=Document::new("Source",Size::new(1600,1067),ColorMode::Rgb,SampleType::U8);let mut r=recipe("brush");r.masks[0].strokes[0].points=(0..30).map(|n|super::super::recipe::NormPoint{x:0.1+n as f64*0.015,y:0.5+(n as f64*0.1).sin()*0.02}).collect();r.masks[0].strokes[0].opacity=0.4;r.masks[0].strokes[0].flow=0.3;
        let mut cache=MaskCache::default();cache.entry(&base,&r.masks[0]).unwrap();let mut cached=0.0;let mut replay=0.0;
        for n in 30..38{r.masks[0].strokes[0].points.push(super::super::recipe::NormPoint{x:0.1+n as f64*0.015,y:0.5+(n as f64*0.1).sin()*0.02});let start=std::time::Instant::now();cache.entry(&base,&r.masks[0]).unwrap();cached+=start.elapsed().as_secs_f64();let start=std::time::Instant::now();coverage(&r.masks[0],base.size).unwrap();replay+=start.elapsed().as_secs_f64();}
        eprintln!("[live brush] append {:.2} ms; replay {:.2} ms",cached*1000.0/8.0,replay*1000.0/8.0);
    }
    #[test]
    #[ignore="brush history replay timing; run explicitly"]
    fn brush_prefix_cache_timing(){
        let base=Document::new("Source",Size::new(1600,1067),ColorMode::Rgb,SampleType::U8);let mut r=recipe("brush");let stroke=r.masks[0].strokes[0].clone();r.masks[0].strokes=(0..20).map(|n|{let mut s=stroke.clone();s.opacity=0.3;s.flow=0.4;s.points=vec![super::super::recipe::NormPoint{x:0.1,y:0.1+n as f64*0.03},super::super::recipe::NormPoint{x:0.8,y:0.1+n as f64*0.03}];s}).collect();
        let mut cache=MaskCache::default();cache.entry(&base,&r.masks[0]).unwrap();let mut cached=0.0;let mut replay=0.0;
        for n in 0..5{r.masks[0].strokes.last_mut().unwrap().points[1].x=0.81+n as f64*0.01;let start=std::time::Instant::now();cache.entry(&base,&r.masks[0]).unwrap();cached+=start.elapsed().as_secs_f64();let start=std::time::Instant::now();coverage(&r.masks[0],base.size).unwrap();replay+=start.elapsed().as_secs_f64();}
        eprintln!("[brush prefix] 20 strokes, cached {:.2} ms; full replay {:.2} ms",cached*1000.0/5.0,replay*1000.0/5.0);
    }
    #[test]
    fn native_gradient_alpha_matches_committed_coverage_and_retains_layer_identity(){
        let base=Document::new("Source",Size::new(64,48),ColorMode::Rgb,SampleType::U8);
        for kind in ["linear","radial"]{for inverse in [false,true]{let mut r=recipe(kind);r.masks[0].inverted=inverse;r.masks[0].feather=0.0;
            let mut cache=MaskCache::default();let expected=photocraft_compose::flatten(&cache.overlay(&base,&r,"mask").unwrap());
            let doc=gradient_overlay_source(&base,&r.masks[0],None).unwrap();let actual=photocraft_compose::flatten(&doc);
            let max=actual.px.iter().zip(expected.px).map(|(a,b)|(a[3]-b[3]).abs()).fold(0.0,f32::max);assert!(max<=1.0/255.0+0.0001,"alpha parity {kind} {inverse}: {max}");
            r.masks[0].to.x=0.65;let next=gradient_overlay_source(&base,&r.masks[0],Some(&doc)).unwrap();assert_eq!(doc.id,next.id);assert_eq!(doc.layers[0].id,next.layers[0].id);assert_ne!(doc.layers[0].content,next.layers[0].content);
        }}
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

pub(super) fn has_range(mask:&LocalMask)->bool{mask.kind==MaskKind::Range||mask.components.iter().any(|c|has_range(&c.mask))}
pub(super) fn normalize_range(mask:&mut LocalMask)->Result<(),String>{
 if mask.kind==MaskKind::Range&&mask.range.is_none(){return Err("范围蒙版参数缺失".into());}
 let Some(range)=&mut mask.range else{return Ok(());};
 if !["color","luminance"].contains(&range.kind.as_str())||super::recipe::parse_color(&range.color).is_none(){return Err("范围蒙版类型或颜色无效".into());}
 for (value,max) in [(&mut range.tolerance,200.0),(&mut range.low,255.0),(&mut range.high,255.0),(&mut range.smoothness,255.0)]{if !value.is_finite(){return Err("范围蒙版数值无效".into());}*value=value.clamp(0.0,max);}
 if range.low>range.high{return Err("亮度范围最小值不能超过最大值".into());}Ok(())
}
