//! Output framing is applied after photo edits. Footer artwork is vector, so
//! font and logo rendering scale to the requested output resolution.
use photocraft_engine::doc::{Document, Layer, LayerContent, PixelFormat, Rect, Surface};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BrandFrame {
    pub enabled: bool,
    pub padding: f64,
    pub footer_height: f64,
    pub background: String,
    pub footer_svg: String,
    #[serde(flatten)]
    pub settings: serde_json::Map<String, serde_json::Value>,
}
pub(super) fn validate(frame: &BrandFrame) -> Result<(), String> {
    if !frame.padding.is_finite()
        || !(0.0..=120.0).contains(&frame.padding)
        || !frame.footer_height.is_finite()
        || !(70.0..=260.0).contains(&frame.footer_height)
        || frame.background.len() != 7
        || !frame.background.starts_with('#')
        || !frame.background[1..].bytes().all(|b| b.is_ascii_hexdigit())
        || frame.footer_svg.len() > 1_500_000
        || serde_json::to_vec(&frame.settings)
            .map_err(|e| e.to_string())?
            .len()
            > 800_000
        || (frame.enabled && frame.footer_svg.is_empty())
    {
        return Err("品牌边框参数无效".into());
    }
    for (key, low, high) in [
        ("canvasRatio", 0.0, 3.0),
        ("blur", 8.0, 80.0),
        ("frost", 0.0, 90.0),
        ("logoSize", 50.0, 200.0),
        ("fontSize", 12.0, 40.0),
    ] {
        if frame.settings.get(key).is_some_and(|v| {
            v.as_f64()
                .is_none_or(|n| !n.is_finite() || n < low || n > high)
        }) {
            return Err("边框画布或毛玻璃参数无效".into());
        }
    }
    Ok(())
}

fn footer_height(frame: &BrandFrame) -> f64 {
    let number = |key: &str, default: f64| {
        frame
            .settings
            .get(key)
            .and_then(|v| v.as_f64())
            .unwrap_or(default)
    };
    let string = |key: &str| {
        frame
            .settings
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
    };
    let visible = |flag: &str, key: &str| {
        frame
            .settings
            .get(flag)
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            && !string(key).trim().is_empty()
    };
    let layout = string("layout");
    let logo_height = if string("logo") == "none" {
        0.0
    } else {
        (if layout == "minimal" { 28.0 } else { 40.0 }) * number("logoSize", 100.0) / 100.0 * 2.8
    };
    let lines = [
        ("showLens", "lens"),
        ("showParameters", "parameters"),
        ("showDate", "date"),
        ("showCaption", "caption"),
    ]
    .into_iter()
    .filter(|(flag, key)| visible(flag, key))
    .count() as f64;
    let text_height = if ["gallery", "signature", "minimal"].contains(&layout) {
        number("fontSize", 20.0) * 3.2
    } else {
        number("fontSize", 20.0)
            * (lines
                + if layout == "center" && visible("showCamera", "camera") {
                    1.0
                } else {
                    0.0
                })
            * 1.15
            / if layout == "center" { 0.43 } else { 0.37 }
    };
    frame.footer_height.max(logo_height).max(text_height)
}

fn blurred_background(
    doc: &Document,
    width: u32,
    height: u32,
    blur: f32,
    logical_width: f32,
) -> Result<Vec<u8>, String> {
    let mut display = doc.clone();
    photocraft_engine::color_cmds::convert_document(
        &mut display,
        photocraft_cms::Builtin::Srgb.profile(),
        photocraft_cms::Intent::RelativeColorimetric,
        true,
    )
    .map_err(|e| e.to_string())?;
    let image = photocraft_io::document_to_image(&display, &mut Vec::new())
        .map_err(|e| e.to_string())?
        .convert(
            photocraft_codecs::ChannelLayout::Rgba,
            photocraft_codecs::SampleType::U8,
        );
    let photo = image::RgbaImage::from_raw(doc.size.width, doc.size.height, image.data().to_vec())
        .ok_or("背景图像无效")?;
    let small_width = (width as f64 / width.max(height) as f64 * 512.0)
        .round()
        .max(1.0) as u32;
    let small_height = (height as f64 / width.max(height) as f64 * 512.0)
        .round()
        .max(1.0) as u32;
    let cover = image::DynamicImage::ImageRgba8(photo)
        .thumbnail(512, 512)
        .resize_to_fill(
            (small_width as f32 * 1.12).ceil() as u32,
            (small_height as f32 * 1.12).ceil() as u32,
            image::imageops::FilterType::Triangle,
        );
    let soft = cover
        .blur(blur * small_width as f32 / logical_width)
        .to_rgba8();
    let crop = image::imageops::crop_imm(
        &soft,
        (soft.width() - small_width) / 2,
        (soft.height() - small_height) / 2,
        small_width,
        small_height,
    )
    .to_image();
    Ok(
        image::imageops::resize(&crop, width, height, image::imageops::FilterType::Triangle)
            .into_raw(),
    )
}

fn border(frame: &BrandFrame, height: f64) -> String {
    let kind = frame
        .settings
        .get("border")
        .and_then(|v| v.as_str())
        .unwrap_or("none");
    let color = frame
        .settings
        .get("accent")
        .and_then(|v| v.as_str())
        .unwrap_or("#888888");
    if frame.padding < 8.0
        || kind == "none"
        || color.len() != 7
        || !color.starts_with('#')
        || !color[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return String::new();
    }
    let p = frame.padding - 5.0;
    let w = 1010.0;
    let h = height + 10.0;
    let rect = |inset: f64| {
        format!(
            "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\"/>",
            p - inset,
            p - inset,
            w + 2.0 * inset,
            h + 2.0 * inset
        )
    };
    let shape = match kind {
        "corners" => format!(
            "<path d=\"M{} {p}h-32v32 M{} {p}h32v32 M{p} {}v32h32 M{} {}h32v-32\"/>",
            p + 32.0,
            p + w - 32.0,
            p + h - 32.0,
            p + w - 32.0,
            p + h
        ),
        "double" => rect(0.0) + &rect(4.0),
        "single" => rect(0.0),
        _ => return String::new(),
    };
    format!("<g fill=\"none\" stroke=\"{color}\" stroke-width=\"1\">{shape}</g>")
}

pub(super) fn apply(
    mut doc: Document,
    frame: Option<&BrandFrame>,
    long_edge: Option<u32>,
) -> Result<Document, String> {
    let Some(frame) = frame.filter(|f| f.enabled) else {
        return Ok(doc);
    };
    validate(frame)?;
    if doc.mode != photocraft_engine::doc::ColorMode::Rgb {
        photocraft_engine::color_cmds::convert_document(
            &mut doc,
            photocraft_cms::Builtin::Srgb.profile(),
            photocraft_cms::Intent::RelativeColorimetric,
            true,
        )
        .map_err(|e| e.to_string())?;
    }
    let photo_width = doc.size.width;
    let scale = f64::from(photo_width) / 1000.0;
    let pad = (frame.padding * scale).round() as u32;
    let footer = (footer_height(frame) * scale).round().max(1.0) as u32;
    let base_width = photo_width.checked_add(2 * pad).ok_or("边框尺寸过大")?;
    let min_height = doc
        .size
        .height
        .checked_add(pad)
        .and_then(|h| h.checked_add(footer))
        .ok_or("边框尺寸过大")?;
    let ratio = frame
        .settings
        .get("canvasRatio")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    let width = if ratio > 0.0 {
        f64::from(base_width)
            .max(f64::from(min_height) * ratio)
            .round() as u32
    } else {
        base_width
    };
    let height = if ratio > 0.0 {
        (f64::from(width) / ratio)
            .round()
            .max(f64::from(min_height)) as u32
    } else {
        min_height
    };
    let x = (width - photo_width) / 2;
    let y = pad + (height - min_height) / 2;
    if u64::from(width) * u64::from(height) > 150_000_000 {
        return Err("品牌边框输出超过 1.5 亿像素，请减小输出尺寸".into());
    }
    let art = frame.footer_svg.replace("<svg ", "<svg x=\"0\" y=\"0\" ");
    let decoration = border(frame, f64::from(doc.size.height) / scale);
    let glass = frame
        .settings
        .get("backgroundStyle")
        .and_then(|v| v.as_str())
        == Some("blur");
    let opacity = if glass {
        frame
            .settings
            .get("frost")
            .and_then(|v| v.as_f64())
            .unwrap_or(50.0)
            / 100.0
    } else {
        1.0
    };
    let svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\"><rect width=\"100%\" height=\"100%\" fill=\"{}\" fill-opacity=\"{opacity}\"/><g transform=\"translate({} {}) scale({scale})\">{decoration}</g><g transform=\"translate({} {}) scale({})\">{art}</g></svg>", frame.background, f64::from(x)-frame.padding*scale, f64::from(y)-frame.padding*scale, (width-base_width)/2, height-footer, f64::from(base_width)/(1000.0+2.0*frame.padding));
    let mut options = resvg::usvg::Options::default();
    // Imported raster logos are embedded data URLs; arbitrary local files and
    // network resources are never resolved from persisted artwork.
    options.image_href_resolver.resolve_string = Box::new(|_, _| None);
    options.fontdb_mut().load_system_fonts();
    for font in [
        include_bytes!("../../../src/features/editor/assets/fonts/Inter-Regular.ttf").as_slice(),
        include_bytes!("../../../src/features/editor/assets/fonts/Inter-Bold.ttf").as_slice(),
        include_bytes!("../../../src/features/editor/assets/fonts/Manrope-Regular.ttf").as_slice(),
        include_bytes!("../../../src/features/editor/assets/fonts/Manrope-Bold.ttf").as_slice(),
        include_bytes!("../../../src/features/editor/assets/fonts/Lora.ttf").as_slice(),
    ] {
        options.fontdb_mut().load_font_data(font.to_vec());
    }
    let tree =
        resvg::usvg::Tree::from_str(&svg, &options).map_err(|e| format!("边框绘制失败：{e}"))?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(width, height).ok_or("边框内存不足")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixels.as_mut(),
    );
    let mut rgba = pixels.take();
    if glass {
        let blur = frame
            .settings
            .get("blur")
            .and_then(|v| v.as_f64())
            .unwrap_or(32.0) as f32;
        let background =
            blurred_background(&doc, width, height, blur, width as f32 / scale as f32)?;
        for (over, under) in rgba.chunks_exact_mut(4).zip(background.chunks_exact(4)) {
            let alpha = u32::from(over[3]);
            for c in 0..3 {
                over[c] =
                    (u32::from(over[c]) + u32::from(under[c]) * (255 - alpha) / 255).min(255) as u8;
            }
            over[3] = 255;
        }
    }
    let matte = photocraft_codecs::Image::from_u8(
        width,
        height,
        photocraft_codecs::ChannelLayout::Rgba,
        rgba,
    )
    .map_err(|e| e.to_string())?
    .with_icc(Some(
        photocraft_cms::Builtin::Srgb.profile().to_bytes().to_vec(),
    ));
    let mut background = super::source::from_image("Brand frame", &matte)?;
    if let Some(icc) = &doc.icc_profile {
        let profile = photocraft_cms::Profile::parse(icc).map_err(|e| e.to_string())?;
        photocraft_engine::color_cmds::convert_document(
            &mut background,
            &profile,
            photocraft_cms::Intent::RelativeColorimetric,
            true,
        )
        .map_err(|e| e.to_string())?;
    }
    let photo =
        photocraft_io::document_to_image(&doc, &mut Vec::new()).map_err(|e| e.to_string())?;
    let matte = photocraft_io::document_to_image(&background, &mut Vec::new())
        .map_err(|e| e.to_string())?
        .convert(photo.layout(), photo.sample_type());
    let mut result = super::source::from_image("Framed photo", &matte)?;
    let mut surface = Surface::new(PixelFormat::new(doc.mode, doc.depth, true));
    let photo = photo.convert(photocraft_codecs::ChannelLayout::Rgba, photo.sample_type());
    surface.write_interleaved(
        Rect::from_xywh(x as i32, y as i32, doc.size.width, doc.size.height),
        photo.data(),
    );
    result
        .layers
        .push(Layer::new("Photo", LayerContent::Raster(surface)));
    result.icc_profile = doc.icc_profile.clone();
    result.metadata = doc.metadata.clone();
    result.resolution_dpi = doc.resolution_dpi;
    // The requested long edge describes the finished image, including its frame.
    super::photocraft::resize(&result, long_edge)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn frame() -> BrandFrame {
        serde_json::from_value(serde_json::json!({"enabled":true,"padding":100,"footerHeight":200,"background":"#ffffff","footerSvg":"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"200\"><rect width=\"1200\" height=\"200\" fill=\"#ff0000\"/></svg>"})).unwrap()
    }
    #[test]
    fn adds_frame_without_cropping_photo_and_honors_finished_size() {
        let image = photocraft_codecs::Image::from_u8(
            100,
            60,
            photocraft_codecs::ChannelLayout::Rgba,
            [20, 80, 140, 255].repeat(6000),
        )
        .unwrap();
        let doc = super::super::source::from_image("Photo", &image).unwrap();
        let framed = apply(doc.clone(), Some(&frame()), None).unwrap();
        assert_eq!((framed.size.width, framed.size.height), (120, 90));
        let image = photocraft_io::document_to_image(&framed, &mut Vec::new())
            .unwrap()
            .convert(
                photocraft_codecs::ChannelLayout::Rgba,
                photocraft_codecs::SampleType::U8,
            );
        let pixel = |x: usize, y: usize| &image.data()[(y * 120 + x) * 4..(y * 120 + x) * 4 + 4];
        assert_eq!(pixel(0, 0), &[255, 255, 255, 255]);
        assert_eq!(pixel(10, 10), &[20, 80, 140, 255]);
        assert_eq!(pixel(109, 69), &[20, 80, 140, 255]);
        assert_eq!(pixel(40, 80), &[255, 0, 0, 255]);
        let resized = apply(doc, Some(&frame()), Some(60)).unwrap();
        assert_eq!(resized.size.width.max(resized.size.height), 60);
    }
    #[test]
    fn validates_and_round_trips_custom_settings() {
        let mut value = frame();
        value
            .settings
            .insert("font".into(), serde_json::json!("Georgia"));
        assert!(validate(&value).is_ok());
        assert_eq!(
            serde_json::from_value::<BrandFrame>(serde_json::to_value(&value).unwrap()).unwrap(),
            value
        );
        value.padding = f64::NAN;
        assert!(validate(&value).is_err());
        value.padding = 10.0;
        value.background = "#ff\"fff".into();
        assert!(validate(&value).is_err());
    }
    #[test]
    fn retains_sixteen_bit_photo_samples_and_disabled_frames_do_nothing() {
        let image = photocraft_codecs::Image::from_u16(
            10,
            6,
            photocraft_codecs::ChannelLayout::Rgba,
            &[12345, 23456, 34567, 65535].repeat(60),
        )
        .unwrap();
        let doc = super::super::source::from_image("16-bit photo", &image).unwrap();
        let mut settings = frame();
        settings.enabled = false;
        assert_eq!(
            apply(doc.clone(), Some(&settings), None).unwrap().size,
            doc.size
        );
        settings.enabled = true;
        let out = apply(doc, Some(&settings), None).unwrap();
        assert_eq!(out.depth, photocraft_engine::doc::SampleType::U16);
        let image = photocraft_io::document_to_image(&out, &mut Vec::new())
            .unwrap()
            .convert(
                photocraft_codecs::ChannelLayout::Rgba,
                photocraft_codecs::SampleType::U16,
            );
        let start = ((1 * 12 + 1) * 4) * 2;
        let sample: Vec<_> = image.data()[start..start + 8]
            .chunks_exact(2)
            .map(|v| u16::from_ne_bytes([v[0], v[1]]))
            .collect();
        assert_eq!(sample, vec![12345, 23456, 34567, 65535]);
    }
    #[test]
    fn wide_glass_canvas_keeps_photo_pixels_and_blurs_only_the_background() {
        let image = photocraft_codecs::Image::from_u8(
            100,
            60,
            photocraft_codecs::ChannelLayout::Rgba,
            [20, 80, 140, 255].repeat(6000),
        )
        .unwrap();
        let doc = super::super::source::from_image("Photo", &image).unwrap();
        let mut value = frame();
        value
            .settings
            .insert("canvasRatio".into(), serde_json::json!(16.0 / 9.0));
        value
            .settings
            .insert("backgroundStyle".into(), serde_json::json!("blur"));
        value.settings.insert("frost".into(), serde_json::json!(0));
        let result = apply(doc, Some(&value), None).unwrap();
        assert_eq!((result.size.width, result.size.height), (160, 90));
        let image = photocraft_io::document_to_image(&result, &mut Vec::new())
            .unwrap()
            .convert(
                photocraft_codecs::ChannelLayout::Rgba,
                photocraft_codecs::SampleType::U8,
            );
        let pixel = |x: usize, y: usize| &image.data()[(y * 160 + x) * 4..(y * 160 + x) * 4 + 4];
        assert_eq!(pixel(30, 10), &[20, 80, 140, 255]);
        assert_eq!(pixel(129, 69), &[20, 80, 140, 255]);
        for (a, b) in pixel(0, 0).iter().zip([20u8, 80, 140, 255]) {
            assert!(a.abs_diff(b) <= 2);
        }
    }
    #[test]
    fn official_vector_logo_is_embedded_in_export_artwork() {
        use base64::Engine;
        let logo = include_str!("../../../src/features/editor/assets/brands/leica.svg");
        let mut value = frame();
        value.footer_svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"200\"><image width=\"1200\" height=\"200\" href=\"data:image/svg+xml;base64,{}\"/></svg>",base64::engine::general_purpose::STANDARD.encode(logo));
        let image = photocraft_codecs::Image::from_u8(
            100,
            60,
            photocraft_codecs::ChannelLayout::Rgba,
            [20, 80, 140, 255].repeat(6000),
        )
        .unwrap();
        let doc = super::super::source::from_image("Photo", &image).unwrap();
        let out = apply(doc, Some(&value), None).unwrap();
        let image = photocraft_io::document_to_image(&out, &mut Vec::new())
            .unwrap()
            .convert(
                photocraft_codecs::ChannelLayout::Rgba,
                photocraft_codecs::SampleType::U8,
            );
        assert!(image.data()[70 * 120 * 4..]
            .chunks_exact(4)
            .any(|p| p[0] > 180 && p[1] < 80 && p[2] < 80));
    }
    #[test]
    fn recipe_frame_reaches_jpeg_and_png_exports_without_changing_the_source() {
        use super::super::export::{
            encode_output, ExportColorSpace, ExportFormat, ExportMode, ValidatedOptions,
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("photo.png");
        image::RgbImage::from_pixel(100, 60, image::Rgb([20, 80, 140]))
            .save(&path)
            .unwrap();
        let before = std::fs::read(&path).unwrap();
        let recipe = super::super::recipe::parse_recipe(
            &serde_json::json!({"version":1,"renderer":"photocraft","brandFrame":frame()}),
        )
        .unwrap();
        for format in [ExportFormat::Jpeg, ExportFormat::Png] {
            let options = ValidatedOptions {
                file_name: None,
                finish: Default::default(),
                format,
                color_space: ExportColorSpace::Srgb,
                mode: ExportMode::Folder,
                folder: None,
                album: None,
                long_edge: Some(60),
                quality: Some(95),
                remove_gps: true,
                copyright: None,
                author: None,
                keywords: vec![],
            };
            let (bytes, width, height) = encode_output(&path, &recipe, &options, &|_| {}).unwrap();
            assert_eq!((width, height), (60, 45));
            let image = image::load_from_memory(&bytes).unwrap();
            assert_eq!((image.width(), image.height()), (width, height));
        }
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
    #[test]
    fn bundled_fonts_preserve_regular_and_bold_weights_in_native_exports() {
        let image = photocraft_codecs::Image::from_u8(
            100,
            60,
            photocraft_codecs::ChannelLayout::Rgba,
            [20, 80, 140, 255].repeat(6000),
        )
        .unwrap();
        let doc = super::super::source::from_image("Photo", &image).unwrap();
        for family in ["Inter", "Manrope"] {
            let mut samples = Vec::new();
            for weight in [400, 700] {
                let mut value = frame();
                value.footer_svg=format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1200\" height=\"200\"><text x=\"100\" y=\"150\" font-family=\"{family}\" font-weight=\"{weight}\" font-size=\"120\">ISO 100</text></svg>");
                let out = apply(doc.clone(), Some(&value), None).unwrap();
                let image = photocraft_io::document_to_image(&out, &mut Vec::new())
                    .unwrap()
                    .convert(
                        photocraft_codecs::ChannelLayout::Rgba,
                        photocraft_codecs::SampleType::U8,
                    );
                samples.push(image.data().to_vec());
            }
            assert_ne!(
                samples[0], samples[1],
                "{family} weights must have different glyph outlines"
            );
        }
    }
}
