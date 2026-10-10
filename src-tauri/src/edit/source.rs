//! PhotoCraft 原生导入/RAW 数据缓存。只适配文档表示，不实现解码或显影算法。
use super::recipe::EditRecipe;
use photocraft_codecs::{ChannelLayout, Image, SampleType as CodecSample};
use photocraft_engine::doc::{
    ColorMode, Document, Layer, LayerContent, PixelFormat, SampleType, Size, Surface,
};
use photocraft_raw::{DevelopOptions, Developed, Sensor, WhiteBalance};
use std::{path::Path, sync::Arc};

pub struct RawSource {
    sensor: Arc<Sensor>,
    white_balance: [f64; 3],
}

pub struct Opened {
    pub original_size: [u32; 2],
    pub document: Document,
    pub raw: Option<Arc<RawSource>>,
    pub warnings: Vec<String>,
}

/// 与上游 io::flat 的文档表示一致；像素布局/位深转换由原生 codecs 处理。
pub fn from_image(name: &str, image: &Image) -> Result<Document, String> {
    let (mode, layout) = match image.layout() {
        ChannelLayout::Gray | ChannelLayout::GrayA => (ColorMode::Grayscale, ChannelLayout::GrayA),
        ChannelLayout::Rgb | ChannelLayout::Rgba => (ColorMode::Rgb, ChannelLayout::Rgba),
        ChannelLayout::Cmyk | ChannelLayout::CmykA => (ColorMode::Cmyk, ChannelLayout::CmykA),
    };
    let (depth, sample) = match image.sample_type() {
        CodecSample::U8 => (SampleType::U8, CodecSample::U8),
        CodecSample::U16 => (SampleType::U16, CodecSample::U16),
        CodecSample::F16 | CodecSample::F32 => (SampleType::F32, CodecSample::F32),
    };
    let native = image.convert(layout, sample);
    let (width, height) = native.dimensions();
    let mut doc = Document::new(name, Size::new(width, height), mode, depth);
    let mut surface = Surface::new(PixelFormat::new(mode, depth, true));
    surface.write_interleaved(doc.bounds(), native.data());
    doc.layers
        .push(Layer::new("Photo", LayerContent::Raster(surface)));
    doc.icc_profile = image.icc.clone().map(Arc::new);
    doc.metadata.exif = image.meta.exif.clone().map(Arc::new);
    doc.metadata.xmp = image.meta.xmp.clone();
    if let Some((dpi, _)) = image.meta.dpi {
        doc.resolution_dpi = dpi;
    }
    Ok(doc)
}

fn developed_document(developed: &Developed) -> Result<Document, String> {
    let image = Image::from_u16(
        developed.width,
        developed.height,
        ChannelLayout::Rgb,
        &developed.rgb,
    )
    .map_err(|e| e.to_string())?
    .with_icc(Some(
        photocraft_cms::Builtin::ProPhotoCompat
            .profile()
            .to_bytes()
            .to_vec(),
    ));
    from_image("RAW", &image)
}

// Shrink the contiguous 16-bit pixels before constructing a tiled document.
// This avoids creating and then resampling a full 61MP document just for a proxy.
fn shrink_developed(developed: &mut Developed, edge: u32) -> Result<(), String> {
    let long = developed.width.max(developed.height);
    if long <= edge {
        return Ok(());
    }
    let pixels = std::mem::take(&mut developed.rgb);
    let image = image::ImageBuffer::<image::Rgb<u16>, Vec<u16>>::from_raw(
        developed.width,
        developed.height,
        pixels,
    )
    .ok_or("Invalid RAW preview buffer")?;
    let width = (u64::from(developed.width) * u64::from(edge) / u64::from(long)).max(1) as u32;
    let height = (u64::from(developed.height) * u64::from(edge) / u64::from(long)).max(1) as u32;
    let resized =
        image::imageops::resize(&image, width, height, image::imageops::FilterType::Triangle);
    developed.width = width;
    developed.height = height;
    developed.rgb = resized.into_raw();
    Ok(())
}

pub fn open(path: &Path) -> Result<Opened, String> {
    open_if_current(path, || true)
}

pub fn open_if_current(path: &Path, current: impl Fn() -> bool) -> Result<Opened, String> {
    open_for(path, None, current)
}

/// Interactive preparation uses fast native demosaicing and builds tiles only
/// for the fixed proxy. Full-quality source/export retain upstream defaults.
pub fn open_preview_if_current(
    path: &Path,
    edge: u32,
    current: impl Fn() -> bool,
) -> Result<Opened, String> {
    open_for(path, Some(edge), current)
}

fn open_for(
    path: &Path,
    preview_edge: Option<u32>,
    current: impl Fn() -> bool,
) -> Result<Opened, String> {
    if !current() {
        return Err("预览请求已被更新或关闭".into());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("读取照片失败: {e}"))?;
    if !current() {
        return Err("预览请求已被更新或关闭".into());
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("Photo");
    if photocraft_raw::is_raw(&bytes) {
        match photocraft_raw::decode(&bytes, &photocraft_raw::Limits::default()) {
            Ok(sensor) => {
                let options = DevelopOptions {
                    demosaic: if preview_edge.is_some() {
                        photocraft_raw::Demosaic::Mhc
                    } else {
                        photocraft_raw::Demosaic::Ahd
                    },
                    ..DevelopOptions::default()
                };
                let mut developed =
                    develop_when(&sensor, &options, &current).map_err(|e| e.to_string())?;
                let original_size = [developed.width, developed.height];
                if let Some(edge) = preview_edge {
                    shrink_developed(&mut developed, edge)?;
                }
                let doc = developed_document(&developed)?;
                return Ok(Opened {
                    original_size,
                    document: doc,
                    raw: Some(Arc::new(RawSource {
                        sensor: Arc::new(sensor),
                        white_balance: developed.info.wb_multipliers,
                    })),
                    warnings: developed.warnings,
                });
            }
            Err(photocraft_raw::RawError::Unsupported(_)) => { /* 上游原生导入返回明确的内嵌预览提示。 */
            }
            Err(error) => return Err(format!("RAW 解码失败: {error}")),
        }
    }
    let imported = photocraft_io::import(name, &bytes)
        .map_err(|e| format!("PhotoCraft 无法导入此格式: {e}"))?;
    if !current() {
        return Err("预览请求已被更新或关闭".into());
    }
    Ok(Opened {
        original_size: [imported.document.size.width, imported.document.size.height],
        document: imported.document,
        raw: None,
        warnings: imported.warnings,
    })
}

impl RawSource {
    pub fn bytes(&self) -> usize {
        self.sensor.data.len() * std::mem::size_of::<u16>()
    }

    pub fn develop(&self, recipe: &EditRecipe) -> Result<Document, String> {
        self.develop_if_current(recipe, || true)
    }

    pub fn develop_if_current(
        &self,
        recipe: &EditRecipe,
        current: impl Fn() -> bool,
    ) -> Result<Document, String> {
        let a = recipe.advanced.clone().unwrap_or_default();
        let gains =
            photocraft_algo::camera_raw::white_balance_gains(a.temperature as f32, a.tint as f32);
        let options = DevelopOptions {
            exposure: a.exposure,
            white_balance: WhiteBalance::Multipliers(std::array::from_fn(|i| {
                self.white_balance[i] * f64::from(gains[i])
            })),
            ..Default::default()
        };
        let developed = develop_when(&self.sensor, &options, &current)?;
        if !current() {
            return Err("预览请求已被更新或关闭".into());
        }
        developed_document(&developed)
    }
}

// 初始化、精细预览和导出共用限流，避免多个完整显影争抢内存和工作线程。
fn develop_when(
    sensor: &Sensor,
    options: &DevelopOptions,
    current: impl Fn() -> bool,
) -> Result<Developed, String> {
    static DEVELOPMENT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = DEVELOPMENT.lock().map_err(|_| "RAW 显影锁不可用")?;
    if !current() {
        return Err("预览请求已被更新或关闭".into());
    }
    let developed = photocraft_raw::develop_sensor(sensor, options).map_err(|e| e.to_string())?;
    if !current() {
        return Err("预览请求已被更新或关闭".into());
    }
    Ok(developed)
}

#[cfg(test)]
mod preview_tests {
    use super::*;
    #[test]
    fn preview_shrinks_before_tiling_and_preserves_16_bit_colour() {
        let info = photocraft_raw::RawInfo {
            format: photocraft_raw::RawFormat::Dng,
            make: None,
            model: None,
            sensor_width: 240,
            sensor_height: 160,
            cfa: None,
            wb_multipliers: [1.0; 3],
            orientation: 1,
            baseline_exposure: 0.0,
        };
        let mut developed = Developed {
            width: 240,
            height: 160,
            rgb: vec![12000; 240 * 160 * 3],
            info,
            warnings: vec![],
        };
        shrink_developed(&mut developed, 60).unwrap();
        assert_eq!((developed.width, developed.height), (60, 40));
        assert!(developed.rgb.iter().all(|sample| *sample == 12000));
        assert_eq!(developed.info.sensor_width, 240);
        let doc = developed_document(&developed).unwrap();
        assert_eq!(doc.depth, SampleType::U16);
        assert_eq!(doc.size, Size::new(60, 40));
        assert!(doc.icc_profile.is_some());
    }
}
