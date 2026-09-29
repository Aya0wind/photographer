//! 相册子分组（0019）：album_item.subgroup 命名层（NULL = 相册根）。
//! 子分组清单（DISTINCT+计数升序）、根/子分组视图分页（subgroup /
//! subgroup_is_null 过滤，默认整个相册）、加引用带子分组（重复加引用
//! None 保持 / Some 改写）、挪子分组账本基元（根↔组↔组；0022 物理化后
//! IPC 层先物理挪移，db 层基元仍是纯引用 UPDATE——物理一致性见
//! album_subgroup_physical_test）、导入带子分组（物理落子文件夹 + resume
//! 幂等）、claim 带子分组（物理落子文件夹）、lr_export_import 成片入子分组。

mod common;

use common::library_fixture as setup;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::{build_source, open_db, run_engine};
use db::{AssetFilters, AssetRow};
use events::AssetKind;
use ipc::album::fetch_album_subgroups;
use ipc::claim::fetch_album_claim_assets;


fn ins(db: &db::Db, path: &str, captured: Option<&str>) -> i64 {
    db.insert_asset(&AssetRow {
        path: path.to_string(),
        filename: path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
        size: 100,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: path.len() as u64,
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
    })
    .unwrap();
    db.asset_id_by_path(path).unwrap().unwrap()
}

fn subgroup_of(db: &db::Db, album_id: i64, asset_id: i64) -> Option<String> {
    db.0.query_row(
        "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
        rusqlite::params![album_id, asset_id],
        |r| r.get(0),
    )
    .unwrap()
}

fn page_names(state: &ipc::AppState, album_id: i64, filters: AssetFilters) -> Vec<String> {
    ipc::album::fetch_album_assets_page(state, album_id, 0, 50, Some(filters))
        .unwrap()
        .into_iter()
        .map(|d| d.name)
        .collect()
}

// ---------------------------------------------------------------------------
// 子分组清单
// ---------------------------------------------------------------------------

#[test]
fn album_subgroups_list_distinct_counts_ascending() {
    let (_dir, state, db) = setup();
    let album = db.album_create("旅拍").unwrap();

    // 空相册 → 空清单
    assert!(fetch_album_subgroups(&state, album.id).unwrap().is_empty());

    let a = ins(&db, "X:/p/a.jpg", None);
    let b = ins(&db, "X:/p/b.jpg", None);
    let c = ins(&db, "X:/p/c.jpg", None);
    let d = ins(&db, "X:/p/d.jpg", None);
    db.album_add_assets(album.id, &[a, b], Some("成片"))
        .unwrap();
    db.album_add_assets(album.id, &[c], Some("原片")).unwrap();
    db.album_add_assets(album.id, &[d], None).unwrap(); // 散在根

    let list = fetch_album_subgroups(&state, album.id).unwrap();
    assert_eq!(
        list,
        vec![
            ipc::album::AlbumSubgroupDto {
                name: "原片".into(),
                item_count: 1
            },
            ipc::album::AlbumSubgroupDto {
                name: "成片".into(),
                item_count: 2
            },
        ],
        "name 升序、计数正确；根散照片不入清单"
    );

    // 相册不存在报错
    assert!(fetch_album_subgroups(&state, 999).is_err());
}

// ---------------------------------------------------------------------------
// 根 / 子分组视图分页
// ---------------------------------------------------------------------------

#[test]
fn album_assets_page_subgroup_views_root_and_default() {
    let (_dir, state, db) = setup();
    let album = db.album_create("视图册").unwrap();
    let a = ins(&db, "X:/p/a.jpg", Some("2026-01-01T00:00:00.000Z"));
    let b = ins(&db, "X:/p/b.jpg", Some("2026-01-02T00:00:00.000Z"));
    let c = ins(&db, "X:/p/c.jpg", Some("2026-01-03T00:00:00.000Z"));
    db.album_add_assets(album.id, &[a], None).unwrap(); // 根
    db.album_add_assets(album.id, &[b], Some("成片")).unwrap();
    db.album_add_assets(album.id, &[c], Some("成片")).unwrap();

    // 默认（不传）= 整个相册（根 + 所有子分组，按 captured_at）
    assert_eq!(
        page_names(&state, album.id, AssetFilters::default()),
        vec!["c.jpg", "b.jpg", "a.jpg"]
    );
    // subgroup 精确匹配
    assert_eq!(
        page_names(
            &state,
            album.id,
            AssetFilters {
                subgroup: Some("成片".into()),
                ..Default::default()
            }
        ),
        vec!["c.jpg", "b.jpg"]
    );
    // subgroup_is_null = true → 只看根
    assert_eq!(
        page_names(
            &state,
            album.id,
            AssetFilters {
                subgroup_is_null: Some(true),
                ..Default::default()
            }
        ),
        vec!["a.jpg"]
    );
    // 不存在的子分组 → 空集
    assert_eq!(
        page_names(
            &state,
            album.id,
            AssetFilters {
                subgroup: Some("不存在".into()),
                ..Default::default()
            }
        ),
        Vec::<String>::new()
    );
    // 全局 assets_page（无 album_id）不受子分组过滤误伤
    assert_eq!(
        ipc::assets::fetch_assets_page(
            &state,
            0,
            50,
            AssetFilters {
                subgroup: Some("成片".into()),
                ..Default::default()
            }
        )
        .unwrap()
        .len(),
        2
    );
}

// ---------------------------------------------------------------------------
// 加引用带子分组 / 挪子分组
// ---------------------------------------------------------------------------

#[test]
fn album_add_assets_with_subgroup_and_move_between() {
    let (_dir, _state, db) = setup();
    let album = db.album_create("分组册").unwrap();
    let a = ins(&db, "X:/p/a.jpg", None);
    let b = ins(&db, "X:/p/b.jpg", None);
    let stranger = ins(&db, "X:/p/stranger.jpg", None);

    // 带子分组加引用
    assert_eq!(
        db.album_add_assets(album.id, &[a], Some("成片")).unwrap(),
        1
    );
    assert_eq!(subgroup_of(&db, album.id, a).as_deref(), Some("成片"));

    // 默认（None）加引用 = 根（兼容既有用例语义）
    assert_eq!(db.album_add_assets(album.id, &[b], None).unwrap(), 1);
    assert_eq!(subgroup_of(&db, album.id, b), None);

    // 重复加引用（None）→ 幂等且保持原子分组
    assert_eq!(db.album_add_assets(album.id, &[a], None).unwrap(), 0);
    assert_eq!(subgroup_of(&db, album.id, a).as_deref(), Some("成片"));
    // 重复加引用（Some）→ 改写归属，不新增行
    assert_eq!(
        db.album_add_assets(album.id, &[a], Some("原片")).unwrap(),
        0
    );
    assert_eq!(subgroup_of(&db, album.id, a).as_deref(), Some("原片"));
    let rows: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![album.id, a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 1, "PK(album_id, asset_id) 唯一");

    // 挪子分组：根→组
    assert_eq!(
        db.album_item_move_subgroup(album.id, &[b], Some("成片"))
            .unwrap(),
        1
    );
    assert_eq!(subgroup_of(&db, album.id, b).as_deref(), Some("成片"));
    // 组→组
    db.album_item_move_subgroup(album.id, &[b], Some("精选"))
        .unwrap();
    assert_eq!(subgroup_of(&db, album.id, b).as_deref(), Some("精选"));
    // 组→根
    db.album_item_move_subgroup(album.id, &[b], None).unwrap();
    assert_eq!(subgroup_of(&db, album.id, b), None);

    // 不在册 id → 0 行不报错；相册不存在 → 报错
    assert_eq!(
        db.album_item_move_subgroup(album.id, &[stranger], Some("x"))
            .unwrap(),
        0
    );
    assert!(db.album_item_move_subgroup(999, &[a], None).is_err());

    // 挪动后子分组清单计数随动
    db.album_item_move_subgroup(album.id, &[a, b], Some("成片"))
        .unwrap();
    let list = db.album_subgroups(album.id).unwrap();
    assert_eq!(
        list,
        vec![("成片".to_string(), 2u64)],
        "精选清空后消失，成片计数 2"
    );
}

// ---------------------------------------------------------------------------
// 导入带子分组（resume 幂等）
// ---------------------------------------------------------------------------

#[test]
fn import_with_album_subgroup_and_resume_idempotent() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let db = open_db(db_dir.path());
    let album = db.album_create("导入册").unwrap();
    build_source(src.path());

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |p| {
        p.album_id = Some(album.id);
        p.album_subgroup = Some("机内直出".into());
    });
    assert_eq!(job_status(&db, job_id), "done");
    assert_eq!(stats.done_files, 3);

    // 0022 物理化：文件真实落 {target}/{创建YYYY}/{创建MM}/{dir_name}/{子组}/
    let home = db.album_home_rel(album.id).unwrap().unwrap();
    let sub_dir = target.path().join(format!("{home}/机内直出"));
    let paths: Vec<String> = db
        .0
        .prepare("SELECT path FROM assets ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(paths.len(), 3);
    for path in &paths {
        let p = std::path::PathBuf::from(path);
        assert!(
            p.starts_with(&sub_dir),
            "导入件应落子组文件夹 {sub_dir:?}，实得 {path}"
        );
        assert!(p.is_file());
    }
    let subgroups: Vec<String> = db
        .0
        .prepare("SELECT subgroup FROM album_item WHERE album_id = ?1 AND subgroup IS NOT NULL")
        .unwrap()
        .query_map([album.id], |r| r.get(0))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(subgroups, vec!["机内直出"; 3]);

    // resume 重放：verified 跳过，不产生重复引用、子分组不变
    let bus = events::EventBus::new();
    let engine = import::engine::Engine::resume(
        open_db(db_dir.path()),
        bus,
        Box::new(devices::volume::VolumeSource::new(src.path())),
        import::engine::ImportPlan {
            source_id: "test-src".into(),
            target_root: target.path().to_path_buf(),
            name_template: "{原文件名}".into(),
            duplicate_policy: settings::DuplicatePolicy::Skip,
            skip_imported: true,
            streams: 2,
            mode: import::engine::ImportMode::Copy,
            second_target: None,
            include: None,
            album_id: Some(album.id),
            album_subgroup: Some("机内直出".into()),
        },
        job_id,
    )
    .unwrap();
    let stats2 = engine.run();
    // resume 统计基线计入 journal 已 verified 的 3 件（不重做，仅计数继承）
    assert_eq!(stats2.done_files, 3);
    let count: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1",
            [album.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 3, "resume 不重复挂引用");
}

fn job_status(db: &db::Db, job_id: i64) -> String {
    db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
        r.get(0)
    })
    .unwrap()
}

// ---------------------------------------------------------------------------
// claim / lr_export_import 带子分组
// ---------------------------------------------------------------------------

#[test]
fn claim_with_subgroup_lands_named_layer() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    let album = db.album_create("交付册").unwrap();

    let captured = "2026-06-01T10:00:00.000Z";
    let src_dir = photo_root.join("2026").join("06-01");
    std::fs::create_dir_all(&src_dir).unwrap();
    let photo = src_dir.join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), Some(captured));

    let result = fetch_album_claim_assets(&state, album.id, &[id], Some("成片")).unwrap();
    assert_eq!(result.moved, 1, "{result:?}");
    // 物理挪进相册子组文件夹（0022：唯一不平铺例外）+ 引用带子分组
    assert!(!photo.exists());
    let moved_path: String =
        db.0.query_row("SELECT path FROM assets WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
    let home = db.album_home_rel(album.id).unwrap().unwrap();
    let expected = photo_root
        .join(home.replace('/', std::path::MAIN_SEPARATOR_STR))
        .join("成片")
        .join("DSC_0001.jpg");
    assert_eq!(moved_path.replace('/', std::path::MAIN_SEPARATOR_STR), expected.to_string_lossy());
    assert!(expected.is_file());
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("成片"));

    // 幂等重试：skipped 且子分组保持
    let again = fetch_album_claim_assets(&state, album.id, &[id], Some("成片")).unwrap();
    assert_eq!(again.skipped, 1);
    assert_eq!(subgroup_of(&db, album.id, id).as_deref(), Some("成片"));
}
