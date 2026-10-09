//! 相册与落盘布局（2026-10-09 单数据库多照片库 §一/§三新语义）：
//! 相册/子组 = 纯逻辑概念——claim/挪组只改 album_item 引用，零文件操作；
//! 导入落盘 = 纯时间 `{库root}/{拍摄年}/{拍摄月}/{原文件名}`（EXIF 时间
//! 缺失回退 mtime），相册归属不影响物理位置；重名走 rename 策略
//! （`_1` 后缀）且 XMP 边车同名跟随；默认相册「未分组」幂等创建禁删禁改名。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan, settings,
    tasks, tethering, thumbs,
};

use std::fs;
use std::time::Duration;

use common::{open_db, run_engine};
use db::AssetRow;
use events::AssetKind;
use ipc::album::fetch_album_item_move_subgroup;
use ipc::claim::fetch_album_claim_assets;

fn ins(db: &db::Db, path: &str, kind: AssetKind, captured: Option<&str>) -> i64 {
    db.insert_asset(&AssetRow {
        path: path.to_string(),
        filename: path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
        size: 100,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: path.len() as u64,
        kind,
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
    db.asset_id_by_path(path).unwrap().unwrap()
}

// ---------------------------------------------------------------------------
// 目录名净化与唯一化（album_create 仍生成 dir_name 残留列）
// ---------------------------------------------------------------------------

#[test]
fn album_create_generates_sanitized_unique_dir_names() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());

    // 非法字符折叠 + 尾点剥离
    let a = db.album_create("春节 2026<拍摄>: v1.").unwrap();
    assert_eq!(a.dir_name, "春节 2026-拍摄-- v1");

    // 同名相册 → 显示名重名报错；换名但净化后同目录名 → 后缀去重
    let b = db.album_create("春-节-2026").unwrap();
    assert_ne!(b.dir_name, a.dir_name, "净化结果不应与既有目录撞名");
    // 保留设备名加前缀
    let c = db.album_create("CON").unwrap();
    assert_eq!(c.dir_name, "album-CON");
    // 显示名改名不动 dir_name
    db.album_rename(a.id, "新的名字").unwrap();
    let listed = db.album_list().unwrap();
    let still = listed.iter().find(|x| x.id == a.id).unwrap();
    assert_eq!(still.dir_name, a.dir_name);
    // 空净化结果回退
    let d = db.album_create("***").unwrap();
    assert!(!d.dir_name.is_empty());
}

// ---------------------------------------------------------------------------
// 纯时间布局（§三 唯一公式：EXIF 时间缺失回退文件 mtime）
// ---------------------------------------------------------------------------

/// 纯时间相对段公式表驱动：`{拍摄年}/{拍摄月}/{原文件名}`。
#[test]
fn time_layout_formula_is_table_driven() {
    use import::engine::pipeline::time_rel;
    assert_eq!(
        time_rel(common::utc(2026, 6, 28, 15, 30, 0), "IMG_0001.CR3"),
        "2026/06/IMG_0001.CR3"
    );
    assert_eq!(
        time_rel(common::utc(2025, 12, 31, 23, 59, 59), "DSC.jpg"),
        "2025/12/DSC.jpg"
    );
    assert_eq!(
        time_rel(common::utc(1999, 1, 1, 0, 0, 0), "a.b.c.png"),
        "1999/01/a.b.c.png"
    );
}

/// 把文件 mtime 设到指定 UTC 时刻（EXIF 缺失回退源验证用）。
fn set_mtime(path: &std::path::Path, at: chrono::DateTime<chrono::Utc>) {
    assert!(path.is_file(), "测试文件不存在: {}", path.display());
    let t: std::time::SystemTime = at.into();
    let times = std::fs::FileTimes::new().set_modified(t).set_accessed(t);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

/// 导入落位按拍摄时间分年月目录；相册/子组只是引用，不影响物理位置
///（跨天照片各归各的年月目录——与旧「相册内平铺」模型的本质差异）。
#[test]
fn import_lands_pure_time_layout_ignoring_album() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let album = db.album_create("青海 湖/自驾").unwrap();

    common::build_source(src.path());
    // 无 EXIF → 回退 mtime：三张分别落在 2026/01、2026/05、2025/12
    let dcim = src.path().join("DCIM").join("100CANON");
    set_mtime(&dcim.join("IMG_0001.jpg"), common::utc(2026, 1, 2, 3, 0, 0));
    set_mtime(&dcim.join("IMG_0002.CR3"), common::utc(2026, 5, 6, 7, 0, 0));
    set_mtime(&dcim.join("IMG_0003.jpg"), common::utc(2025, 12, 31, 9, 0, 0));

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.album_id = Some(album.id);
        p.album_subgroup = Some("成片".into());
    });
    assert_eq!(job_status(&db, job_id), "done");
    assert_eq!(stats.done_files, 3);
    assert!(target.path().join("2026/01/IMG_0001.jpg").is_file());
    assert!(target.path().join("2026/05/IMG_0002.CR3").is_file());
    assert!(target.path().join("2025/12/IMG_0003.jpg").is_file());

    // 相册引用建立（含子组），物理位置与相册无关
    let in_album: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND subgroup = '成片'",
            [album.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_album, 3, "三件全部入册（子组引用）");
    // 库 root 下只有年月目录（无相册目录层）
    let mut roots: Vec<String> = std::fs::read_dir(target.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    roots.sort();
    assert_eq!(roots, vec!["2025".to_string(), "2026".to_string()]);
}

/// 重名 rename 策略：目标已存在（内容不同）→ `_1` 后缀；XMP 边车与本体
/// 同名跟随（本体改名后边车 stem 同步）。
#[test]
fn rename_policy_suffix_and_sidecar_follows() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    common::build_source(src.path());
    set_mtime(
        &src.path().join("DCIM/100CANON/IMG_0001.jpg"),
        common::utc(2026, 3, 8, 1, 0, 0),
    );
    fs::write(
        src.path().join("DCIM/100CANON/IMG_0001.xmp"),
        b"<xmp-rating-4/>",
    )
    .unwrap();

    // 预占目标路径（内容不同；skip_imported=false 排除内容查重干扰）
    fs::create_dir_all(target.path().join("2026/03")).unwrap();
    fs::write(target.path().join("2026/03/IMG_0001.jpg"), b"occupied").unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.duplicate_policy = settings::DuplicatePolicy::Rename;
        p.skip_imported = false;
    });
    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.failed_files, 0);
    assert_eq!(
        fs::read(target.path().join("2026/03/IMG_0001.jpg")).unwrap(),
        b"occupied",
        "占用文件不得覆盖"
    );
    assert!(
        target.path().join("2026/03/IMG_0001_1.jpg").is_file(),
        "重名走 _1 后缀"
    );
    assert!(
        target.path().join("2026/03/IMG_0001_1.xmp").is_file(),
        "边车与本体同名跟随（stem 随 rename 同步）"
    );
}

// ---------------------------------------------------------------------------
// 相册操作逻辑化（§一：变更归属零文件操作）
// ---------------------------------------------------------------------------

/// 归册 = 纯引用建立：文件纹丝不动、路径不改写；幂等重试 skipped；
/// 挪子组 = 纯引用改写（零文件操作）；回收站资产拒绝。
#[test]
fn claim_builds_reference_without_touching_files() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(photo_root.join("2026/06")).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    let album = db.album_create("青海湖").unwrap();

    let photo = photo_root.join("2026").join("06").join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(
        &db,
        &photo.to_string_lossy(),
        AssetKind::Photo,
        Some("2026-06-01T10:00:00.000Z"),
    );

    let result = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    assert!(result.failed.is_empty());
    assert!(photo.is_file(), "归册不得动物理文件");
    let path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert_eq!(path, photo.to_string_lossy(), "路径不改写");

    // 幂等重试：已引用 → skipped
    let again = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(again.moved, 0);
    assert_eq!(again.skipped, 1);

    // 挪子组 = 纯引用改写（零文件操作）
    let moved = fetch_album_item_move_subgroup(&state, album.id, &[id], Some("成片")).unwrap();
    assert_eq!(moved, 1);
    assert!(photo.is_file(), "挪组不得动物理文件");
    let subgroup: Option<String> = db
        .0
        .query_row(
            "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![album.id, id],
            |r| r.get(0),
        )
        .unwrap_or(None);
    assert_eq!(subgroup.as_deref(), Some("成片"));

    // 回收站资产拒绝归册
    db.assets_trash_move(&[id]).unwrap();
    let other = db.album_create("另一册").unwrap();
    let rejected = fetch_album_claim_assets(&state, other.id, &[id], None).unwrap();
    assert_eq!(rejected.failed.len(), 1);
    assert!(rejected.failed[0].error.contains("回收站"));
}

// ---------------------------------------------------------------------------
// 默认相册「未分组」（系统级保底）
// ---------------------------------------------------------------------------

#[test]
fn default_album_ungrouped_is_immovable_and_recreated_by_name() {
    let (_dir, state, db) = common::library_fixture();

    // 自动创建幂等：两次 ensure 返回同一个 id
    let first = db.ensure_default_album().unwrap();
    let second = db.ensure_default_album().unwrap();
    assert_eq!(first, second);
    assert_eq!(first, 1);

    // 禁删
    assert!(db.album_delete(first).is_err());
    // 禁改名
    assert!(ipc::album::fetch_album_rename(&state, first, "改名了").is_err());
    // 再 ensure 仍是同一条（不会重复创建）
    assert_eq!(db.ensure_default_album().unwrap(), first);

    // 普通相册不受影响：可改名、可删
    let normal = db.album_create("普通册").unwrap();
    ipc::album::fetch_album_rename(&state, normal.id, "改名册").unwrap();
    db.album_delete(normal.id).unwrap();
}

/// 导入相册可选：不挂相册正常导入（画廊全局视图语义）；不存在相册早失败。
#[test]
fn import_album_is_optional_but_validated_when_present() {
    use common::ipc_plan;

    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    common::build_many(src.path(), 2);

    // 无相册导入：正常完成
    let plan = ipc_plan(&state, target.path());
    let job_id = ipc::start_import(&state, plan).unwrap();
    assert!(common::wait_done(&state, Duration::from_secs(15)));
    let db = open_db(db_dir.path());
    assert_eq!(job_status(&db, job_id), "done");
    assert_eq!(common::count_assets(&db), 2);

    // 相册不存在：start_import 拒绝
    let mut bad = ipc_plan(&state, target.path());
    bad.album_id = Some(999_999);
    let err = ipc::start_import(&state, bad).unwrap_err();
    assert!(err.contains("不存在"), "{err}");

    // 计划缺照片库：拒绝
    let mut no_lib = ipc_plan(&state, target.path());
    no_lib.target_library_id = String::new();
    let err = ipc::start_import(&state, no_lib).unwrap_err();
    assert!(err.contains("目标照片库"), "{err}");
}

fn job_status(db: &db::Db, job_id: i64) -> String {
    db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
        r.get(0)
    })
    .unwrap()
}
