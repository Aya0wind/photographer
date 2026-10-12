//! photo_library_remove（移除登记）语义测试（2026-10-09 §一/§七）：
//! 永不删照片文件（用户红线）；delete_records=true 连库内资产记录一并删；
//! 记录删除数如实返回；不存在的库明确报错。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

use std::path::Path;
use std::time::Duration;

use ipc::photo_library::fetch_photo_library_remove;

/// 登记一个照片库（root 已在盘）并放两张照片 + 两条资产记录。
fn library_with_assets(
    state: &ipc::AppState,
    root: &Path,
) -> (String, Vec<std::path::PathBuf>) {
    std::fs::create_dir_all(root.join("2026/06")).unwrap();
    let files = vec![root.join("2026/06/DSC_1.jpg"), root.join("2026/06/DSC_2.jpg")];
    for file in &files {
        std::fs::write(file, b"jpeg").unwrap();
    }
    let app_db = common::open_db(&state.config_dir);
    if app_db.photos_library_list().unwrap().is_empty(){app_db.photos_library_register("保留库",&root.with_file_name("retained-library").to_string_lossy(),&state.config_dir).unwrap();}
    let row = app_db
        .photos_library_register("删除目标", &root.to_string_lossy(), &state.config_dir)
        .unwrap();
    for (i, file) in files.iter().enumerate() {
        app_db
            .0
            .execute(
                "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
                 created_at, origin, library_id) \
                 VALUES (?1, ?2, 1, '2026-01-01T00:00:00Z', ?3, 'photo', 'imported', \
                 '2026-01-01T00:00:00Z', 'imported', ?4)",
                rusqlite::params![
                    file.to_string_lossy().to_string(),
                    file.file_name().unwrap().to_string_lossy().to_string(),
                    i as i64,
                    row.id
                ],
            )
            .unwrap();
    }
    (row.id, files)
}

#[test]
fn remove_deletes_records_but_keeps_files_by_default() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let photos = tempfile::tempdir().unwrap();
    let (id, files) = library_with_assets(&state, photos.path());

    // 2026-10-11 定案两分支：删除照片库 = 记录/派生数据一并删，默认不动文件
    let result = fetch_photo_library_remove(&state, &id, false).unwrap();
    assert_eq!(result.records_deleted, 2, "连带删除的记录数如实返回");
    assert_eq!(result.files_deleted, 0);
    for file in &files {
        assert!(file.is_file(), "默认不动物理文件: {}", file.display());
    }
    let app_db = common::open_db(&state.config_dir);
    let left: i64 = app_db
        .0
        .query_row("SELECT COUNT(*) FROM assets WHERE library_id = ?1", [&id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(left, 0, "资产记录已清");
    assert!(app_db.photos_library_get(&id).unwrap().is_none());
}

#[test]
fn unknown_library_is_an_error() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let err = fetch_photo_library_remove(&state, "no-such-library", false).unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}

// ---------------------------------------------------------------------------
// M2c（§五）：离线库（外置卷拔出）移除登记照常——remove 本就不碰任何
// 文件，status 不构成障碍；连记录删也不需要库在线。
// ---------------------------------------------------------------------------

#[test]
fn remove_offline_library_works_without_touching_files() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let photos = tempfile::tempdir().unwrap();
    let (id, files) = library_with_assets(&state, photos.path());
    // 模拟外置卷拔出：整库 offline（root 仍在盘——status 是唯一真相）
    let app_db = common::open_db(&state.config_dir);
    app_db.photos_library_set_status(&id, "offline").unwrap();

    // 连记录删：离线状态不构成障碍（纯 DB 操作），文件照旧永不删
    let result = fetch_photo_library_remove(&state, &id, false).unwrap();
    assert_eq!(result.records_deleted, 2);
    for file in &files {
        assert!(file.is_file(), "离线库移除同样永不删照片文件");
    }
    assert!(app_db.photos_library_get(&id).unwrap().is_none());
}

// ---------------------------------------------------------------------------
// 悬挂清扫 + 簇心缓存失效（2026-10-09 审计 P0/P1）：删库收尾清扫
// library_id 悬挂的幽灵资产（竞态窗口写出的行）连派生数据一并清理；
// purge_asset_derivations 四路共用入口补齐人脸簇心缓存整库失效。
// ---------------------------------------------------------------------------

#[test]
fn remove_sweeps_dangling_library_assets_and_invalidates_face_cluster_cache() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let photos = tempfile::tempdir().unwrap();
    let (id, files) = library_with_assets(&state, photos.path());

    // 幽灵资产行：library_id 指向从未登记（或已被并发删除）的库——扫描/
    // 导入/联拍竞态窗口写出的形态
    let ghost_file = photos.path().join("2026/06/GHOST.jpg");
    std::fs::write(&ghost_file, b"jpeg-ghost").unwrap();
    let app_db = common::open_db(&state.config_dir);
    app_db
        .0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
             created_at, origin, library_id) \
             VALUES (?1, 'GHOST.jpg', 1, '2026-01-01T00:00:00Z', 99, 'photo', 'imported', \
             '2026-01-01T00:00:00Z', 'imported', 'ghost-lib')",
            [ghost_file.to_string_lossy().to_string()],
        )
        .unwrap();
    // 幽灵行的缩略图缓存（清扫应连缓存一起清）
    let cache = thumbs::cache_path(
        &state.config_dir,
        &ghost_file,
        256,
        std::fs::metadata(&ghost_file).unwrap().modified().unwrap(),
    );
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    std::fs::write(&cache, b"thumb").unwrap();
    // 人脸簇心缓存预置（观测 purge_asset_derivations 的失效语义）
    ai::face::seed_cluster_cache_for_test(&state.config_dir);
    assert!(ai::face::cluster_cache_present(&state.config_dir));

    let result = fetch_photo_library_remove(&state, &id, false).unwrap();
    assert_eq!(result.records_deleted, 2, "只计在删库的真实记录");

    let app_db = common::open_db(&state.config_dir);
    let ghost_left: i64 = app_db
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE library_id NOT IN \
             (SELECT id FROM photos_libraries)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(ghost_left, 0, "悬挂 library_id 幽灵资产被清扫");
    assert!(!cache.is_file(), "幽灵行的缩略图缓存随行清理");
    assert!(
        !ai::face::cluster_cache_present(&state.config_dir),
        "purge_asset_derivations 应整库失效人脸簇心缓存"
    );
    for file in &files {
        assert!(file.is_file(), "物理文件照旧不动");
    }
}
