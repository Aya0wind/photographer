//! Download receipts. Files are removed only after the library acknowledges
//! ingestion; disconnect only removes empty directories.
use super::backend::{CapturedObject, TetherError};
use std::io::Write;
use std::collections::HashMap;
use std::sync::Mutex;
use std::path::{Path, PathBuf};

pub(super) fn persist(
    root: &Path,
    name: &str,
    bytes: &[u8],
) -> Result<CapturedObject, TetherError> {
    if bytes.is_empty()
        || name.is_empty()
        || name == "."
        || name == ".."
        || name.contains(['/', '\\', '\0'])
    {
        return Err(TetherError::Other("相机文件名或内容无效".into()));
    }
    let directory = root.join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir(&directory).map_err(io_error)?;
    let target = directory.join(name);
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&target)
        .map_err(io_error)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        drop(file);
        let _ = std::fs::remove_file(&target);
        let _ = std::fs::remove_dir(&directory);
        return Err(io_error(error));
    }
    drop(file);
    let object = CapturedObject {
        object_id: target.to_string_lossy().into_owned(),
        object_name: name.into(),
        object_size: bytes.len() as u64,
    };
    if let Err(error) = seal(root, &object) {
        crate::devices::diagnostics::record(format!("完整下载收据写入失败，原文件保留: {error}"));
    }
    Ok(object)
}

pub(super) fn retire_incomplete(ledger: &super::receipts_store::ReceiptsStore) -> Vec<super::receipts_store::UnackedReceipt> {
    ledger.unacked().into_iter().filter(|receipt| {
        if has_complete_receipt(&receipt.backend, &receipt.id) { return true; }
        ledger.record_lost(&receipt.id, "传输未完成或完整文件已丢失，不再恢复", &chrono::Utc::now().to_rfc3339());
        false
    }).collect()
}

pub(super) fn has_complete_receipt(backend: &str, id: &str) -> bool {
    let directory = match backend {
        "sony-sdk" => "Photographer-tethering",
        "gphoto" => "Photographer-gphoto",
        _ => return false, // A remote pending receipt is a transfer already started.
    };
    let Ok(base) = std::fs::canonicalize(std::env::temp_dir().join(directory)) else { return false; };
    let Ok(path) = std::fs::canonicalize(id) else { return false; };
    path.starts_with(base) && std::fs::read(manifest_path(&path)).ok()
        .and_then(|data| serde_json::from_slice::<CapturedObject>(&data).ok())
        .is_some_and(|object| object.object_id == id && object.object_size > 0
            && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == object.object_size))
}

fn manifest_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".photographer-receipt.json");
    name.into()
}

/// Only a completed backend download may create a recoverable receipt.
pub(super) fn seal(root: &Path, object: &CapturedObject) -> Result<(), TetherError> {
    let root = std::fs::canonicalize(root).map_err(io_error)?;
    let path = std::fs::canonicalize(&object.object_id).map_err(io_error)?;
    if !path.starts_with(&root) || !path.is_file() || object.object_size == 0
        || std::fs::metadata(&path).map_err(io_error)?.len() != object.object_size {
        return Err(TetherError::Other("拍摄照片传输不完整".into()));
    }
    let data = serde_json::to_vec(object).map_err(|e| TetherError::Other(e.to_string()))?;
    let mut file = std::fs::File::create(manifest_path(&path)).map_err(io_error)?;
    file.write_all(&data).and_then(|_| file.sync_all()).map_err(io_error)
}

#[derive(Default)]
pub(super) struct Recovery {
    roots: Mutex<HashMap<String, PathBuf>>,
    queue: Mutex<Vec<CapturedObject>>,
}

impl Recovery {
    pub fn request(&self, current: &Path, ids: &[String]) -> Vec<String> {
        let Some(base) = current.parent().and_then(|base| std::fs::canonicalize(base).ok()) else {
            return Vec::new();
        };
        let mut roots = self.roots.lock().unwrap();
        let mut queue = self.queue.lock().unwrap();
        let mut restored = Vec::new();
        for id in ids {
            if roots.contains_key(id) { restored.push(id.clone()); continue; }
            let Ok(path) = std::fs::canonicalize(id) else { continue; };
            // Ledger IDs are trusted, but still constrain paths to this backend's
            // staging tree; canonicalization prevents symlink/path traversal.
            if !path.starts_with(&base) || !path.is_file() { continue; }
            let Some(session_root) = path.strip_prefix(&base).ok().and_then(|relative|
                relative.components().next().map(|first| base.join(first.as_os_str()))) else { continue; };
            let object = std::fs::read(manifest_path(&path)).ok()
                .and_then(|data| serde_json::from_slice::<CapturedObject>(&data).ok());
            let Some(object) = object.filter(|object| object.object_id == *id
                && object.object_size > 0
                && std::fs::metadata(&path).is_ok_and(|m| m.len() == object.object_size)) else {
                // Legacy files without manifests cannot be verified; preserve
                // them for manual import instead of assuming they are partial.
                continue;
            };
            roots.insert(id.clone(), session_root);
            restored.push(id.clone());
            queue.push(object);
        }
        restored
    }

    pub fn take(&self) -> Vec<CapturedObject> {
        std::mem::take(&mut *self.queue.lock().unwrap())
    }

    pub fn root(&self, current: &Path, object: &CapturedObject) -> PathBuf {
        self.roots.lock().unwrap().get(&object.object_id).cloned().unwrap_or_else(|| current.into())
    }
}

pub(super) fn validated_path(root: &Path, object: &CapturedObject) -> Result<PathBuf, TetherError> {
    let root = std::fs::canonicalize(root).map_err(io_error)?;
    let path = std::fs::canonicalize(&object.object_id).map_err(io_error)?;
    // A receipt lives exactly one UUID directory beneath the session root.
    if path.parent().and_then(Path::parent) != Some(root.as_path()) || !path.is_file() {
        return Err(TetherError::AccessDenied);
    }
    Ok(path)
}

pub(super) fn acknowledge(root: &Path, object: &CapturedObject) -> Result<(), TetherError> {
    let path = validated_path(root, object)?;
    std::fs::remove_file(&path).map_err(io_error)?;
    let _ = std::fs::remove_file(manifest_path(&path));
    if let Some(parent) = path.parent() {
        let _ = std::fs::remove_dir(parent);
    }
    Ok(())
}

pub(super) fn disconnect(root: &Path) {
    // Nonempty directories include downloads not acknowledged by the library.
    // Never recursively remove them, including after a partial write.
    if root.exists() && std::fs::remove_dir(root).is_err() {
        eprintln!(
            "联拍未入库文件已保留，请从文件夹恢复导入: {}",
            root.display()
        );
    }
}

fn io_error(error: std::io::Error) -> TetherError {
    TetherError::Other(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_downloads_do_not_overwrite_and_disconnect_keeps_unacknowledged() {
        let dir = tempfile::tempdir().unwrap();
        let first = persist(dir.path(), "照片.jpg", b"first").unwrap();
        let second = persist(dir.path(), "照片.jpg", b"second").unwrap();
        assert_ne!(first.object_id, second.object_id);
        acknowledge(dir.path(), &first).unwrap();
        disconnect(dir.path());
        assert!(!Path::new(&first.object_id).exists());
        assert_eq!(std::fs::read(&second.object_id).unwrap(), b"second");
    }

    #[test]
    fn rejects_path_traversal_and_external_receipts() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["../a.jpg", "/a.jpg", "a\\b.jpg", ".", ""] {
            assert!(persist(dir.path(), name, b"photo").is_err());
        }
        let other = tempfile::tempdir().unwrap();
        let receipt = persist(other.path(), "a.jpg", b"keep").unwrap();
        assert!(acknowledge(dir.path(), &receipt).is_err());
        assert_eq!(std::fs::read(receipt.object_id).unwrap(), b"keep");
    }
}
