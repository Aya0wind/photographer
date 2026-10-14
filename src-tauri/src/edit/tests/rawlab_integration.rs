//! Edit component integration: native core, source handoff and renderer.
use super::*;
#[test]
fn native_stage_consumes_controls_once_and_reuses_for_downstream_edits() {
    let Some(path) = std::env::var_os("PHOTOGRAPHER_RAW_BRIDGE") else {
        return;
    };
    let backend = Arc::new(unsafe { Backend::load(path) }.unwrap());
    let _ = BACKEND.set(Ok(backend.clone()));
    let base = photocraft::linear_rgb_document("Linear", 1, 1, &[0.1, 0.2, 0.3]).unwrap();
    let mut recipe=super::super::recipe::parse_recipe(&serde_json::json!({"version":1,"renderer":"photocraft","colorPipeline":{"version":1,"backend":"rawlab","coreRevision":core_revision()},"advanced":{"exposure":1,"temperature":0,"tint":0}})).unwrap();
    let mut cache = ColorCache::default();
    let (display, downstream) = cache.apply(&base, &recipe).unwrap();
    assert!(downstream.color_pipeline.is_none());
    assert_eq!(downstream.advanced.as_ref().unwrap().exposure, 0.0);
    let expected = backend
        .render(
            &[0.2, 0.4, 0.6],
            [1, 1],
            None,
            0.0,
            GpuMode::Off,
            &AtomicBool::new(false),
        )
        .unwrap();
    let actual = photocraft_compose::flatten(&display);
    for c in 0..3 {
        assert!((actual.px[0][c] - expected.rgb()[c]).abs() < 0.0021);
    }
    recipe.advanced.as_mut().unwrap().vibrance = 30.0;
    let (again, _) = cache.apply(&base, &recipe).unwrap();
    assert_eq!(display.id, again.id);
    recipe.advanced.as_mut().unwrap().exposure = 0.0;
    let (different, _) = cache.apply(&base, &recipe).unwrap();
    assert_ne!(display.id, different.id);
    let mut renderer = super::super::preview_renderer::Renderer::new(&base);
    let preview = renderer.canvas_document(&base, &recipe).unwrap();
    let export = photocraft::render_document(&base, &recipe).unwrap();
    let preview = photocraft_compose::flatten(&preview);
    let export = photocraft_compose::flatten(&export);
    for c in 0..3 {
        assert!((preview.px[0][c] - export.px[0][c]).abs() < 0.0021);
    }
}
