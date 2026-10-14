//! Shared-media/PhotoCraft seam: storage precision and ICC are actual contracts.
use photocraft_engine::doc::{LayerContent, SampleType};
use photographer_lib::edit::photocraft::linear_rgb_document;

#[test]
fn linear_source_keeps_negative_superwhite_and_matching_icc() {
    let rgb = [-0.125, 0.1845, 2.5, 4.0, 0.0, 0.25];
    let doc = linear_rgb_document("Linear RAW", 2, 1, &rgb).unwrap();
    assert_eq!(doc.depth, SampleType::F32);
    assert_eq!(
        doc.icc_profile,
        Some(photocraft_cms::Builtin::LinearSrgb.profile().to_bytes())
    );
    let LayerContent::Raster(surface) = &doc.layers[0].content else {
        panic!("Expected linear source tiles")
    };
    assert_eq!(surface.pixel(0, 0), vec![-0.125, 0.1845, 2.5, 1.0]);
    assert_eq!(surface.pixel(1, 0), vec![4.0, 0.0, 0.25, 1.0]);
    assert!(linear_rgb_document("Invalid", 1, 1, &[f32::NAN, 0.0, 0.0]).is_err());
}
