//! Bounded native analysis artifacts shared by selection and face consumers.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use crate::ai::face::DetectedFace;
use crate::db::Db;

const IMAGE_BYTES: usize = 64 * 1024 * 1024;

type ImageCell = Arc<OnceLock<Option<Arc<image::RgbImage>>>>;
type ImageEntry = (String, ImageCell);
fn images() -> &'static Mutex<VecDeque<ImageEntry>> {
    static IMAGES: OnceLock<Mutex<VecDeque<ImageEntry>>> = OnceLock::new();
    IMAGES.get_or_init(Default::default)
}

fn source_key(db: &Db, path: &Path, asset: i64) -> Option<String> {
    let (source, size, mtime): (String, i64, String) =
        db.0.query_row(
            "SELECT path,size,mtime FROM assets WHERE id=?1",
            [asset],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .ok()?;
    Some(format!(
        "{}:{source}:{size}:{mtime}:analysis-1024-v1",
        path.display()
    ))
}

pub fn analysis(db: &Db, path: &Path, asset: i64) -> Option<Arc<image::RgbImage>> {
    let key = source_key(db, path, asset)?;
    let cell = {
        let mut cache = images().lock().ok()?;
        if let Some(position) = cache.iter().position(|(candidate, _)| *candidate == key) {
            let entry = cache.remove(position)?;
            let cell = entry.1.clone();
            cache.push_back(entry);
            cell
        } else {
            // Each analysis image is at most 1024*1024 RGB bytes.
            while cache.len() >= IMAGE_BYTES / (1024 * 1024 * 3) {
                cache.pop_front();
            }
            let cell = Arc::new(OnceLock::new());
            cache.push_back((key.clone(), cell.clone()));
            cell
        }
    };
    let result = cell
        .get_or_init(|| {
            let (source, mtime): (String, String) =
                db.0.query_row("SELECT path,mtime FROM assets WHERE id=?1", [asset], |r| {
                    Ok((r.get(0)?, r.get(1)?))
                })
                .ok()?;
            let source = PathBuf::from(source);
            // Never search arbitrary generations. The ledger defines the required input.
            let file = crate::thumbs::cached_for_asset(path, &source, 1024, &mtime)?;
            let rgb = image::ImageReader::open(file)
                .ok()?
                .decode()
                .ok()?
                .to_rgb8();
            if rgb.width() > 1024 || rgb.height() > 1024 {
                return None;
            }
            Some(Arc::new(rgb))
        })
        .clone();
    if result.is_none() {
        images()
            .lock()
            .ok()?
            .retain(|(candidate, entry)| candidate != &key || !Arc::ptr_eq(entry, &cell));
    }
    result
}

/// Rebuilding an unchanged source must also discard decoded artifacts.
pub fn invalidate(path: &Path) {
    let prefix = format!("{}:", path.display());
    if let Ok(mut cache) = images().lock() {
        cache.retain(|(key, _)| !key.starts_with(&prefix));
    }
    if let Ok(mut cache) = detections().lock() {
        cache.retain(|(key, _)| !key.starts_with(&prefix));
    }
}

type DetectionEntry = (String, Arc<Vec<DetectedFace>>);
fn detections() -> &'static Mutex<VecDeque<DetectionEntry>> {
    static DETECTIONS: OnceLock<Mutex<VecDeque<DetectionEntry>>> = OnceLock::new();
    DETECTIONS.get_or_init(Default::default)
}

pub fn faces(
    db: &Db,
    path: &Path,
    asset: i64,
    manager: &crate::ai::ModelManager,
    image: &image::RgbImage,
) -> Result<Arc<Vec<DetectedFace>>, String> {
    let params = manager.ai_params();
    let key = format!(
        "{}:{}:{}:{:?}",
        source_key(db, path, asset).ok_or("资产不存在")?,
        crate::ai::face_detect_model_id(params.quality_tier),
        params.face_detect_threshold,
        (image.width(), image.height())
    );
    let mut cache = detections().lock().map_err(|_| "人脸产物缓存不可用")?;
    if let Some((_, result)) = cache.iter().find(|(candidate, _)| *candidate == key) {
        return Ok(result.clone());
    }
    let result = Arc::new(crate::ai::face::detect_faces(manager, image)?);
    if cache.len() >= 128 {
        cache.pop_front();
    }
    cache.push_back((key, result.clone()));
    Ok(result)
}
