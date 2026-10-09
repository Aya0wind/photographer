//! photo_library_relocate（照片库整体重定位，2026-10-09 §一「无移库」模型）：
//! 改登记 root + 库内路径前缀批量重写（资产归属不变）；apply=false 只预检
//! 返回计数；root 互斥校验（与既有库/数据库目录重叠拒绝）；新根可先登记
//! 后挂载（root_exists 如实上报）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

use std::path::Path;
use std::time::Duration;

use common::state_with_library;
use ipc::photo_library::fetch_photo_library_relocate;

fn row(path: &str, library_id: &str) -> db::AssetRow {
    db::AssetRow {
        path: path.into(),
        filename: Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned(),
        size: 1,
        mtime: "2026-10-09T00:00:00.000Z".into(),
        xxhash: 1,
        kind: events::AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-10-09T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 1,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
        library_id: Some(library_id.into()),
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    }
}

/// 登记 photo root + 两条库内资产 + 一条库外资产（unaffected 判定用）。
fn setup_library(state: &ipc::AppState, root: &Path, outside: &Path) -> String {
    std::fs::create_dir_all(root.join("2026/06")).unwrap();
    let app_db = common::open_db(&state.config_dir);
    let lib = app_db
        .photos_library_register("主库", &root.to_string_lossy(), &state.config_dir)
        .unwrap();
    app_db
        .insert_asset(&row(
            &root.join("2026/06/DSC_1.jpg").to_string_lossy(),
            &lib.id,
        ))
        .unwrap();
    app_db
        .insert_asset(&row(
            &root.join("2026/06/sub/DSC_2.jpg").to_string_lossy(),
            &lib.id,
        ))
        .unwrap();
    // 库外路径（历史遗留/外部登记）→ unaffected
    app_db.insert_asset(&row(&outside.join("elsewhere.jpg").to_string_lossy(), &lib.id)).unwrap();
    lib.id
}

#[test]
fn relocate_rewrites_prefix_and_keeps_ownership() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let old_root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let id = setup_library(&state, old_root.path(), outside.path());

    // 用户已在文件管理器把整个照片树搬到新根（模拟）
    let new_root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(new_root.path().join("2026/06/sub")).unwrap();

    // 预检：affected=2（库内前缀命中），unaffected=1（库外）
    let inspect = fetch_photo_library_relocate(
        &state,
        &id,
        &new_root.path().to_string_lossy(),
        false,
    )
    .unwrap();
    assert_eq!(inspect.affected, 2);
    assert_eq!(inspect.unaffected, 1);
    assert!(inspect.root_exists, "新根在盘");

    // 执行：root 改登记 + 路径前缀重写（归属 library_id 不变）
    let applied = fetch_photo_library_relocate(
        &state,
        &id,
        &new_root.path().to_string_lossy(),
        true,
    )
    .unwrap();
    assert_eq!(applied.affected, 2);
    let app_db = common::open_db(&state.config_dir);
    let lib = app_db.photos_library_get(&id).unwrap().unwrap();
    assert_eq!(
        lib.root_path,
        new_root.path().to_string_lossy().to_string(),
        "登记 root 已更新"
    );
    let paths: Vec<String> = app_db
        .0
        .prepare("SELECT path FROM assets WHERE library_id = ?1 ORDER BY path")
        .unwrap()
        .query_map([&id], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(paths.len(), 3);
    assert!(paths.iter().all(|p| !p.contains(&old_root.path().to_string_lossy().to_string())));
    assert!(paths.iter().any(|p| *p == new_root.path().join("2026/06/DSC_1.jpg").to_string_lossy()));
    assert!(paths
        .iter()
        .any(|p| *p == new_root.path().join("2026/06/sub/DSC_2.jpg").to_string_lossy()));
    assert!(paths
        .iter()
        .any(|p| *p == outside.path().join("elsewhere.jpg").to_string_lossy()),
        "库外路径不动（unaffected）");
}

#[test]
fn relocate_to_overlapping_root_is_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let root_a = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let id = setup_library(&state, root_a.path(), outside.path());
    let other_id = setup_library(&state, outside.path(), root_a.path());

    // 新根与**另一库**互相包含 → 拒绝（§八-6 root 互斥；自身旧根不参与）
    let inside_other = outside.path().join("nested");
    let err = fetch_photo_library_relocate(&state, &id, &inside_other.to_string_lossy(), true).unwrap_err();
    assert!(err.contains("互相包含") || err.contains("相同"), "{err}");
    // 重定位到自身当前路径 = 幂等 no-op：apply 后 root 与资产路径原样
    fetch_photo_library_relocate(
        &state,
        &other_id,
        &outside.path().to_string_lossy(),
        true,
    )
    .unwrap();
    let app_db = common::open_db(&state.config_dir);
    assert_eq!(
        app_db.photos_library_get(&other_id).unwrap().unwrap().root_path,
        outside.path().to_string_lossy().to_string(),
        "同路径 apply 不改登记"
    );

    // 新根与数据库目录重叠 → 拒绝
    let err2 =
        fetch_photo_library_relocate(&state, &id, &db_dir.path().to_string_lossy(), true).unwrap_err();
    assert!(err2.contains("数据库目录"), "{err2}");
}

#[test]
fn relocate_reports_missing_root_without_failing() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let id = setup_library(&state, root.path(), outside.path());

    // 可先改后挂载：新根不在盘 → apply 成功但 root_exists=false（缺失走 offline）
    let mount_later = tempfile::tempdir().unwrap();
    let missing = mount_later.path().join("photos");
    std::fs::remove_dir(mount_later.path()).unwrap(); // 整个挂载点暂缺
    let applied = fetch_photo_library_relocate(&state, &id, &missing.to_string_lossy(), true).unwrap();
    assert!(!applied.root_exists, "新根尚未挂载");
    let app_db = common::open_db(&state.config_dir);
    let lib = app_db.photos_library_get(&id).unwrap().unwrap();
    assert_eq!(lib.root_path, missing.to_string_lossy().to_string());
}

#[test]
fn relocate_unknown_library_is_an_error() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let err = fetch_photo_library_relocate(&state, "no-such", src.path().to_str().unwrap(), false)
        .unwrap_err();
    assert!(err.contains("不存在"), "{err}");
}
