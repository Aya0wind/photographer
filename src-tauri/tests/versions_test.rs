//! 原片-成片版本关系（0017，阶段 B2）：孪生导入自动建组（raw+sooc）、
//! 存量 pair 回填幂等（DB 路径 + scripts 同语义）、LR 成片导回三依据匹配
//! （文件名模板 / EXIF 时间窗口 / pHash 汉明）与确认流（导入建关系+归组）、
//! 幂等重放跳过、版本查询、group_role 三视图筛选、purge 空组清理。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks,
    thumbs, videos,
};

use std::time::Duration;

use common::open_db;
use db::{AssetFilters, AssetRow};
use events::AssetKind;
use ipc::versions::{
    asset_versions_core, lr_export_import_core, lr_export_scan_core, LrExportImportItem,
};

/// 直插一行资产（path 唯一，pair 可选），返回自增 id。
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

fn setup() -> (tempfile::TempDir, ipc::AppState, db::Db) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    (dir, state, db)
}

fn group_of(db: &db::Db, id: i64) -> Option<i64> {
    db.asset_group_of(id).unwrap()
}

// ---------------------------------------------------------------------------
// 组形成：孪生导入自动建组
// ---------------------------------------------------------------------------

#[test]
fn twin_import_forms_group_with_raw_sooc_roles() {
    let (_dir, _state, db) = setup();
    // RAW 先入册（无孪生：不入组）
    let raw = ins(&db, "X:/card/DSC_0001.NEF", AssetKind::Raw, None);
    assert_eq!(group_of(&db, raw), None, "孤儿单资产不强制入组");
    // 机内 JPEG 随后入册 → 双向 pair + 同组建组
    let jpg = ins(&db, "X:/card/DSC_0001.JPG", AssetKind::Photo, None);

    let group = group_of(&db, raw).expect("raw 应已入组");
    assert_eq!(group_of(&db, jpg), Some(group), "孪生同组");
    let members = db.group_members(group).unwrap();
    assert_eq!(
        members,
        vec![(raw, "raw".to_string()), (jpg, "sooc".to_string())]
    );

    // 重复入库（同路径 REPLACE）幂等：不产生新组
    ins(&db, "X:/card/DSC_0001.NEF", AssetKind::Raw, None);
    assert_eq!(db.asset_group_of(jpg).unwrap(), Some(group));
    assert_eq!(db.group_members(group).unwrap().len(), 2);

    // 版本查询：未派生时两成员
    let versions = asset_versions_core(&db, jpg).unwrap();
    assert_eq!(versions.group_id, Some(group));
    assert_eq!(versions.members.len(), 2);
    assert_eq!(versions.members[0].role.as_deref(), Some("raw"));

    // 视频孪生不入组（kind 不构成 raw/sooc）
    let vid = ins(&db, "X:/card/MVI_0009.MP4", AssetKind::Video, None);
    ins(&db, "X:/card/MVI_0009.JPG", AssetKind::Photo, None);
    assert_eq!(group_of(&db, vid), None, "video 孪生不建组");
}

#[test]
fn legacy_pair_backfill_is_idempotent() {
    let (_dir, _state, db) = setup();
    // 模拟 0017 之前的存量数据：绕过 pair 逻辑直插（SQL），再手写 pair 双向链
    for (path, kind) in [
        ("X:/old/DSC_0020.NEF", "raw"),
        ("X:/old/DSC_0020.JPG", "photo"),
        ("X:/old/DSC_0021.NEF", "raw"),
        ("X:/old/DSC_0021.JPG", "photo"),
    ] {
        db.0.execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, \
                 created_at, origin) VALUES (?1, ?2, 100, '2026-09-01T00:00:00.000Z', 1, ?3, \
                 'imported', '2026-09-01T00:00:00.000Z', 'imported')",
            rusqlite::params![path, path.rsplit('/').next().unwrap(), kind],
        )
        .unwrap();
    }
    for (a, b) in [
        ("DSC_0020.NEF", "DSC_0020.JPG"),
        ("DSC_0021.NEF", "DSC_0021.JPG"),
    ] {
        let ida: i64 =
            db.0.query_row("SELECT id FROM assets WHERE filename = ?1", [a], |r| {
                r.get(0)
            })
            .unwrap();
        let idb: i64 =
            db.0.query_row("SELECT id FROM assets WHERE filename = ?1", [b], |r| {
                r.get(0)
            })
            .unwrap();
        db.0.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
            rusqlite::params![ida, idb],
        )
        .unwrap();
        db.0.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
            rusqlite::params![idb, ida],
        )
        .unwrap();
    }

    // 首轮回填：2 组
    let (linked, created) = db.backfill_photo_groups_from_pairs().unwrap();
    assert_eq!((linked, created), (2, 2));
    // 幂等重跑：零新建
    assert_eq!(db.backfill_photo_groups_from_pairs().unwrap(), (0, 0));

    let group: i64 =
        db.0.query_row(
            "SELECT ga.group_id FROM group_asset ga JOIN assets a ON a.id = ga.asset_id \
             WHERE a.filename = 'DSC_0020.NEF'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let role: String =
        db.0.query_row(
            "SELECT role FROM group_asset WHERE group_id = ?1 \
             AND asset_id = (SELECT id FROM assets WHERE filename = 'DSC_0020.JPG')",
            [group],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(role, "sooc");
}

// ---------------------------------------------------------------------------
// LR 成片导回：扫描三依据 + 导入确认流
// ---------------------------------------------------------------------------

/// 一张可解码的纯灰 JPEG（导入测试只认魔数，不解码）。
fn write_gray_jpeg(path: &std::path::Path, gray: u8) {
    let img = image::RgbImage::from_pixel(64, 64, image::Rgb([gray, gray, gray]));
    img.save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

/// 确定性噪声纹理 JPEG（pHash 有结构：同 seed 距离 0，异 seed 距离 ~32）。
fn write_noise_jpeg(path: &std::path::Path, seed: u64) {
    let mut rng = seed;
    let mut img = image::RgbImage::new(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let v = (rng >> 33) as u8;
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    img.save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

/// 同 [`write_noise_jpeg`] 纹理但首像素 +5：字节不同（不触发 size+xxhash
/// 去重）、结构几乎一致（pHash 汉明 ≈ 0-4）。
fn write_noise_jpeg_perturbed(path: &std::path::Path, seed: u64) {
    let mut rng = seed;
    let mut img = image::RgbImage::new(64, 64);
    for y in 0..64u32 {
        for x in 0..64u32 {
            rng = rng
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut v = (rng >> 33) as u8;
            if x == 0 && y == 0 {
                v = v.wrapping_add(5);
            }
            img.put_pixel(x, y, image::Rgb([v, v, v]));
        }
    }
    img.save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

#[test]
fn lr_export_scan_matches_by_filename_time_and_phash() {
    let (dir, state, db) = setup();
    let photo_root = dir.path().join("photos");
    let export_dir = dir.path().join("lr_export");
    std::fs::create_dir_all(&export_dir).unwrap();

    // 库内原片：DSC_1234.NEF（raw）
    let raw = ins(
        &db,
        "X:/lib/DSC_1234.NEF",
        AssetKind::Raw,
        Some("2026-06-01T08:00:00.000Z"),
    );

    // ① 文件名模板：DSC_1234_edit_v1.jpg → 基名 DSC_1234
    let f1 = export_dir.join("DSC_1234_edit_v1.jpg");
    std::fs::write(&f1, b"\xFF\xD8\xFF\xE0jpeg").unwrap();

    // ② EXIF 时间窗口：通用 EXIF fixture（DateTimeOriginal 2026-06-28 15:30:00）
    let f2 = export_dir.join("IMG_9999.jpg");
    std::fs::write(&f2, common::build_exif_jpeg()).unwrap();
    let _ = ins(
        &db,
        "X:/lib/IMG_7777.NEF",
        AssetKind::Raw,
        Some("2026-06-28T15:30:00.000Z"),
    );

    // ③ pHash 近邻：库内噪声纹理资产（phash 已算），导出同纹理图（文件名
    //    /时间都不匹配 → 汉明兜底通道）
    let gray_path = photo_root.join("IMG_5555.NEF");
    write_noise_jpeg(&gray_path, 42);
    let gray_id = ins(&db, &gray_path.to_string_lossy(), AssetKind::Raw, None);
    let gray_phash = metadata::phash::phash_of_gray(
        &image::load_from_memory(&std::fs::read(&gray_path).unwrap())
            .unwrap()
            .to_luma8(),
    );
    db.set_phash(gray_id, gray_phash).unwrap();
    // 用真实 (size, xxhash) 覆写库行——ins 写的是假指纹，这里模拟真实入库
    let gray_bytes = std::fs::read(&gray_path).unwrap();
    db.0.execute(
        "UPDATE assets SET size = ?2, xxhash = ?3 WHERE id = ?1",
        rusqlite::params![
            gray_id,
            gray_bytes.len() as i64,
            xxhash_rust::xxh64::xxh64(&gray_bytes, 0) as i64
        ],
    )
    .unwrap();

    // 已在库的文件（size+xxhash 命中）不出现在候选
    std::fs::copy(&gray_path, export_dir.join("IMG_5555_copy.jpg")).unwrap();
    // 上面 copy 与原文件字节相同 → size+xxhash 命中跳过 ✓

    let candidates = lr_export_scan_core(&db, export_dir.to_str().unwrap()).unwrap();
    let by_name = |p: &str| {
        candidates
            .iter()
            .find(|c| c.path.ends_with(p))
            .unwrap_or_else(|| panic!("候选缺 {p}: {candidates:?}"))
    };

    // ① 文件名：基名匹配 raw（70 分），无 exif/phash 加成
    let m1 = by_name("DSC_1234_edit_v1.jpg");
    assert!(m1.candidates.iter().any(|c| c.asset_id == raw));
    let hit = m1.candidates.iter().find(|c| c.asset_id == raw).unwrap();
    assert!(
        hit.basis.contains("filename"),
        "basis 应含 filename: {}",
        hit.basis
    );
    assert!(hit.score >= 70 && hit.score <= 100);

    // ② EXIF 时间：±2s 窗口命中
    let m2 = by_name("IMG_9999.jpg");
    assert!(
        m2.candidates.iter().any(|c| c.basis.contains("exif_time")),
        "basis 应含 exif_time: {:?}",
        m2.candidates
    );

    // ③ pHash 兜底：文件名/时间无匹配 → 汉明近邻
    let f3 = export_dir.join("totally_unrelated.jpg");
    write_noise_jpeg_perturbed(&f3, 42);
    let candidates3 = lr_export_scan_core(&db, export_dir.to_str().unwrap()).unwrap();
    let m3 = candidates3
        .iter()
        .find(|c| c.path.ends_with("totally_unrelated.jpg"))
        .unwrap();
    assert!(
        m3.candidates
            .iter()
            .any(|c| c.asset_id == gray_id && c.basis.contains("phash")),
        "basis 应含 phash_score: {:?}",
        m3.candidates
    );

    // 已在库的 copy 不出现
    assert!(!candidates3
        .iter()
        .any(|c| c.path.ends_with("IMG_5555_copy.jpg")));

    // 完全无依据的候选 → 空 candidates（「待关联」）
    let f4 = export_dir.join("no_match_at_all.jpg");
    write_noise_jpeg(&f4, 7777); // 异纹理 → pHash 汉明 ~32 超限
    let candidates4 = lr_export_scan_core(&db, export_dir.to_str().unwrap()).unwrap();
    let m4 = candidates4
        .iter()
        .find(|c| c.path.ends_with("no_match_at_all.jpg"))
        .unwrap();
    assert!(
        m4.candidates.iter().all(|c| c.asset_id != gray_id),
        "不同灰度不该被 pHash 命中"
    );

    // 目录不存在报错
    assert!(lr_export_scan_core(&db, "X:/no/such/dir").is_err());
    let _ = &state;
}

#[test]
fn lr_export_import_links_relation_group_and_is_idempotent() {
    let (dir, state, db) = setup();
    let photo_root = dir.path().join("photos");
    let export_dir = dir.path().join("lr_export");
    std::fs::create_dir_all(&export_dir).unwrap();

    // 原片组：RAW + 机内 JPEG
    let raw = ins(&db, "X:/lib/DSC_0050.NEF", AssetKind::Raw, None);
    let jpg = ins(&db, "X:/lib/DSC_0050.JPG", AssetKind::Photo, None);
    let group = group_of(&db, raw).unwrap();

    // 相册（可选挂载）
    let album = db.album_create("交付相册").unwrap();

    // 成片
    let derived_src = export_dir.join("DSC_0050_edit_v1.jpg");
    write_gray_jpeg(&derived_src, 100);

    let matches = vec![LrExportImportItem {
        path: derived_src.to_string_lossy().into_owned(),
        source_asset_id: raw,
        basis: Some(r#"{"bases":["filename"]}"#.into()),
    }];
    let result = lr_export_import_core(&state, &db, &matches, Some(album.id), None).unwrap();
    assert_eq!(result.imported, 1, "{result:?}");
    assert!(result.failed.is_empty());

    // 成片入库位置：photoRoot/importSubdir/DSC_0050/
    let derived_path = photo_root
        .join("SmartPhoto")
        .join("DSC_0050")
        .join("DSC_0050_edit_v1.jpg");
    assert!(
        derived_path.is_file(),
        "成片应复制到 {}",
        derived_path.display()
    );
    let derived = db
        .asset_id_by_path(&derived_path.to_string_lossy())
        .unwrap()
        .unwrap();

    // 关系：derived_from → raw，confirmed=1，source=lr_export，basis 落库
    assert!(db.asset_relation_exists(derived, raw).unwrap());
    let (rel_source, basis, confirmed): (Option<String>, Option<String>, i64) =
        db.0.query_row(
            "SELECT source, match_basis, confirmed FROM asset_relation \
             WHERE asset_id = ?1 AND related_asset_id = ?2",
            rusqlite::params![derived, raw],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, i64>(2)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(confirmed, 1);
    assert_eq!(rel_source.as_deref(), Some("lr_export"));
    assert_eq!(basis.as_deref(), Some(r#"{"bases":["filename"]}"#));

    // 归组：追加到原片组（不新建）
    assert_eq!(group_of(&db, derived), Some(group));
    assert_eq!(group_of(&db, jpg), Some(group));
    let versions = asset_versions_core(&db, raw).unwrap();
    assert_eq!(versions.members.len(), 3);
    assert_eq!(versions.members[2].role.as_deref(), Some("derived"));
    assert_eq!(versions.members[2].asset_id, derived);

    // 挂相册
    assert!(db
        .asset_albums(derived)
        .unwrap()
        .iter()
        .any(|a| a.id == album.id));

    // 幂等重放：同文件再导入 → skipped，不产生第二份
    let result2 = lr_export_import_core(&state, &db, &matches, None, None).unwrap();
    assert_eq!(result2.imported, 0);
    assert_eq!(result2.skipped, 1);
    assert_eq!(db.group_members(group).unwrap().len(), 3);

    // 原片不存在 / 回收站资产 → failed 条目
    let trashed = ins(&db, "X:/lib/DSC_0060.NEF", AssetKind::Raw, None);
    db.assets_trash_move(&[trashed]).unwrap();
    let bad = lr_export_import_core(
        &state,
        &db,
        &[
            LrExportImportItem {
                path: "X:/no/such.jpg".into(),
                source_asset_id: raw,
                basis: None,
            },
            LrExportImportItem {
                path: derived_src.to_string_lossy().into_owned(),
                source_asset_id: trashed,
                basis: None,
            },
        ],
        None,
        None,
    )
    .unwrap();
    assert_eq!(bad.imported, 0);
    assert_eq!(bad.failed.len(), 2);
}

// ---------------------------------------------------------------------------
// group_role 视图 + purge 空组清理
// ---------------------------------------------------------------------------

#[test]
fn group_role_filters_raw_only_derived_only_no_derived() {
    let (dir, state, db) = setup();
    // 组 1：raw + sooc（尚未派生）
    let raw = ins(&db, "X:/p/DSC_0100.NEF", AssetKind::Raw, None);
    let sooc = ins(&db, "X:/p/DSC_0100.JPG", AssetKind::Photo, None);
    // 孤儿单资产（未入组）
    let _loner = ins(&db, "X:/p/LONE_0001.jpg", AssetKind::Photo, None);

    let page = |role: &str| {
        let mut f = AssetFilters {
            group_role: Some(role.into()),
            ..Default::default()
        };
        f.group_role = Some(role.into());
        let mut names: Vec<String> = ipc::assets::fetch_assets_page(&state, 0, 50, f)
            .unwrap()
            .into_iter()
            .map(|d| d.name)
            .collect();
        names.sort();
        names
    };

    // raw_only：自身非派生件 → raw + sooc + 孤儿
    assert_eq!(
        page("raw_only"),
        vec!["DSC_0100.JPG", "DSC_0100.NEF", "LONE_0001.jpg"]
    );
    // derived_only：无
    assert!(page("derived_only").is_empty());
    // no_derived：全部（都还没成片）
    assert_eq!(page("no_derived").len(), 3);

    // 未知 token 容错不过滤
    assert_eq!(page("bogus").len(), 3);

    // 导入成片 → 组 1 有 derived
    let photo_root = dir.path().join("photos");
    let export_dir = dir.path().join("lr_export");
    std::fs::create_dir_all(&export_dir).unwrap();
    let src = export_dir.join("DSC_0100_final.jpg");
    write_gray_jpeg(&src, 90);
    let result = lr_export_import_core(
        &state,
        &db,
        &[LrExportImportItem {
            path: src.to_string_lossy().into_owned(),
            source_asset_id: raw,
            basis: None,
        }],
        None,
        None,
    )
    .unwrap();
    assert_eq!(result.imported, 1);

    let derived_path = photo_root
        .join("SmartPhoto")
        .join("DSC_0100")
        .join("DSC_0100_final.jpg");
    let derived = db
        .asset_id_by_path(&derived_path.to_string_lossy())
        .unwrap()
        .unwrap();

    assert_eq!(
        page("raw_only"),
        vec!["DSC_0100.JPG", "DSC_0100.NEF", "LONE_0001.jpg"],
        "raw_only 不含派生件"
    );
    assert_eq!(page("derived_only"), vec!["DSC_0100_final.jpg"]);
    // no_derived：组 1 已有成片 → raw/sooc 退出；孤儿仍在
    assert_eq!(page("no_derived"), vec!["LONE_0001.jpg"]);

    // purge 成片 → 组回到 raw+sooc（空组清理不误删有员组）
    db.assets_trash_move(&[derived]).unwrap();
    db.assets_delete_rows(&[derived]).unwrap();
    assert_eq!(
        db.group_members(group_of(&db, raw).unwrap()).unwrap().len(),
        2
    );

    // 清掉全组成员 → 空组壳被删除
    let group = group_of(&db, raw).unwrap();
    db.assets_delete_rows(&[raw, sooc]).unwrap();
    let remains: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM photo_group WHERE id = ?1",
            [group],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remains, 0, "空组壳应被清理");
}
