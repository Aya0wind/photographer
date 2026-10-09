//! 子组逻辑化（2026-10-09 §一定案，取代 0022 物理化）：subgroup 回归纯
//! DB 标记——存储布局唯一公式 = 纯时间 `{库root}/{拍摄年}/{拍摄月}/
//! {原文件名}`，子组段不再物理存在。覆盖：导入带子组零物理足迹（引用
/// 带 subgroup）、移组三向纯引用改写（根↔组A↔组B：零文件操作 + 路径
/// 不改写 + 回收站不在此拦截层）、claim 带子组只建引用。

mod common;

use common::library_fixture as setup;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

use std::path::Path;

use common::{open_db, run_engine};
use db::AssetRow;
use events::AssetKind;
use ipc::album::fetch_album_item_move_subgroup;
use ipc::claim::fetch_album_claim_assets;

fn ins(db: &db::Db, path: &Path, captured: Option<&str>) -> i64 {
    let path_str = path.to_string_lossy().into_owned();
    db.insert_asset(&AssetRow {
        filename: path_str.rsplit(['\\', '/']).next().unwrap_or(&path_str).to_string(),
        path: path_str.clone(),
        size: 100,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: 42,
        kind: AssetKind::Photo,
        captured_at: captured.map(str::to_string),
        camera: Some("Sony A7M4".to_string()),
        source: "imported".to_string(),
        created_at: "2026-09-01T00:00:00.000Z".to_string(),
        origin: "imported".to_string(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();
    db.asset_id_by_path(&path_str).unwrap().unwrap()
}

fn set_mtime(path: &Path, at: chrono::DateTime<chrono::Utc>) {
    let t: std::time::SystemTime = at.into();
    let times = std::fs::FileTimes::new().set_modified(t).set_accessed(t);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

/// 导入带子组：落位仍是纯时间目录（无子组文件夹段）；album_item 引用带
/// subgroup 值。
#[test]
fn import_with_subgroup_has_zero_physical_footprint() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let album = db.album_create("子组册").unwrap();

    common::build_many(src.path(), 2);
    set_mtime(&src.path().join("DCIM/IMG_0000.jpg"), common::utc(2026, 4, 1, 8, 0, 0));
    set_mtime(&src.path().join("DCIM/IMG_0001.jpg"), common::utc(2026, 4, 2, 9, 0, 0));

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.album_id = Some(album.id);
        p.album_subgroup = Some("成片/A".into());
    });
    assert_eq!(stats.done_files, 2);
    assert_eq!(
        db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "done"
    );
    // 唯一物理层 = 纯时间年月目录；子组名不落盘
    assert!(target.path().join("2026/04/IMG_0000.jpg").is_file());
    assert!(target.path().join("2026/04/IMG_0001.jpg").is_file());
    let subdirs: Vec<std::path::PathBuf> = std::fs::read_dir(target.path().join("2026/04"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    assert!(subdirs.is_empty(), "子组不得有物理文件夹: {subdirs:?}");

    // 引用带原始子组字符串（净化段已退役——展示名即真值）
    let subgroups = db.album_subgroups(album.id).unwrap();
    assert_eq!(subgroups, vec![("成片/A".to_string(), 2)]);
}

/// 移组三向（根↔组A↔组B）纯引用改写：文件与 assets.path 纹丝不动。
#[test]
fn move_subgroup_is_pure_reference_rewrite() {
    let (_dir, state, db) = setup();
    let album = db.album_create("三向册").unwrap();
    let photo_dir = state.config_dir.join("photos-logical");
    std::fs::create_dir_all(&photo_dir).unwrap();
    let photo = photo_dir.join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo, Some("2026-06-01T10:00:00.000Z"));

    // 根 → 组A
    assert_eq!(
        fetch_album_claim_assets(&state, album.id, &[id], Some("组A")).unwrap().moved,
        1
    );
    let sub_a: Option<String> = subgroup_of(&db, album.id, id);
    assert_eq!(sub_a.as_deref(), Some("组A"));

    // 组A → 组B：零文件操作、路径不改写
    let moved = fetch_album_item_move_subgroup(&state, album.id, &[id], Some("组B")).unwrap();
    assert_eq!(moved, 1);
    assert!(photo.is_file());
    let path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert_eq!(path, photo.to_string_lossy());
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("组B"));

    // 组B → 根（None）
    let back = fetch_album_item_move_subgroup(&state, album.id, &[id], None).unwrap();
    assert_eq!(back, 1);
    assert_eq!(subgroup_of(&db, album.id, id), None);
    assert!(photo.is_file(), "全程零文件操作");

    // 不在册的 id 自然不命中（0 行语义）
    let other_album = db.album_create("别册").unwrap();
    assert_eq!(
        fetch_album_item_move_subgroup(&state, other_album.id, &[id], Some("组X")).unwrap(),
        0
    );
}

fn subgroup_of(db: &db::Db, album_id: i64, asset_id: i64) -> Option<String> {
    db.0.query_row(
        "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
        rusqlite::params![album_id, asset_id],
        |r| r.get(0),
    )
    .unwrap_or(None)
}

/// claim 带子组：引用直接落在目标子组（无物理语义）。
#[test]
fn claim_with_subgroup_writes_reference_into_subgroup() {
    let (_dir, state, db) = setup();
    let album = db.album_create("归册子组").unwrap();
    let photo_dir = state.config_dir.join("photos-claim");
    std::fs::create_dir_all(&photo_dir).unwrap();
    let photo = photo_dir.join("DSC_0002.jpg");
    std::fs::write(&photo, b"jpeg2").unwrap();
    let id = ins(&db, &photo, None);

    let result = fetch_album_claim_assets(&state, album.id, &[id], Some("精选")).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    assert!(photo.is_file());
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("精选"));

    // 幂等重试：已引用（显式给子组时改写归属，与 album_add_assets 同语义）
    let again = fetch_album_claim_assets(&state, album.id, &[id], Some("重选")).unwrap();
    assert_eq!((again.moved, again.skipped), (0, 1));
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("重选"));
}
