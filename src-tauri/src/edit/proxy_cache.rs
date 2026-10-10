//! Persistent editor proxies. Only derived pixels are written inside the database.
use crate::db::AssetRow;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

fn cache_path(database: &Path, asset: &AssetRow, edge: u32) -> PathBuf {
    let (mtime, size) = std::fs::metadata(&asset.path)
        .ok()
        .and_then(|metadata| {
            let modified: chrono::DateTime<chrono::Utc> = metadata.modified().ok()?.into();
            Some((modified.to_rfc3339(), metadata.len()))
        })
        .unwrap_or_else(|| (asset.mtime.clone(), asset.size));
    let mut key = Sha256::new();
    for part in [
        "editor-proxy-srgb-v1".to_string(),
        asset.path.clone(),
        mtime,
        size.to_string(),
        edge.to_string(),
    ] {
        key.update(part.as_bytes());
        key.update([0]);
    }
    database
        .join("editor-proxies")
        .join(format!("{:x}.jpg", key.finalize()))
}

fn read_cached(path: &Path, edge: u32) -> Option<(image::RgbImage, Vec<u8>)> {
    let length = std::fs::metadata(path).ok()?.len();
    if length == 0 || length > 16 * 1024 * 1024 {
        return None;
    }
    let jpeg = std::fs::read(path).ok()?;
    let pixels = crate::thumbs::jpeg_scaled_bytes(&jpeg, edge as u16)?;
    Some((pixels, jpeg))
}

pub(super) fn get_or_create(
    database: &Path,
    asset: &AssetRow,
    edge: u32,
    build: impl FnOnce() -> Option<image::RgbImage>,
) -> Option<(image::RgbImage, Vec<u8>)> {
    // One editor opens at a time; merge concurrent opens of the same proxy.
    static CREATION: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = CREATION.get_or_init(|| Mutex::new(())).lock().ok()?;
    let target = cache_path(database, asset, edge);
    if let Some(cached) = read_cached(&target, edge) {
        return Some(cached);
    }
    let pixels = build()?;
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95)
        .encode(
            pixels.as_raw(),
            pixels.width(),
            pixels.height(),
            image::ExtendedColorType::Rgb8,
        )
        .ok()?;
    let temporary = target.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let stored = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(target.parent().unwrap())?;
        std::fs::write(&temporary, &jpeg)?;
        std::fs::rename(&temporary, &target)
    })();
    if let Err(error) = stored {
        let _ = std::fs::remove_file(&temporary);
        crate::devices::diagnostics::record(format!("editor proxy cache write failed: {error}"));
    }
    // First and subsequent opens use identical cached pixels, not a different
    // pre-encoding image on the first visit.
    let pixels = crate::thumbs::jpeg_scaled_bytes(&jpeg, edge as u16)?;
    Some((pixels, jpeg))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn asset(path: &Path) -> AssetRow {
        let metadata = std::fs::metadata(path).unwrap();
        let modified: chrono::DateTime<chrono::Utc> = metadata.modified().unwrap().into();
        serde_json::from_value(
            serde_json::json!({"path":path.to_string_lossy(),"filename":"photo.raw",
            "size":metadata.len(),"mtime":modified.to_rfc3339(),"xxhash":0,"kind":"raw",
            "source":"test","createdAt":modified.to_rfc3339()}),
        )
        .unwrap()
    }
    #[test]
    fn proxy_is_reused_and_source_changes_or_corruption_regenerate_it() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original.raw");
        std::fs::write(&original, b"original").unwrap();
        let row = asset(&original);
        let first = get_or_create(directory.path(), &row, 32, || {
            Some(image::RgbImage::from_pixel(
                32,
                24,
                image::Rgb([30, 90, 160]),
            ))
        })
        .unwrap();
        let cached = get_or_create(directory.path(), &row, 32, || {
            panic!("cache hit must not regenerate the proxy")
        })
        .unwrap();
        assert_eq!(first.1, cached.1);
        assert_eq!(first.0, cached.0);
        let cache = cache_path(directory.path(), &row, 32);
        assert!(cache.is_file());
        assert_eq!(std::fs::read(&original).unwrap(), b"original");
        std::fs::write(&cache, b"corrupt").unwrap();
        assert!(get_or_create(directory.path(), &row, 32, || Some(first.0.clone())).is_some());
        assert_ne!(std::fs::read(&cache).unwrap(), b"corrupt");
        std::fs::write(&original, b"changed original with different size").unwrap();
        assert_ne!(cache_path(directory.path(), &row, 32), cache);
        let mut rebuilt = false;
        get_or_create(directory.path(), &row, 32, || {
            rebuilt = true;
            Some(first.0)
        })
        .unwrap();
        assert!(rebuilt);
    }
}
