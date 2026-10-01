//! Download receipts. Files are removed only after the library acknowledges
//! ingestion; disconnect only removes empty directories.
use super::backend::{CapturedObject, TetherError};
use std::io::Write;
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
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(io_error)?;
    Ok(CapturedObject {
        object_id: target.to_string_lossy().into_owned(),
        object_name: name.into(),
        object_size: bytes.len() as u64,
    })
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
