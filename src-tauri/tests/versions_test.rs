//! 孪生分组（0017 photo_group，用户定案简化后仅存的原片-成片痕迹）：
//! 孪生导入自动建组（raw+sooc）、存量 pair 回填幂等（DB 路径 + scripts
//! 同语义）、版本查询（asset_versions，查看器 chips 数据源）、purge 空组
//! 清理。LR 成片导回与 group_role 视图已随原成片概念移除（0020）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::AssetRow;
use events::AssetKind;
use ipc::versions::asset_versions_core;

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

// ---------------------------------------------------------------------------
// purge 空组清理（孪生全清后组壳删除；LR 成片相关用例已随 0020 移除）
// ---------------------------------------------------------------------------

#[test]
fn purge_all_members_cleans_up_empty_group_shell() {
    let (_dir, _state, db) = setup();
    let raw = ins(&db, "X:/p/DSC_0100.NEF", AssetKind::Raw, None);
    let sooc = ins(&db, "X:/p/DSC_0100.JPG", AssetKind::Photo, None);
    let group = group_of(&db, raw).unwrap();
    assert_eq!(group_of(&db, sooc), Some(group));

    // 清掉全组成员 → 空组壳被删除；单成员组保留
    db.assets_trash_move(&[raw]).unwrap();
    db.assets_delete_rows(&[raw]).unwrap();
    let remains: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM photo_group WHERE id = ?1",
            [group],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remains, 1, "还有 sooc 成员，组保留");
    db.assets_trash_move(&[sooc]).unwrap();
    db.assets_delete_rows(&[sooc]).unwrap();
    let remains: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM photo_group WHERE id = ?1",
            [group],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(remains, 0, "空组壳应被清理");
}
