//! M0 diagnostic only: LibRaw linear RGB -> film/neutral -> PhotoCraft PNG.
//! Usage: rawlab_probe <trusted bridge> <RAW> <new output.png> [compatible film.cube]
//! Does not modify the original, settings, database or saved recipes.
use photographer_media::rawlab::{Backend, DevelopOptions, GpuMode};
use std::{io::Write, path::Path, sync::atomic::AtomicBool, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().collect();
    if !(4..=5).contains(&args.len()) {
        return Err("usage: rawlab_probe <trusted bridge> <RAW> <new output.png> [RawLab-compatible film.cube]".into());
    }
    if Path::new(&args[3]).extension().and_then(|s| s.to_str()) != Some("png") {
        return Err("prototype output must be .png".into());
    }
    if std::fs::metadata(&args[2])?.len() > 1024 * 1024 * 1024 {
        return Err("RAW exceeds 1 GiB prototype input budget".into());
    }
    let start = Instant::now();
    let backend = unsafe { Backend::load(&args[1]) }?;
    let source = args[2].to_str().ok_or("RAW path is not UTF-8")?;
    let linear = backend.develop(source, DevelopOptions::default(), &AtomicBool::new(false))?;
    let decode_ms = start.elapsed().as_secs_f64() * 1000.0;
    let [width, height] = linear.dimensions();
    // Prove the lossless float/ICC handoff without applying display RGB filters
    // directly to a linear document. Further edits follow the display stage.
    let document = photographer_lib::linear_rgb_document(
        "LibRaw prototype",
        width,
        height,
        linear.rgb(),
    )?;
    if document.depth != photocraft_engine::doc::SampleType::F32 {
        return Err("linear document lost float depth".into());
    }
    drop(document);
    let lut_path = args
        .get(4)
        .map(|path| path.to_str().ok_or("LUT path is not UTF-8"))
        .transpose()?;
    if let Some(path) = lut_path {
        if std::fs::metadata(path)?.len() > 16 * 1024 * 1024 {
            return Err("film LUT exceeds 16 MiB".into());
        }
    }
    let display = backend.render(
        linear.rgb(),
        [width, height],
        lut_path,
        1.0,
        GpuMode::Auto,
        &AtomicBool::new(false),
    )?;
    // Already oriented; there is no second application of source EXIF rotation.
    let image = photocraft_codecs::Image::from_f32(
        width,
        height,
        photocraft_codecs::ChannelLayout::Rgb,
        display.rgb(),
    )?
    .with_icc(Some(
        photocraft_cms::Builtin::Srgb.profile().to_bytes().to_vec(),
    ));
    let mut surface =
        photocraft_engine::doc::Surface::new(photocraft_engine::doc::PixelFormat::new(
            photocraft_engine::doc::ColorMode::Rgb,
            photocraft_engine::doc::SampleType::F32,
            true,
        ));
    let native = image.convert(
        photocraft_codecs::ChannelLayout::Rgba,
        photocraft_codecs::SampleType::F32,
    );
    let mut doc = photocraft_engine::doc::Document::new(
        "Display prototype",
        photocraft_engine::doc::Size::new(width, height),
        photocraft_engine::doc::ColorMode::Rgb,
        photocraft_engine::doc::SampleType::F32,
    );
    surface.write_interleaved(doc.bounds(), native.data());
    doc.layers.push(photocraft_engine::doc::Layer::new(
        "Photo",
        photocraft_engine::doc::LayerContent::Raster(surface),
    ));
    doc.icc_profile = image.icc.map(std::sync::Arc::new);
    let output = photocraft_io::export(&doc, "png", &photocraft_io::ExportOptions::default())?;
    // create_new prevents overwriting an existing file, including original photos.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&args[3])?;
    file.write_all(&output.bytes)?;
    eprintln!(
        "{}x{}, DNG baseline={} EV, decode={decode_ms:.1} ms, gpu={}, total={:.1} ms",
        width,
        height,
        linear.baseline_ev(),
        display.gpu_used(),
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(())
}
