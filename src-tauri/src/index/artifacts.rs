//! Bounded native analysis artifacts shared by selection and face consumers.
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use crate::ai::face::DetectedFace;
use crate::db::Db;

const IMAGE_BYTES: usize = 64 * 1024 * 1024;

type ImageCell = Arc<OnceLock<Option<Arc<image::RgbImage>>>>;
type ImageEntry = (String, ImageCell, Instant);
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
    let physical = std::fs::metadata(&source)
        .ok()
        .and_then(|m| m.modified().ok().map(|time| (m.len(), time)));
    Some(format!(
        "{}:{source}:{size}:{mtime}:{physical:?}:analysis-1024-v1",
        path.display()
    ))
}

pub fn analysis(db: &Db, path: &Path, asset: i64) -> Option<Arc<image::RgbImage>> {
    let key = source_key(db, path, asset)?;
    let cell = {
        let mut cache = images().lock().ok()?;
        if let Some(position) = cache.iter().position(|(candidate, _, _)| *candidate == key) {
            let entry = cache.remove(position)?;
            let cell = entry.1.clone();
            cache.push_back((entry.0, cell.clone(), Instant::now()));
            cell
        } else {
            // Each analysis image is at most 1024*1024 RGB bytes.
            while cache.len() >= IMAGE_BYTES / (1024 * 1024 * 3) {
                cache.pop_front();
            }
            let cell = Arc::new(OnceLock::new());
            cache.push_back((key.clone(), cell.clone(), Instant::now()));
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
            // The producer caches against disk mtime. Camera/import ledger timestamps
            // can differ from the final file's time; do not wait for a nonexistent key.
            let file = if source.exists() {
                crate::thumbs::cached(path, &source, 1024)
            } else {
                crate::thumbs::cached_for_asset(path, &source, 1024, &mtime)
            }?;
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
            .retain(|(candidate, entry, _)| candidate != &key || !Arc::ptr_eq(entry, &cell));
    }
    result
}

/// Rebuilding an unchanged source must also discard decoded artifacts.
pub fn invalidate(path: &Path) {
    let prefix = format!("{}:", path.display());
    if let Ok(mut cache) = images().lock() {
        cache.retain(|(key, _, _)| !key.starts_with(&prefix));
    }
    if let Ok(mut cache) = detections().lock() {
        cache.retain(|(key, _, _)| !key.starts_with(&prefix));
    }
}

/// Drop cache references only; active consumers keep their own Arc alive.
pub fn release_idle() {
    const TTL: Duration = Duration::from_secs(30);
    let mut removed = 0;
    if let Ok(mut cache) = images().try_lock() {
        let before = cache.len();
        cache.retain(|(_, _, touched)| touched.elapsed() < TTL);
        removed += before - cache.len();
    }
    if let Ok(mut cache) = detections().try_lock() {
        let before = cache.len();
        cache.retain(|(_, _, touched)| touched.elapsed() < TTL);
        removed += before - cache.len();
    }
    if removed > 0 {
        log::info!("索引空闲缓存回收: {removed} 项");
    }
}

type DetectionEntry = (String, Arc<Vec<DetectedFace>>, Instant);
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
    if let Some((_, result, touched)) = cache.iter_mut().find(|(candidate, _, _)| *candidate == key)
    {
        *touched = Instant::now();
        return Ok(result.clone());
    }
    let result = Arc::new(crate::ai::face::detect_faces(manager, image)?);
    if cache.len() >= 128 {
        cache.pop_front();
    }
    cache.push_back((key, result.clone(), Instant::now()));
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_eviction_drops_cache_ownership_without_invalidating_active_consumers() {
        let key = format!("idle-test-{}", uuid::Uuid::new_v4());
        let image = Arc::new(image::RgbImage::new(4, 4));
        let cell: ImageCell = Arc::new(OnceLock::new());
        cell.set(Some(image.clone())).unwrap();
        images().lock().unwrap().push_back((
            key.clone(),
            cell.clone(),
            Instant::now() - Duration::from_secs(60),
        ));
        drop(cell);
        assert_eq!(Arc::strong_count(&image), 2);
        release_idle();
        assert_eq!(Arc::strong_count(&image), 1);
        assert_eq!(image.dimensions(), (4, 4));
        assert!(!images().lock().unwrap().iter().any(|entry| entry.0 == key));
    }

    #[test]
    fn analysis_uses_producer_cache_despite_ledger_time_and_invalidates_disk_changes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("library.db")).unwrap();
        let source = dir.path().join("received.jpg");
        std::fs::write(&source, [1]).unwrap();
        db.0.execute("INSERT INTO assets(id,path,filename,size,mtime,xxhash,kind,source,created_at) VALUES (1,?1,'received.jpg',1,'2026-01-01T00:00:00.000Z',0,'photo','tether','now')", [source.to_string_lossy().as_ref()]).unwrap();
        let modified = std::fs::metadata(&source).unwrap().modified().unwrap();
        let cache = crate::thumbs::cache_path(dir.path(), &source, 1024, modified);
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        image::RgbImage::from_pixel(32, 24, image::Rgb([100, 120, 140]))
            .save(&cache)
            .unwrap();
        assert_eq!(analysis(&db, dir.path(), 1).unwrap().dimensions(), (32, 24));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&source)
            .unwrap();
        file.set_times(
            std::fs::FileTimes::new().set_modified(modified + std::time::Duration::from_secs(5)),
        )
        .unwrap();
        assert!(analysis(&db, dir.path(), 1).is_none());
    }
}
