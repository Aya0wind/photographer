//! 同卷判定（M8-① rename 快道前置）：按**卷序列号**（GetVolumeInformationW）
//! 而非盘符——junction/盘符映射/目录联接会让盘符比较骗人（两个盘符指向
//! 同一物理卷，或同盘符经 subst 指向别处）；序列号是文件系统实例级唯一。
//!
//! 任一端取不到序列号（UNC 怪异形态/权限/非 Windows）→ 判定为**不同卷**
//! （回退流式复制，宁慢勿错）。结果按根路径缓存（每卷一次系统调用）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 卷序列号缓存（根路径 → 序列号；None 也缓存 = 该根取不到）。
fn serial_cache() -> &'static Mutex<HashMap<PathBuf, Option<u64>>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<PathBuf, Option<u64>>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 路径所在卷的序列号（根路径缓存）。取不到 → None。
fn volume_serial(path: &Path) -> Option<u64> {
    let mut root = path;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    let root = root.to_path_buf();
    let mut cache = serial_cache().lock().expect("volume serial cache poisoned");
    if let Some(hit) = cache.get(&root) {
        return *hit;
    }
    let serial = crate::platform::query_volume_serial(&root);
    cache.insert(root, serial);
    serial
}

/// 两路径是否同一卷（双方序列号可得且相等）。任一失败 = false。
pub fn same_volume(a: &Path, b: &Path) -> bool {
    match (volume_serial(a), volume_serial(b)) {
        (Some(sa), Some(sb)) => sa == sb,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_dir_and_ancestors_agree() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("x.jpg");
        let b = tmp.path().join("sub").join("y.jpg");
        // 同一 tempdir 树 → 同卷（Windows 上序列号可得）
        if cfg!(windows) {
            assert!(same_volume(tmp.path(), &b));
            let _ = a;
        }
    }

    #[test]
    fn missing_paths_fall_back_to_not_same() {
        // 不存在的根：GetVolumeInformationW 失败 → false（宁慢勿错）
        assert!(!same_volume(
            Path::new(r"\\?\NonexistentVolume\{:}\x"),
            Path::new(".")
        ));
    }
}
