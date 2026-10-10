//! 临时原生会话的增量预览。UI 配方仍是持久化/撤销真值；不保留预览命令历史。
use std::{sync::Arc, time::Instant};

use photocraft_engine::{
    doc::{BlendMode, DocId, Document, Layer, LayerContent, LayerMask},
    Session,
};
use serde::Serialize;
use serde_json::Value;

use super::{photocraft, recipe::EditRecipe};

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    pub frames: u64,
    pub cached_frames: u64,
    pub frame_cache_hit: bool,
    pub camera_cache_hit: bool,
    pub annotation_cache_hit: bool,
    pub camera_ms: f64,
    pub adjustments_ms: f64,
    pub geometry_ms: f64,
    pub annotations_ms: f64,
    pub raw_ms: f64,
    pub composite_ms: f64,
    pub profile_ms: f64,
    pub gpu: bool,
    pub encode_ms: f64,
    pub total_ms: f64,
}

#[cfg(test)]
mod native_tests {
    use super::*;
    #[test]
    fn extended_color_layers_match_upstream_and_keep_slider_identity() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        use serde_json::json;
        let base=Document::with_background("Color",Size::new(32,24),ColorMode::Rgb,SampleType::U8,Color::rgba(0.7,0.2,0.4,1.0));
        let options=[
            ("colorBalance",json!({"shadows":[10,0,0],"midtones":[25,-10,5],"highlights":[0,0,-20],"preserveLuminosity":true}),"colorBalance",json!({"shadows":[10,0,0],"midtones":[25,-10,5],"highlights":[0,0,-20],"preserveLuminosity":true})),
            ("blackWhite",json!({"enabled":true,"weights":[40,60,40,60,20,80],"tint":"#E1D3B3"}),"blackWhite",json!({"tint":true,"tintColor":"#E1D3B3"})),
            ("selectiveColor",json!({"relative":false,"ranges":{"reds":[20,-10,30,0]}}),"selectiveColor",json!({"method":"absolute","reds":[20,-10,30,0]}))
        ];
        for (key,value,kind,params) in options {
            let mut body=json!({"version":1,"renderer":"photocraft","advanced":{}});body["advanced"][key]=value;
            let recipe=super::super::recipe::parse_recipe(&body).unwrap();let mut renderer=Renderer::new(&base);
            let native=renderer.canvas_document(&base,&recipe).unwrap();
            let mut session=Session::new();session.add_document(base.clone(),None);
            photocraft::execute(&mut session,&format!("layer.newAdjustmentLayer.{kind}"),params).unwrap();
            let expected=photocraft::active_document(&session).unwrap();
            assert_eq!(photocraft_compose::flatten(&native).px,photocraft_compose::flatten(&expected).px,"{key}");
            assert_ne!(photocraft_compose::flatten(&native).px,photocraft_compose::flatten(&base).px,"{key} must affect actual pixels");
            let saved=serde_json::to_value(&recipe).unwrap();let restored=super::super::recipe::parse_recipe(&saved).unwrap();
            let export=photocraft::render_document(&base,&restored).unwrap();
            assert_eq!(photocraft_compose::flatten(&native).px,photocraft_compose::flatten(&export).px);
            let mut updated=recipe.clone();
            match key {
                "colorBalance"=>updated.advanced.as_mut().unwrap().color_balance.as_mut().unwrap().midtones[0]=40.0,
                "blackWhite"=>updated.advanced.as_mut().unwrap().black_white.as_mut().unwrap().weights[0]=70.0,
                _=>updated.advanced.as_mut().unwrap().selective_color.as_mut().unwrap().ranges.get_mut("reds").unwrap()[0]=40.0
            }
            let next=renderer.canvas_document(&base,&updated).unwrap();assert_eq!(native.layers.last().unwrap().id,next.layers.last().unwrap().id);
        }
    }
    #[test]
    fn warmup_keeps_original_neutral_and_reuses_slider_layers() {
        use photocraft_engine::doc::{Color, ColorMode, SampleType, Size};
        let base = Document::with_background(
            "Warmup",
            Size::new(32, 16),
            ColorMode::Rgb,
            SampleType::U8,
            Color::rgba(0.2, 0.5, 0.7, 1.0),
        );
        let mut renderer = Renderer::new(&base);
        let warm = renderer.warm_canvas_document(&base).unwrap();
        let neutral = super::super::recipe::parse_recipe(
            &serde_json::json!({"version":1,"renderer":"photocraft"}),
        )
        .unwrap();
        let restored = renderer.canvas_document(&base, &neutral).unwrap();
        assert_eq!(restored.layers.len(), base.layers.len());
        assert_eq!(
            photocraft_compose::flatten(&restored).px,
            photocraft_compose::flatten(&base).px
        );
        let exposure = super::super::recipe::parse_recipe(
            &serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"exposure":0.3}}),
        )
        .unwrap();
        let changed = renderer.canvas_document(&base, &exposure).unwrap();
        assert_eq!(warm.layers[1].id, changed.layers[1].id);
    }
    #[test]
    fn annotation_overlay_uses_post_geometry_bounds_and_reuses_layers_during_color_edits() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("Photo",Size::new(80,60),ColorMode::Rgb,SampleType::U8,Color::rgba(0.2,0.3,0.4,1.0));
        let mut recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","geometry":{"angle":30},"crop":{"x":0.1,"y":0.1,"w":0.8,"h":0.8},"brushStrokes":[{"id":"stroke","color":"#ff0000","widthRel":0.1,"points":[{"x":0.2,"y":0.3},{"x":0.7,"y":0.6}]}]})).unwrap();
        let mut renderer=Renderer::new(&base);
        let first=renderer.canvas_annotations(&base,&recipe).unwrap().unwrap();
        assert_eq!(first.size,super::super::native_geometry::canvas_size(&recipe,base.size));
        assert_eq!(first.layers.len(),1);
        let expected=photocraft::render_document(&base,&recipe).unwrap();
        let mut photo_recipe=recipe.clone();photo_recipe.text_layers.clear();photo_recipe.brush_strokes.clear();
        let mut split=photocraft::render_document(&base,&photo_recipe).unwrap();split.layers.extend(first.layers.clone());
        assert_eq!(photocraft_compose::flatten(&split).px,photocraft_compose::flatten(&expected).px,"post-geometry annotation composition must match export");
        recipe.advanced=Some(serde_json::from_value(serde_json::json!({"exposure":1})).unwrap());
        let next=renderer.canvas_annotations(&base,&recipe).unwrap().unwrap();
        assert_eq!(next.id,first.id);assert_eq!(next.layers[0].id,first.layers[0].id);
        assert_eq!(photocraft_compose::flatten(&next).px,photocraft_compose::flatten(&first).px);
    }
    #[test]
    fn lut_strength_reuses_layer_and_matches_export_adjustments() {
        use photocraft_engine::doc::{Color,ColorMode,SampleType,Size};
        let base=Document::with_background("LUT",Size::new(8,8),ColorMode::Rgb,SampleType::U8,Color::rgba(0.4,0.5,0.6,1.0));
        let mut recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","advanced":{"lookup":{"id":"builtin:warm","amount":100,"enabled":true}}})).unwrap();
        let mut renderer=Renderer::new(&base);
        let full=renderer.canvas_document(&base,&recipe).unwrap();let id=full.layers.last().unwrap().id;
        recipe.advanced.as_mut().unwrap().lookup.as_mut().unwrap().amount=50.0;
        let half=renderer.canvas_document(&base,&recipe).unwrap();
        assert_eq!(half.layers.last().unwrap().id,id);assert_eq!(half.layers.last().unwrap().opacity,0.5);
        let exported=photocraft::adjusted_document(&base,&recipe).unwrap();
        assert_eq!(photocraft_compose::flatten(&half).px,photocraft_compose::flatten(&exported).px);
        recipe.advanced.as_mut().unwrap().lookup.as_mut().unwrap().enabled=false;
        let disabled=renderer.canvas_document(&base,&recipe).unwrap();
        assert_eq!(photocraft_compose::flatten(&disabled).px,photocraft_compose::flatten(&base).px);
    }
    #[test]
    fn camera_preview_curves_match_upstream_filter_and_preserve_source_tiles() {
        let image = photocraft_codecs::Image::from_u8(
            32,
            16,
            photocraft_codecs::ChannelLayout::Rgb,
            (0..32 * 16 * 3)
                .map(|i| ((i * 73 + 17) % 256) as u8)
                .collect(),
        )
        .unwrap();
        let base = crate::edit::source::from_image("Parity", &image).unwrap();
        let mut recipe = super::super::recipe::parse_recipe(
            &serde_json::json!({"version":1,"renderer":"photocraft"}),
        )
        .unwrap();
        recipe.advanced = Some(
            serde_json::from_value(
                serde_json::json!({"exposure":1.2,"temperature":24.0,"tint":-15.0}),
            )
            .unwrap(),
        );
        let expected = photocraft::render_document(&base, &recipe).unwrap();
        let mut renderer = Renderer::new(&base);
        let native = renderer.canvas_document(&base, &recipe).unwrap();
        assert_eq!(native.layers[0].id, base.layers[0].id);
        let expected = photocraft_compose::flatten(&expected);
        let actual = photocraft_compose::flatten(&native);
        let maximum = actual
            .px
            .iter()
            .zip(expected.px.iter())
            .flat_map(|(a, b)| (0..3).map(move |ch| (a[ch] - b[ch]).abs()))
            .fold(0.0f32, f32::max);
        assert!(maximum < 0.02, "channel LUT parity error: {maximum}");
        let id = native.layers[1].id;
        recipe.advanced.as_mut().unwrap().exposure = 0.5;
        let next = renderer.canvas_document(&base, &recipe).unwrap();
        assert_eq!(
            next.layers[1].id, id,
            "adjustment identity must remain stable for GPU caching"
        );
        assert_eq!(next.layers[0].id, base.layers[0].id);
    }
}

struct AdjustmentLayer {
    key: &'static str,
    params: Value,
    layer: Layer,
}

struct AnnotationCache {
    template: Document,
    layers: Vec<Layer>,
}

pub struct Renderer {
    masks: super::masks::MaskCache,
    session: Session,
    camera: Option<(DocId, photocraft_algo::camera_raw::CameraRaw, Document)>,
    source_scale:f32,
    canvas_camera: Option<([f64; 3], Layer)>,
    adjustments: Vec<AdjustmentLayer>,
    geometry: Option<Document>,
    annotations: Option<AnnotationCache>,
    overlay_template:Option<Document>,
    previous: Option<(DocId, EditRecipe)>,
    jpeg: Vec<u8>,
    pub timings: Timings,
}

impl Renderer {
    pub(super) fn with_source_size(mut self,original:[u32;2],base:&Document)->Self {
        self.source_scale=(base.size.width.max(base.size.height) as f32 / original[0].max(original[1]).max(1) as f32).min(1.0);self
    }
    pub(super) fn mask_overlay(&mut self,base:&Document,recipe:&EditRecipe,id:&str)->Result<Document,String> {
        self.masks.overlay(base,recipe,id)
    }
    pub fn new(base: &Document) -> Self {
        let mut session = Session::new();
        session.add_document(base.clone(), None);
        Self {
            masks:Default::default(),
            session,
            camera: None,
            source_scale:1.0,
            canvas_camera: None,
            adjustments: Vec::new(),
            geometry: None,
            annotations: None,
            overlay_template:None,
            previous: None,
            jpeg: Vec::new(),
            timings: Timings::default(),
        }
    }

    /// 更新预览状态不产生第二套 undo/journal，也不让历史快照无限保留代理瓦片。
    fn set_document(&mut self, doc: Document) -> Result<(), String> {
        self.session.journal.clear();
        let state = self.session.active_mut().ok_or("预览文档不存在")?;
        state.active_layer = doc.top_layer();
        state.selected_layers = state.active_layer.into_iter().collect();
        state.layer_anchor = state.active_layer;
        state.history = Default::default();
        state.doc = Arc::new(doc);
        state.revision = state.revision.wrapping_add(1);
        state.last_damage = None;
        Ok(())
    }

    fn camera_document(
        &mut self,
        base: &Document,
        recipe: &EditRecipe,
    ) -> Result<Document, String> {
        self.timings.camera_cache_hit=false;
        let tuning = photocraft::camera_params(recipe,self.source_scale);
        if let Some((id, key, doc)) = &self.camera {
            if *id == base.id && *key == tuning {
                self.timings.camera_cache_hit = true;
                return Ok(doc.clone());
            }
        }
        self.set_document(base.clone())?;
        photocraft::camera_adjustment_scaled(&mut self.session, recipe,self.source_scale)?;
        let document = photocraft::active_document(&self.session)?;
        self.camera = Some((base.id, tuning, document.clone()));
        Ok(document)
    }

    fn update_adjustments(
        &mut self,
        mut base: Document,
        recipe: &EditRecipe,
    ) -> Result<(), String> {
        for spec in photocraft::adjustment_specs(recipe)? {
            let cached = self
                .adjustments
                .iter_mut()
                .find(|layer| layer.key == spec.key);
            let layer = match cached {
                Some(cached) => {
                    if cached.params != spec.params {
                        let adjustment = if let Some(native)=spec.native.clone(){native}else{photocraft_engine::adjust_params::from_params(
                            spec.kind,
                            &spec.params,
                            None,
                            base.mode,
                        )
                        .map_err(|e| e.to_string())?};
                        cached.layer.content = LayerContent::Adjustment(adjustment);
                        cached.params = spec.params;
                    }
                    cached.layer.clone()
                }
                None => {
                    let adjustment = if let Some(native)=spec.native.clone(){native}else{photocraft_engine::adjust_params::from_params(
                        spec.kind,
                        &spec.params,
                        None,
                        base.mode,
                    )
                    .map_err(|e| e.to_string())?};
                    let mut layer = Layer::new(spec.key, LayerContent::Adjustment(adjustment));
                    if spec.luminosity {
                        layer.blend = BlendMode::Luminosity;
                    }
                    self.adjustments.push(AdjustmentLayer {
                        key: spec.key,
                        params: spec.params,
                        layer: layer.clone(),
                    });
                    layer
                }
            };
            let mut layer = layer;
            layer.opacity=spec.opacity;
            layer.mask = base.selection.as_ref().map(|surface| LayerMask {
                surface: surface.clone(),
                ..LayerMask::reveal_all()
            });
            base.layers.push(layer);
        }
        base.layers.extend(self.masks.layers(&base,recipe)?);
        self.set_document(base)
    }

    fn geometry_document(&self, recipe: &EditRecipe) -> Result<Document, String> {
        let doc = photocraft::active_document(&self.session)?;
        if recipe.rotate_quarter == 0 && recipe.crop.is_none() && recipe.geometry.is_none() {
            return Ok(doc);
        }
        let mut session = Session::new();
        session.add_document(doc, None);
        photocraft::geometry(&mut session, recipe)?;
        photocraft::active_document(&session)
    }

    fn annotation_layers(
        &mut self,
        template: &Document,
        recipe: &EditRecipe,
    ) -> Result<Vec<Layer>, String> {
        let same_recipe = self.previous.as_ref().is_some_and(|(_, before)| {
            before.text_layers == recipe.text_layers && before.brush_strokes == recipe.brush_strokes
        });
        if let Some(cache) = &self.annotations {
            if same_recipe
                && template.selection.is_none()
                && cache.template.size == template.size
                && cache.template.pixel_format() == template.pixel_format()
                && cache.template.resolution_dpi == template.resolution_dpi
                && cache.template.icc_profile == template.icc_profile
            {
                self.timings.annotation_cache_hit = true;
                return Ok(cache.layers.clone());
            }
        }
        // 文字/笔刷是独立透明图层，颜色参数变化无需重新栅格化整套标注。
        let mut empty = template.clone();
        empty.id = DocId::fresh();
        empty.name = "Annotations".into();
        empty.layers.clear();
        // 保留原生色彩模式/调色板与选区语义；有选区时暂不复用标注缓存。
        let mut session = Session::new();
        session.add_document(empty, None);
        photocraft::annotations(&mut session, recipe)?;
        let layers = session.active().ok_or("标注文档不存在")?.doc.layers.clone();
        // 只存小型模板元数据，不能把整张基础图片作为标注缓存多留一份。
        let mut metadata = Document::new(
            "Annotation metadata",
            template.size,
            template.mode,
            template.depth,
        );
        metadata.resolution_dpi = template.resolution_dpi;
        metadata.icc_profile = template.icc_profile.clone();
        self.annotations = Some(AnnotationCache {
            template: metadata,
            layers: layers.clone(),
        });
        Ok(layers)
    }

    /// Exercise the same stable layer IDs used by the first slider gesture.
    /// This document is rendered only while the native surface is hidden.
    pub(super) fn warm_canvas_document(&mut self, base: &Document) -> Result<Document, String> {
        let recipe = super::recipe::parse_recipe(&serde_json::json!({
            "version": 1, "renderer": "photocraft",
            "adjustments": {"brightness": 1.0, "contrast": 1.0, "saturation": 1.0},
            "advanced": {"exposure": 0.01, "vibrance": 1.0,
                "levels": {"black": 0.0, "white": 255.0, "gamma": 1.01},
                "curves": [[0.0,0.0],[128.0,129.0],[255.0,255.0]],
                "channelCurves": {"luminance": [[0.0,0.0],[255.0,255.0]]},
                "hsl": {"reds": {"hue": 1.0, "saturation": 0.0, "lightness": 0.0}}}
        }))?;
        self.canvas_document(base, &recipe)
    }

    /// Metadata-only preparation for a direct GPU canvas; never flatten or JPEG.
    pub(super) fn canvas_document(
        &mut self,
        base: &Document,
        recipe: &EditRecipe,
    ) -> Result<Document, String> {
        use photocraft_engine::doc::adjust::{CurvePoint, ToneSpace};
        use photocraft_engine::doc::{Adjustment, SampleType};
        if base.mode != photocraft_engine::doc::ColorMode::Rgb || base.depth == SampleType::F32 {
            return Err("原生画布暂不支持此色彩模式或 HDR 文档".into());
        }
        let tuning = photocraft::camera_tuning(recipe);
        let development=recipe.advanced.as_ref().and_then(|a|a.development.as_ref()).is_some_and(|d|d.active());
        let mut camera = if development {self.camera_document(base,recipe)?} else {base.clone()};
        if !development && tuning != [0.0; 3] {
            if !self
                .canvas_camera
                .as_ref()
                .is_some_and(|(key, _)| *key == tuning)
            {
                // The exposed camera controls are channel-separable. Sample the
                // upstream algorithm at all 256 display levels, not a custom look.
                let mut pixels: Vec<[f32; 4]> = (0..256)
                    .map(|i| [i as f32 / 255.0, i as f32 / 255.0, i as f32 / 255.0, 1.0])
                    .collect();
                let params = photocraft_algo::camera_raw::CameraRaw {
                    exposure: tuning[0] as f32,
                    temperature: tuning[1] as f32,
                    tint: tuning[2] as f32,
                    ..Default::default()
                };
                photocraft_algo::camera_raw::develop(&mut pixels, 256, 1, &params, false);
                let curves = std::array::from_fn(|ch| {
                    (0..256)
                        .map(|i| CurvePoint {
                            input: i as f32 / 255.0,
                            output: pixels[i][ch],
                        })
                        .collect()
                });
                let content = LayerContent::Adjustment(Adjustment::Curves {
                    master: Vec::new(),
                    per_channel: curves,
                    space: ToneSpace::Rgb,
                    black: Vec::new(),
                });
                let mut layer = self
                    .canvas_camera
                    .as_ref()
                    .map(|(_, l)| l.clone())
                    .unwrap_or_else(|| Layer::new("Camera preview", content.clone()));
                layer.content = content;
                self.canvas_camera = Some((tuning, layer));
            }
            camera
                .layers
                .push(self.canvas_camera.as_ref().unwrap().1.clone());
        }
        self.update_adjustments(camera, recipe)?;
        let mut doc = self.geometry_document(recipe)?;
        doc.layers.extend(self.annotation_layers(&doc, recipe)?);
        self.previous = Some((base.id, recipe.clone()));
        Ok(doc)
    }

    /// Build a separate post-geometry annotation document without warping pixels.
    pub(super) fn canvas_annotations(&mut self,base:&Document,recipe:&EditRecipe)->Result<Option<Document>,String> {
        if recipe.text_layers.iter().all(|l|l.text.trim().is_empty()) && recipe.brush_strokes.is_empty(){return Ok(None);}
        let size=super::native_geometry::canvas_size(recipe,base.size);
        let matches=self.overlay_template.as_ref().is_some_and(|d|d.size==size && d.mode==base.mode && d.depth==base.depth && d.icc_profile==base.icc_profile && d.resolution_dpi==base.resolution_dpi);
        if !matches {
            let mut d=Document::new("Annotations",size,base.mode,base.depth);d.icc_profile=base.icc_profile.clone();d.resolution_dpi=base.resolution_dpi;
            self.overlay_template=Some(d);
        }
        let mut doc=self.overlay_template.as_ref().unwrap().clone();
        doc.layers=self.annotation_layers(&doc,recipe)?;
        self.previous=Some((base.id,recipe.clone()));
        Ok(Some(doc))
    }

    pub fn render(
        &mut self,
        base: &Document,
        recipe: &EditRecipe,
        quality: u8,
    ) -> Result<Vec<u8>, String> {
        let result = self.render_inner(base, recipe, quality);
        if result.is_err() {
            // 部分阶段失败后，下一次不能以旧配方判断会话状态仍然有效。
            self.previous = None;
            self.geometry = None;
            self.jpeg.clear();
        }
        result
    }

    fn render_inner(
        &mut self,
        base: &Document,
        recipe: &EditRecipe,
        quality: u8,
    ) -> Result<Vec<u8>, String> {
        let started = Instant::now();
        self.timings = Timings {
            frames: self.timings.frames + 1,
            cached_frames: self.timings.cached_frames,
            ..Default::default()
        };
        if self
            .previous
            .as_ref()
            .is_some_and(|(id, old)| *id == base.id && old == recipe)
        {
            self.timings.cached_frames += 1;
            self.timings.frame_cache_hit = true;
            self.timings.total_ms = started.elapsed().as_secs_f64() * 1000.0;
            return Ok(self.jpeg.clone());
        }
        let phase = Instant::now();
        let camera = self.camera_document(base, recipe)?;
        self.timings.camera_ms = phase.elapsed().as_secs_f64() * 1000.0;
        let adjustments_changed = self.previous.as_ref().is_none_or(|(id, before)| {
            *id != base.id
                || before.adjustments != recipe.adjustments
                || before.advanced != recipe.advanced
                || before.masks != recipe.masks || before.legacy_adjustments != recipe.legacy_adjustments
        });
        let phase = Instant::now();
        if adjustments_changed {
            self.update_adjustments(camera, recipe)?;
        }
        self.timings.adjustments_ms = phase.elapsed().as_secs_f64() * 1000.0;
        let geometry_changed = adjustments_changed
            || self.previous.as_ref().is_none_or(|(_, before)| {
                before.rotate_quarter != recipe.rotate_quarter || before.crop != recipe.crop || before.geometry != recipe.geometry
            });
        let phase = Instant::now();
        if geometry_changed {
            self.geometry = Some(self.geometry_document(recipe)?);
        }
        self.timings.geometry_ms = phase.elapsed().as_secs_f64() * 1000.0;
        let mut doc = self.geometry.clone().ok_or("几何预览缓存不存在")?;
        let phase = Instant::now();
        let annotations = self.annotation_layers(&doc, recipe)?;
        doc.layers.extend(annotations);
        self.timings.annotations_ms = phase.elapsed().as_secs_f64() * 1000.0;
        let (jpeg, display) = photocraft::jpeg_with_timings(&doc, quality)?;
        self.timings.composite_ms = display.composite_ms;
        self.timings.profile_ms = display.profile_ms;
        self.timings.encode_ms = display.encode_ms;
        self.timings.gpu = display.gpu;
        self.timings.total_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.previous = Some((base.id, recipe.clone()));
        self.jpeg = jpeg.clone();
        Ok(jpeg)
    }
}
