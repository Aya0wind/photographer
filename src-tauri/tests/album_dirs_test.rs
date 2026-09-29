//! 相册物理目录化 + 归册挪移 + LR 暂存夹（0018，阶段 B3；布局改版
//! 2026-09-28 定案：`photoRoot/{创建YYYY}/{创建MM}/{dir_name}/` 相册内平铺）：
//! 布局公式 album_home_rel 表驱动、目录名净化与唯一化、受控改目录（物理
//! rename + 路径改写 + 边车随行，父目录=创建年月）、导入落相册主目录平铺
//! （跨天照片同目录；未分组兜底同款公式）、归册挪移（同卷 rename、跨卷
//! copy+校验+删源、幂等、他相册主目录拒绝、外部库拒绝；落位与导入同公式）、
//! LR 暂存夹（硬链接优先 / 跨卷复制回退 + 计数）。

mod common;

use common::library_fixture as setup;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::{open_db, run_engine};
use db::AssetRow;
use events::AssetKind;
use ipc::album::fetch_album_dir_rename;
use ipc::claim::{fetch_album_claim_assets, fetch_lr_staging_create};

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
    })
    .unwrap();
    db.asset_id_by_path(path).unwrap().unwrap()
}


// ---------------------------------------------------------------------------
// 目录名净化与唯一化
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
// 布局公式（唯一来源 album_home_rel；表驱动钉死）
// ---------------------------------------------------------------------------

#[test]
fn album_home_rel_formula_is_table_driven() {
    use db::album_home_rel_parts as parts;
    // 外层两段 = 相册创建时间（UTC 口径）年/月，相册内平铺
    assert_eq!(parts("2026-09-27T05:09:56.381Z", "album-1"), "2026/09/album-1");
    assert_eq!(parts("2025-12-31T23:59:59.999Z", "婚礼"), "2025/12/婚礼");
    assert_eq!(parts("2026-01-01T00:00:00.000Z", "青海 湖-自驾"), "2026/01/青海 湖-自驾");
    assert_eq!(parts("1999-06-30T12:00:00.000Z", "a/b"), "1999/06/a/b");
    // created_at 解析失败（理论不可能）兜底 dir_name 直挂根
    assert_eq!(parts("not-a-date", "x"), "x");
    assert_eq!(parts("", "y"), "y");

    // db 形态与批量形态同源
    let tmp = tempfile::tempdir().unwrap();
    let db = open_db(tmp.path());
    let album = db.album_create("公式册").unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-05T08:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();
    assert_eq!(
        db.album_home_rel(album.id).unwrap().as_deref(),
        Some("2026/03/公式册")
    );
    assert!(db
        .album_home_rels()
        .unwrap()
        .contains(&(album.id, "2026/03/公式册".to_string())));
}

// ---------------------------------------------------------------------------
// 受控改目录
// ---------------------------------------------------------------------------

#[test]
fn album_dir_rename_moves_dir_rewrites_paths_and_sidecar_follows() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let album = db.album_create("婚礼跟拍").unwrap();
    // 固定创建时间 → 主目录 photoRoot/2026/03/婚礼跟拍（布局公式外层段）
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-05T08:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();
    // 相册主目录内已有一张平铺照片 + 边车（新布局产物形态）
    let old_dir = photo_root.join("2026").join("03").join("婚礼跟拍");
    std::fs::create_dir_all(&old_dir).unwrap();
    let photo = old_dir.join("DSC_9001.NEF");
    std::fs::write(&photo, b"nef").unwrap();
    std::fs::write(old_dir.join("DSC_9001.xmp"), b"<xmp/>").unwrap();
    let id = ins(
        &db,
        &photo.to_string_lossy(),
        AssetKind::Raw,
        Some("2026-06-01T10:00:00.000Z"), // 拍摄日与外层无关（平铺）
    );

    // 受控改目录：只动最后一段 dir_name，父目录（创建年月）不动
    let new_dir = fetch_album_dir_rename(&state, album.id, "婚礼跟拍-精修").unwrap();
    assert_eq!(new_dir, "婚礼跟拍-精修");
    assert!(!photo.exists(), "旧路径应不存在");
    let new_photo = photo_root
        .join("2026")
        .join("03")
        .join("婚礼跟拍-精修")
        .join("DSC_9001.NEF");
    assert!(new_photo.is_file(), "文件随目录整体移动（仍在创建年月父目录下）");
    assert!(new_photo.with_extension("xmp").is_file(), "边车随行");

    // DB 路径已改写；dir_name 更新
    let path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert_eq!(path, new_photo.to_string_lossy());
    assert_eq!(
        db.album_dir_name(album.id).unwrap().as_deref(),
        Some("婚礼跟拍-精修")
    );

    // 与其他相册目录重名拒绝
    let other = db.album_create("另一个相册").unwrap();
    assert!(fetch_album_dir_rename(&state, other.id, "婚礼跟拍-精修").is_err());
    // 目标目录已存在拒绝（盘上同名目录须在同父目录——创建年月之下）
    let third = db.album_create("占位相册").unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-05T08:00:00.000Z' WHERE id = ?1",
        [third.id],
    )
    .unwrap();
    let third_home_parent = photo_root.join("2026").join("03");
    std::fs::create_dir_all(third_home_parent.join("占位")).unwrap();
    assert!(fetch_album_dir_rename(&state, third.id, "占位").is_err());
    // 相册不存在报错
    assert!(fetch_album_dir_rename(&state, 999, "无中生有").is_err());
}

// ---------------------------------------------------------------------------
// 导入落位（布局固定：photoRoot/{创建YYYY}/{创建MM}/{dir_name}/ 平铺）
// ---------------------------------------------------------------------------

/// 把文件 mtime 设到指定 UTC 时刻（跨天照片平铺验证用）。
fn set_mtime(path: &std::path::Path, at: chrono::DateTime<chrono::Utc>) {
    let t: std::time::SystemTime = at.into();
    let times = std::fs::FileTimes::new().set_modified(t).set_accessed(t);
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(times)
        .unwrap();
}

#[test]
fn import_lands_flat_in_album_home_by_created_date() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let album = db.album_create("青海 湖/自驾").unwrap();
    // 固定创建时间 2026-09-12 → 主目录 2026/09/青海 湖-自驾
    db.0.execute(
        "UPDATE album SET created_at = '2026-09-12T02:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();
    common::build_source(src.path());
    // 跨天/跨月拍摄时间（mtime 回退源）：全部应落**同一个**相册主目录平铺
    set_mtime(&src.path().join("DCIM\\100CANON\\IMG_0001.jpg"), common::utc(2026, 1, 2, 3, 0, 0));
    set_mtime(&src.path().join("DCIM\\100CANON\\IMG_0002.CR3"), common::utc(2026, 5, 6, 7, 0, 0));
    set_mtime(&src.path().join("DCIM\\100CANON\\IMG_0003.jpg"), common::utc(2025, 12, 31, 9, 0, 0));

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.album_id = Some(album.id);
    });
    assert_eq!(job_status(&db, job_id), "done");
    assert_eq!(stats.done_files, 3);
    let home = target.path().join("2026").join("09").join("青海 湖-自驾");
    for name in ["IMG_0001.jpg", "IMG_0002.CR3", "IMG_0003.jpg"] {
        let p = home.join(name);
        assert!(p.is_file(), "跨天照片应同目录平铺: {}", p.display());
    }
    // 相册目录内**无**内层日期目录（布局平铺铁证）
    let entries: Vec<_> = std::fs::read_dir(&home).unwrap().collect::<Result<_, _>>().unwrap();
    assert!(
        entries.iter().all(|e| !e.path().is_dir()),
        "相册内不得再有子目录: {:?}",
        entries.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
    let count: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1",
            [album.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "三件全部入册");

    // 不带 album_id（run_engine 兜底「未分组」）：同款公式（其 created_at =
    // 首次自动创建时刻）——先 ensure 并固定创建时间，所有照片平铺进这一个目录
    let ungrouped_id = db.ensure_default_album().unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-08T01:00:00.000Z' WHERE id = ?1",
        [ungrouped_id],
    )
    .unwrap();
    let src2 = tempfile::tempdir().unwrap();
    common::build_many(src2.path(), 3);
    set_mtime(&src2.path().join("DCIM\\IMG_0000.jpg"), common::utc(2026, 2, 1, 0, 0, 0));
    set_mtime(&src2.path().join("DCIM\\IMG_0001.jpg"), common::utc(2026, 8, 9, 0, 0, 0));
    let target2 = tempfile::tempdir().unwrap();
    let (job2, stats2) = run_engine(src2.path(), db_dir.path(), target2.path(), |_| {});
    assert_eq!(job_status(&db, job2), "done");
    assert_eq!(stats2.done_files, 3);
    let fallback = target2.path().join("2026").join("03").join("未分组");
    for name in ["IMG_0000.jpg", "IMG_0001.jpg", "IMG_0002.jpg"] {
        assert!(
            fallback.join(name).is_file(),
            "未分组兜底相册平铺: {}",
            fallback.join(name).display()
        );
    }
}

fn job_status(db: &db::Db, job_id: i64) -> String {
    db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
        r.get(0)
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// 归册挪移
// ---------------------------------------------------------------------------

#[test]
fn claim_moves_date_root_assets_into_album_dir_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    let album = db.album_create("青海湖").unwrap();
    // 固定创建时间 2026-03-05 → 主目录 2026/03/青海湖
    db.0.execute(
        "UPDATE album SET created_at = '2026-03-05T08:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();

    // 日期根照片 + 边车（历史遗留位置；拍摄日 2026-06-01 ≠ 相册创建月）
    let captured = "2026-06-01T10:00:00.000Z";
    let src_dir = photo_root.join("2026").join("06-01");
    std::fs::create_dir_all(&src_dir).unwrap();
    let photo = src_dir.join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    std::fs::write(src_dir.join("DSC_0001.xmp"), b"<xmp/>").unwrap();
    let id = ins(
        &db,
        &photo.to_string_lossy(),
        AssetKind::Photo,
        Some(captured),
    );

    let result = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    assert_eq!(result.skipped, 0);
    assert!(result.failed.is_empty());

    // 落位 = photoRoot/{创建YYYY}/{创建MM}/{dir_name}/ 平铺（与拍摄日无关）
    let dst = photo_root.join("2026").join("03").join("青海湖").join("DSC_0001.jpg");
    assert!(dst.is_file(), "应挪到 {}", dst.display());
    assert!(dst.with_extension("xmp").is_file(), "边车随行");
    assert!(!photo.exists());
    let path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert_eq!(path, dst.to_string_lossy());
    let in_album: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![album.id, id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_album, 1, "挪移补挂引用");

    // 幂等重试：已在相册目录 → skipped
    let again = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(again.moved, 0);
    assert_eq!(again.skipped, 1);

    // 他相册主目录资产 → 归入 = 物理挪移并改主相册（引用转移）；目标相册
    // created_at 决定外层段（另一册 2026-07 → 2026/07/另一册）
    let other = db.album_create("另一册").unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-07-22T09:30:00.000Z' WHERE id = ?1",
        [other.id],
    )
    .unwrap();
    let result2 = fetch_album_claim_assets(&state, other.id, &[id], None).unwrap();
    assert_eq!(result2.moved, 1, "{result2:?}");
    let moved_path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    let expected2 = photo_root.join("2026").join("07").join("另一册").join("DSC_0001.jpg");
    assert_eq!(
        moved_path,
        expected2.to_string_lossy(),
        "物理位置改到新主相册（外层段=新相册创建年月）"
    );
    let refs: Vec<i64> =
        db.0.prepare("SELECT album_id FROM album_item WHERE asset_id = ?1")
            .unwrap()
            .query_map([id], |r| r.get(0))
            .unwrap()
            .collect::<Result<Vec<i64>, _>>()
            .unwrap();
    assert!(!refs.contains(&album.id), "原主相册引用应移除");
    assert!(refs.contains(&other.id), "新主相册引用建立");

    // 外部库资产 → 拒绝挪移
    let ext_dir = dir.path().join("elsewhere");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let ext_file = ext_dir.join("EXT_0001.jpg");
    std::fs::write(&ext_file, b"ext").unwrap();
    let ext = ins(
        &db,
        &ext_file.to_string_lossy(),
        AssetKind::Photo,
        Some(captured),
    );
    db.0.execute("UPDATE assets SET origin = 'external' WHERE id = ?1", [ext])
        .unwrap();
    let result3 = fetch_album_claim_assets(&state, album.id, &[ext], None).unwrap();
    assert_eq!(result3.failed.len(), 1);
    assert!(result3.failed[0].error.contains("外部库"));

    // 回收站资产 → 拒绝
    db.assets_trash_move(&[ext]).unwrap();
    let result4 = fetch_album_claim_assets(&state, album.id, &[ext], None).unwrap();
    assert_eq!(result4.failed.len(), 1);
    assert!(result4.failed[0].error.contains("回收站"));
}

#[test]
fn claim_cross_volume_copies_verifies_and_deletes_source() {
    let dir = tempfile::tempdir().unwrap();
    // 用 canonicalize 产生 verbatim 前缀（\\?\C:\...）——volume_root_of 判为
    // 另一「卷」，确定性地走 copy+校验+删源分支（tempdir 模拟两卷）
    let photos_real = dir.path().join("photos");
    std::fs::create_dir_all(&photos_real).unwrap();
    let photos_verbatim = std::fs::canonicalize(&photos_real).unwrap();
    let db_dir_real = dir.path().join("db");
    std::fs::create_dir_all(&db_dir_real).unwrap();
    let state = common::state_with_library(
        std::fs::canonicalize(&db_dir_real).unwrap().as_path(),
        &photos_verbatim,
        Duration::from_millis(1),
    );
    let db = open_db(&db_dir_real);
    let album = db.album_create("跨卷相册").unwrap();

    // 源在普通路径（非 verbatim）
    let src_dir = dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    let photo = src_dir.join("DSC_0002.jpg");
    std::fs::write(&photo, b"jpeg-bytes-0002").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), AssetKind::Photo, None);
    // 库内指纹与真实文件对齐（校验按库内 xxhash 对账）
    db.0.execute(
        "UPDATE assets SET size = ?2, xxhash = ?3 WHERE id = ?1",
        rusqlite::params![
            id,
            15i64,
            xxhash_rust::xxh64::xxh64(b"jpeg-bytes-0002", 0) as i64
        ],
    )
    .unwrap();

    let result = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    assert!(!photo.exists(), "跨卷校验通过后删源");
    let moved_path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert!(
        moved_path.starts_with(r"\\?\"),
        "落点在 verbatim 相册目录: {moved_path}"
    );
    assert!(std::path::Path::new(&moved_path).is_file());

    // 指纹不符 → 校验失败、源保留
    let bad = src_dir.join("DSC_0003.jpg");
    std::fs::write(&bad, b"different-bytes").unwrap();
    let bad_id = ins(&db, &bad.to_string_lossy(), AssetKind::Photo, None);
    db.0.execute(
        "UPDATE assets SET size = 999, xxhash = 42 WHERE id = ?1",
        [bad_id],
    )
    .unwrap();
    let result2 = fetch_album_claim_assets(&state, album.id, &[bad_id], None).unwrap();
    assert_eq!(result2.moved, 0);
    assert_eq!(result2.failed.len(), 1);
    assert!(result2.failed[0].error.contains("校验失败"));
    assert!(bad.is_file(), "校验失败源保留");
}

/// 三调用点同公式（导入 ↔ claim）：引擎导入产物与归册挪移产物落在**同一个**
/// 相册主目录（album_home_rel 唯一公式；album 导出的一致性见
/// edit_export_test 的落位断言——同样经 album_home_rel）。
#[test]
fn import_and_claim_share_the_same_album_home_formula() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    let album = db.album_create("同公式册").unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-04-10T06:00:00.000Z' WHERE id = ?1",
        [album.id],
    )
    .unwrap();

    // ① 引擎导入：落 photoRoot/2026/04/同公式册/IMG_0001.jpg
    let src = tempfile::tempdir().unwrap();
    common::build_many(src.path(), 1);
    // 导入 target_root 直接给 photo_root（布局根）
    let (_, stats) = run_engine(src.path(), &db_dir, &photo_root, |p| {
        p.album_id = Some(album.id);
    });
    assert_eq!(stats.done_files, 1, "{stats:?}");
    let home = photo_root.join("2026").join("04").join("同公式册");
    assert!(home.join("IMG_0000.jpg").is_file(), "导入落相册主目录");

    // ② claim：日期根资产归入同相册 → 同一目录平铺
    let root_dir = photo_root.join("2019").join("11-23");
    std::fs::create_dir_all(&root_dir).unwrap();
    let photo = root_dir.join("OLD_0001.jpg");
    std::fs::write(&photo, b"old-jpeg").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), AssetKind::Photo, Some("2019-11-23T09:00:00.000Z"));
    let result = fetch_album_claim_assets(&state, album.id, &[id], None).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    let moved: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    assert_eq!(
        moved,
        home.join("OLD_0001.jpg").to_string_lossy(),
        "claim 与导入同目录（相册创建年月外层 + 平铺）"
    );
    // 旧日期根清空
    assert!(!photo.exists());
}

// ---------------------------------------------------------------------------
// LR 暂存夹
// ---------------------------------------------------------------------------

#[test]
fn lr_staging_create_hardlinks_same_volume_and_copies_cross_volume() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let p1 = photo_root.join("DSC_0001.jpg");
    std::fs::write(&p1, b"one").unwrap();
    let p2 = photo_root.join("DSC_0002.jpg");
    std::fs::write(&p2, b"two").unwrap();
    let id1 = ins(&db, &p1.to_string_lossy(), AssetKind::Photo, None);
    let id2 = ins(&db, &p2.to_string_lossy(), AssetKind::Photo, None);

    // 唯一暂存夹名（暂存夹落在卷根，避免跨测试运行污染 + 便于清理）
    let uniq = format!(
        "lr-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    // 命名暂存夹：同卷 → 硬链接
    let r = fetch_lr_staging_create(&state, &[id1, id2], Some(&uniq)).unwrap();
    assert!(r.dir.contains(".lr-staging"));
    assert!(r.created);
    assert_eq!(r.hardlinked, 2, "{r:?}");
    assert_eq!(r.copied, 0);
    let staged1 = std::path::Path::new(&r.dir).join("DSC_0001.jpg");
    assert!(staged1.is_file());
    // 内容一致性（硬链接与源同字节）
    assert_eq!(std::fs::read(&staged1).unwrap(), b"one");

    // 重名复用（不新建目录）
    let r2 = fetch_lr_staging_create(&state, &[id1], Some(&uniq)).unwrap();
    assert!(!r2.created, "已存在的暂存夹应复用");

    // 无名 → 时间戳目录
    let r3 = fetch_lr_staging_create(&state, &[id1], None).unwrap();
    assert!(r3.dir.contains(".lr-staging"));
    assert!(r3.created);
    assert_eq!(r3.hardlinked, 1);

    // 跨卷（verbatim 源路径）→ 回退复制
    let photos_verbatim = std::fs::canonicalize(&photo_root).unwrap();
    let verbatim_file = photos_verbatim.join("DSC_0001.jpg");
    let id_v = ins(
        &db,
        &verbatim_file.to_string_lossy(),
        AssetKind::Photo,
        None,
    );
    let r4 = fetch_lr_staging_create(&state, &[id_v], Some("跨卷")).unwrap();
    assert_eq!(r4.copied, 1, "{r4:?}");
    assert_eq!(r4.hardlinked, 0);

    // 失效 id 跳过不计数
    let r5 = fetch_lr_staging_create(&state, &[99999], Some(&uniq)).unwrap();
    assert_eq!(r5.hardlinked + r5.copied, 0);

    // 清理卷根暂存产物（暂存夹一次性可弃）
    let _ = std::fs::remove_dir_all(&r.dir);
}

// ---------------------------------------------------------------------------
// 默认相册「未分组」（0018 修订：导入必落相册的系统级保底）
// ---------------------------------------------------------------------------

#[test]
fn default_album_ungrouped_is_immovable_and_recreated_by_name() {
    let (_dir, state, db) = setup();

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

#[test]
fn claim_from_default_album_moves_file_but_keeps_reference() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    let default_id = db.ensure_default_album().unwrap();
    // 「未分组」创建 2026-01 → 主目录 2026/01/未分组
    db.0.execute(
        "UPDATE album SET created_at = '2026-01-15T00:00:00.000Z' WHERE id = ?1",
        [default_id],
    )
    .unwrap();
    let target = db.album_create("正式相册").unwrap();
    db.0.execute(
        "UPDATE album SET created_at = '2026-05-05T00:00:00.000Z' WHERE id = ?1",
        [target.id],
    )
    .unwrap();

    // 照片已入「未分组」主目录（新布局平铺形态；未真正归类）
    let src_dir = photo_root.join("2026").join("01").join("未分组");
    std::fs::create_dir_all(&src_dir).unwrap();
    let photo = src_dir.join("DSC_0007.jpg");
    std::fs::write(&photo, b"jpeg7").unwrap();
    let id = ins(
        &db,
        &photo.to_string_lossy(),
        AssetKind::Photo,
        Some("2026-01-01T00:00:00.000Z"),
    );
    let _ = db.album_add_assets(default_id, &[id], None);

    // 归入正式相册：物理挪移 + 路径更新；「未分组」引用保留（兜底袋不清）
    let result = fetch_album_claim_assets(&state, target.id, &[id], None).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    let moved_path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    let expected = photo_root.join("2026").join("05").join("正式相册").join("DSC_0007.jpg");
    assert_eq!(moved_path, expected.to_string_lossy());
    assert!(!photo.exists());
    let in_default: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![default_id, id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        in_default, 1,
        "未分组引用保留（UI 判定未真正归类的依据之一）"
    );
    let in_target: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![target.id, id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_target, 1, "目标相册引用建立");
}

#[test]
fn import_start_requires_album_and_ensures_default_exists() {
    use common::ipc_plan;

    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    let db = open_db(db_dir.path());

    // IPC 校验：album_id = None 报「必须选择相册」；同时启动链已确保「未分组」存在
    let plan = ipc_plan(&state, target.path());
    let plan_none = import::engine::ImportPlan {
        album_id: None,
        ..plan.clone()
    };
    let err = ipc::start_import(&state, plan_none).unwrap_err();
    assert!(err.contains("必须选择相册"), "{err}");
    assert!(
        db.0.query_row(
            "SELECT COUNT(*) FROM album WHERE name = '未分组'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap()
            > 0,
        "导入启动链应自动创建「未分组」"
    );

    // 引擎防御：直构引擎（绕过 IPC）None 同样被拒
    common::build_source(src.path());
    let mut plan2 = plan;
    plan2.album_id = None;
    let mut engine = import::engine::Engine::new(
        open_db(db_dir.path()),
        events::EventBus::new(),
        Box::new(devices::volume::VolumeSource::new(src.path())),
        plan2,
    );
    let err = engine.begin().unwrap_err();
    assert!(err.to_string().contains("必须选择相册"));
}
