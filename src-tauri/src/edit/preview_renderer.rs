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
    session: Session,
    camera: Option<(DocId, [f64; 3], Document)>,
    adjustments: Vec<AdjustmentLayer>,
    geometry: Option<Document>,
    annotations: Option<AnnotationCache>,
    previous: Option<(DocId, EditRecipe)>,
    jpeg: Vec<u8>,
    pub timings: Timings,
}

impl Renderer {
    pub fn new(base: &Document) -> Self {
        let mut session = Session::new();
        session.add_document(base.clone(), None);
        Self {
            session,
            camera: None,
            adjustments: Vec::new(),
            geometry: None,
            annotations: None,
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
        let tuning = photocraft::camera_tuning(recipe);
        if let Some((id, key, doc)) = &self.camera {
            if *id == base.id && *key == tuning {
                self.timings.camera_cache_hit = true;
                return Ok(doc.clone());
            }
        }
        self.set_document(base.clone())?;
        photocraft::camera_adjustment(&mut self.session, recipe)?;
        let document = photocraft::active_document(&self.session)?;
        self.camera = Some((base.id, tuning, document.clone()));
        Ok(document)
    }

    fn update_adjustments(
        &mut self,
        mut base: Document,
        recipe: &EditRecipe,
    ) -> Result<(), String> {
        for spec in photocraft::adjustment_specs(recipe) {
            let cached = self
                .adjustments
                .iter_mut()
                .find(|layer| layer.key == spec.key);
            let layer = match cached {
                Some(cached) => {
                    if cached.params != spec.params {
                        let adjustment = photocraft_engine::adjust_params::from_params(
                            spec.kind,
                            &spec.params,
                            None,
                            base.mode,
                        )
                        .map_err(|e| e.to_string())?;
                        cached.layer.content = LayerContent::Adjustment(adjustment);
                        cached.params = spec.params;
                    }
                    cached.layer.clone()
                }
                None => {
                    let adjustment = photocraft_engine::adjust_params::from_params(
                        spec.kind,
                        &spec.params,
                        None,
                        base.mode,
                    )
                    .map_err(|e| e.to_string())?;
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
            layer.mask = base.selection.as_ref().map(|surface| LayerMask {
                surface: surface.clone(),
                ..LayerMask::reveal_all()
            });
            base.layers.push(layer);
        }
        self.set_document(base)
    }

    fn geometry_document(&self, recipe: &EditRecipe) -> Result<Document, String> {
        let doc = photocraft::active_document(&self.session)?;
        if recipe.rotate_quarter == 0 && recipe.crop.is_none() {
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
        });
        let phase = Instant::now();
        if adjustments_changed {
            self.update_adjustments(camera, recipe)?;
        }
        self.timings.adjustments_ms = phase.elapsed().as_secs_f64() * 1000.0;
        let geometry_changed = adjustments_changed
            || self.previous.as_ref().is_none_or(|(_, before)| {
                before.rotate_quarter != recipe.rotate_quarter || before.crop != recipe.crop
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
