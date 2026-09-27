//! library_delete（删除库）安全闸与物理删除测试。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::path::Path;
use std::time::Duration;

use ipc::settings::fetch_library_delete;

fn make_library_dir(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("library.db"), b"sqlite").unwrap();
    std::fs::create_dir_all(dir.join("thumbs")).unwrap();
    dir
}

#[test]
fn refuses_dir_without_library_db() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    // 空临时目录（无 library.db）拒绝
    let target = tempfile::tempdir().unwrap();
    let err = fetch_library_delete(&state, target.path().to_str().unwrap(), None).unwrap_err();
    assert!(err.contains("library.db"), "{err}");
}

#[test]
fn refuses_active_library() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    std::fs::write(db_dir.path().join("library.db"), b"sqlite").unwrap();
    let err = fetch_library_delete(&state, db_dir.path().to_str().unwrap(), None).unwrap_err();
    assert!(err.contains("活跃库"), "{err}");
}

#[test]
fn deletes_db_and_optional_photo_root() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let target = make_library_dir(db_dir.path(), "libdata-a");
    let photos = tempfile::tempdir().unwrap();
    std::fs::write(photos.path().join("DSC_1.ARW"), b"x").unwrap();

    // 只删库数据：照片目录保留
    let r = fetch_library_delete(&state, target.to_str().unwrap(), None).unwrap();
    assert!(r.db_deleted && !r.photo_root_deleted);
    assert!(!target.exists());
    assert!(
        photos.path().join("DSC_1.ARW").is_file(),
        "未勾选时照片目录不动"
    );

    // 连照片目录：一起删
    let target2 = make_library_dir(db_dir.path(), "libdata-b");
    let photos2 = tempfile::tempdir().unwrap();
    std::fs::write(photos2.path().join("a.jpg"), b"x").unwrap();
    let r2 = fetch_library_delete(
        &state,
        target2.to_str().unwrap(),
        Some(photos2.path().to_str().unwrap()),
    )
    .unwrap();
    assert!(r2.db_deleted && r2.photo_root_deleted);
    assert!(!target2.exists() && !photos2.path().exists());
}

#[test]
fn refuses_drive_root_and_same_dir_before_any_deletion() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let target = make_library_dir(db_dir.path(), "libdata-c");

    // 盘根拒绝（先校验后动手：目标库目录必须完好）
    let err = fetch_library_delete(&state, target.to_str().unwrap(), Some("C:\\")).unwrap_err();
    assert!(err.contains("盘根"), "{err}");
    assert!(target.join("library.db").is_file(), "校验失败不得动手");

    // 照片目录 == 库目录拒绝
    let err2 = fetch_library_delete(
        &state,
        target.to_str().unwrap(),
        Some(target.to_str().unwrap()),
    )
    .unwrap_err();
    assert!(err2.contains("相同"), "{err2}");
    assert!(target.join("library.db").is_file());
}
