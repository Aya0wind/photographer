//! Versioned derived float proxy, independent of edits and display LUTs.
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
const MAX_BYTES: u64 = 64 * 1024 * 1024;
pub(super) fn path(root: &Path, source: &Path, edge: u32) -> Option<PathBuf> {
    let meta = std::fs::metadata(source).ok()?;
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let lock: serde_json::Value =
        serde_json::from_str(include_str!("../../../native/rawlab/upstream.lock.json")).ok()?;
    let identity = format!(
        "rawlab-linear-v1|{}|{}|{}|{}|{}",
        lock["commit"].as_str()?,
        source.canonicalize().ok()?.to_string_lossy(),
        meta.len(),
        modified,
        edge
    );
    Some(root.join(format!("{:x}.linear", Sha256::digest(identity.as_bytes()))))
}
pub(super) fn load(path: &Path, edge: u32) -> Option<([u32; 2], [u32; 2], Vec<f32>)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() < 56 || bytes.len() as u64 > MAX_BYTES || &bytes[..8] != b"PCLIN001" {
        return None;
    }
    let get = |o| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let original = [get(8), get(12)];
    let size = [get(16), get(20)];
    if size.contains(&0)
        || original.contains(&0)
        || size[0].max(size[1]) > edge
        || original[0] as u64 * original[1] as u64 > 128_000_000
        || size[0] > original[0]
        || size[1] > original[1]
    {
        return None;
    }
    let expected = (size[0] as usize)
        .checked_mul(size[1] as usize)?
        .checked_mul(12)?
        .checked_add(56)?;
    if expected != bytes.len() {
        return None;
    }
    let mut digest = Sha256::new();
    digest.update(&bytes[..24]);
    digest.update(&bytes[56..]);
    if &digest.finalize()[..] != &bytes[24..56] {
        return None;
    }
    let rgb: Vec<_> = bytes[56..]
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    rgb.iter()
        .all(|v| v.is_finite())
        .then_some((size, original, rgb))
}
pub(super) fn save(
    path: &Path,
    size: [u32; 2],
    original: [u32; 2],
    rgb: &[f32],
) -> Result<(), String> {
    let pixels: Vec<u8> = rgb.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut bytes = b"PCLIN001".to_vec();
    for v in original.into_iter().chain(size) {
        bytes.extend(v.to_le_bytes());
    }
    let mut digest = Sha256::new();
    digest.update(&bytes);
    digest.update(&pixels);
    bytes.extend(digest.finalize());
    bytes.extend(pixels);
    super::lut::atomic_write(path, &bytes)?;
    let mut files: Vec<_> = std::fs::read_dir(path.parent().ok_or("缓存路径无效")?)
        .map_err(|e| e.to_string())?
        .filter_map(Result::ok)
        .filter_map(|e| {
            let p = e.path();
            let m = e.metadata().ok()?;
            (p.extension()?.to_str() == Some("linear") && m.is_file()).then_some((
                m.modified().ok()?,
                m.len(),
                p,
            ))
        })
        .collect();
    files.sort_by_key(|(t, _, _)| *t);
    let mut total: u64 = files.iter().map(|(_, s, _)| *s).sum();
    for (_, s, p) in files {
        if total <= 512 * 1024 * 1024 {
            break;
        }
        if p != path && std::fs::remove_file(p).is_ok() {
            total = total.saturating_sub(s);
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn float_proxy_keeps_range_and_rejects_corruption() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("test.linear");
        let rgb = [-0.1, 0.18, 2.0];
        save(&path, [1, 1], [6000, 4000], &rgb).unwrap();
        let (size, original, restored) = load(&path, 1600).unwrap();
        assert_eq!(size, [1, 1]);
        assert_eq!(original, [6000, 4000]);
        assert_eq!(restored, rgb);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[56] ^= 1;
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path, 1600).is_none());
    }
    #[test]
    fn dimensions_are_bounded_before_pixel_allocation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("test.linear");
        save(&path, [1, 1], [1, 1], &[0.0; 3]).unwrap();
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(load(&path, 1600).is_none());
    }
}
