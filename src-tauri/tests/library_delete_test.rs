//! library_delete（删除库）安全闸与物理删除测试。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, migrate, platform,
    settings, tasks, thumbs,
};

use std::path::Path;
use std::time::Duration;

use ipc::settings::fetch_library_delete;

fn register_for_delete(state: &ipc::AppState, database: &Path, photos: &Path) {
    let mut settings = state.settings.lock().unwrap();
    settings.libraries.push(settings::Library {
        id: database.to_string_lossy().into_owned(),
        name: "Delete target".into(),
        db_dir: database.to_string_lossy().into_owned(),
        photo_root: photos.to_string_lossy().into_owned(),
        ..settings::Library::default()
    });
}

fn make_library_dir(root: &Path, name: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    drop(db::Db::open_migrated(&dir.join("library.db")).unwrap());
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
    register_for_delete(&state, &target, photos.path());
    std::fs::write(photos.path().join("DSC_1.ARW"), b"x").unwrap();

    // 只删库数据：照片目录保留
    let r = fetch_library_delete(&state, target.to_str().unwrap(), None).unwrap();
    assert!(r.db_deleted && !r.photo_root_deleted);
    assert!(!target.exists());
    assert!(
        photos.path().join("DSC_1.ARW").is_file(),
        "未勾选时照片目录不动"
    );

    // 勾选只删纳管文件；仅索引原件和用户自己的文件均保留。
    let target2 = make_library_dir(db_dir.path(), "libdata-b");
    let photos2 = tempfile::tempdir().unwrap();
    register_for_delete(&state, &target2, photos2.path());
    let managed = photos2.path().join("a.jpg");
    let referenced = photos2.path().join("b.jpg");
    let referenced_outside = src.path().join("outside.jpg");
    let mislabeled_outside = src.path().join("wrongly-imported.jpg");
    let unrelated = photos2.path().join("notes.txt");
    std::fs::write(&managed, b"x").unwrap();
    std::fs::write(&referenced, b"y").unwrap();
    std::fs::write(&referenced_outside, b"outside").unwrap();
    std::fs::write(&mislabeled_outside, b"outside too").unwrap();
    std::fs::write(&unrelated, b"notes").unwrap();
    let db = db::Db::open_migrated(&target2.join("library.db")).unwrap();
    for (path, origin) in [
        (&managed, "imported"),
        (&referenced, "external"),
        (&referenced_outside, "external"),
        (&mislabeled_outside, "imported"),
    ] {
        db.0.execute(
            "INSERT INTO assets (path,filename,size,mtime,xxhash,kind,source,created_at,origin) \
             VALUES (?1,?2,1,'2026-01-01T00:00:00Z',1,'photo','imported','2026-01-01T00:00:00Z',?3)",
            rusqlite::params![path.to_string_lossy().to_string(), path.file_name().unwrap().to_string_lossy().to_string(), origin],
        ).unwrap();
    }
    drop(db);
    let r2 = fetch_library_delete(
        &state,
        target2.to_str().unwrap(),
        Some(photos2.path().to_str().unwrap()),
    )
    .unwrap();
    assert!(r2.db_deleted && !r2.photo_root_deleted);
    assert_eq!(r2.managed_files_deleted, 1);
    assert!(!target2.exists() && photos2.path().exists());
    assert!(!managed.exists() && referenced.exists() && unrelated.exists());
    assert!(referenced_outside.exists() && mislabeled_outside.exists());
}

#[test]
fn unknown_file_in_database_directory_blocks_recursive_delete() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let target = make_library_dir(db_dir.path(), "libdata-extra");
    std::fs::write(target.join("personal.txt"), b"keep").unwrap();
    let err = fetch_library_delete(&state, target.to_str().unwrap(), None).unwrap_err();
    assert!(err.contains("其他文件"), "{err}");
    assert!(target.join("personal.txt").exists());
}

#[test]
fn refuses_drive_root_and_same_dir_before_any_deletion() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let target = make_library_dir(db_dir.path(), "libdata-c");
    register_for_delete(&state, &target, src.path());

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
