//! M9 相册（0015，纯引用照片组）：迁移落地与 FK 级联、CRUD/重名拒绝、
//! 幂等入册与失效 id 跳过、资产永久删除（duplicate_delete 路径）引用自动
//! 消失、删相册只删引用、相册内 captured_at keyset 分页 + filters 求交、
//! 全局 assets_page 的 albumId 筛选、导入挂相册（同事务 + 中断 resume
//! 不重不漏 + 相册被删优雅降级 + 不挂时行为不变）、空相册合法、侧栏
//! 真实 COUNT(album)。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, settings, tasks, thumbs,
};

use std::time::Duration;

use common::{
    build_many, count_assets, open_db, plan_for, state_with_library, wait_completed, SlowSource,
};
use db::AssetRow;
use devices::volume::VolumeSource;
use events::{AssetKind, EventBus, FileState};
use import::engine::Engine;
use ipc::album::{
    fetch_album_add_assets, fetch_album_assets_page, fetch_album_cover_set, fetch_album_create,
    fetch_album_delete, fetch_album_list, fetch_album_rename,
};
use ipc::assets::{fetch_assets_page, AssetDto, AssetFilters};

/// 直插一行资产（path 唯一），返回自增 id。
fn ins(db: &db::Db, path: &str, captured: Option<&str>, kind: AssetKind) -> i64 {
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
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();
    db.asset_id_by_path(path).unwrap().unwrap()
}

fn album_item_count(db: &db::Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM album_item", [], |r| r.get(0))
        .unwrap()
}

// ---------------------------------------------------------------------------
// 迁移与 FK 级联配置
// ---------------------------------------------------------------------------

#[test]
fn schema_creates_album_schema_with_fk_actions() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());

    for object in [
        "album",
        "album_item",
        "idx_album_item_asset",
        "idx_album_item_album_added",
    ] {
        let n: i64 =
            db.0.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = ?1",
                [object],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1, "对象 {object} 应随 0015 建立");
    }

    // FK 动作：album_item 双外键均 CASCADE；album.cover → SET NULL。
    // （连接级 foreign_keys=ON 由 Db::open 保证，级联真实生效——下方各
    // 行为测试覆盖。）
    let (album_fk, asset_fk): (String, String) =
        db.0.query_row(
            "SELECT (SELECT GROUP_CONCAT(on_delete) FROM pragma_foreign_key_list('album_item') \
                    WHERE \"table\" = 'album'), \
                    (SELECT GROUP_CONCAT(on_delete) FROM pragma_foreign_key_list('album_item') \
                    WHERE \"table\" = 'assets')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(album_fk.to_uppercase(), "CASCADE");
    assert_eq!(asset_fk.to_uppercase(), "CASCADE");
    let cover_fk: String =
        db.0.query_row(
            "SELECT on_delete FROM pragma_foreign_key_list('album')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cover_fk.to_uppercase(), "SET NULL");

    // 幂等：重开连接（单版本 schema）对象不重复
    drop(db);
    let db = open_db(dir.path());
    let n: i64 =
        db.0.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name = 'album'", [], |r| r.get(0))
            .unwrap();
    assert_eq!(n, 1, "重开不得重复建表");
}

// ---------------------------------------------------------------------------
// CRUD / 重名 / 空相册
// ---------------------------------------------------------------------------

#[test]
fn album_create_rename_duplicate_and_list_order() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    // trim 归一；空名拒绝
    let a = fetch_album_create(&state, "  旅行 2026  ").unwrap();
    assert_eq!(a.name, "旅行 2026");
    assert_eq!(a.item_count, 0);
    assert!(a.cover_asset_id.is_none());
    let err = fetch_album_create(&state, "   ").unwrap_err();
    assert!(err.contains("不能为空"), "{err}");

    // 重名拒绝（UNIQUE 兜底转文案）
    let err = fetch_album_create(&state, "旅行 2026").unwrap_err();
    assert!(err.contains("同名相册"), "{err}");

    // 列表 createdAt DESC（直改时间戳造确定性序）；空相册合法返回
    let b = fetch_album_create(&state, "家庭").unwrap();
    let db = open_db(db_dir.path());
    db.0.execute(
        "UPDATE album SET created_at = ?2 WHERE id = ?1",
        rusqlite::params![a.id, "2026-01-01T00:00:00.000Z"],
    )
    .unwrap();
    db.0.execute(
        "UPDATE album SET created_at = ?2 WHERE id = ?1",
        rusqlite::params![b.id, "2026-02-01T00:00:00.000Z"],
    )
    .unwrap();
    let list = fetch_album_list(&state).unwrap();
    assert_eq!(
        list.iter().map(|x| x.id).collect::<Vec<_>>(),
        vec![b.id, a.id],
        "createdAt DESC"
    );
    assert!(list.iter().all(|x| x.item_count == 0), "空相册合法");

    // 重命名：trim / 空拒绝；撞名拒绝；相册不存在明确报错
    fetch_album_rename(&state, a.id, "  旅行精选 ").unwrap();
    assert_eq!(fetch_album_list(&state).unwrap()[1].name, "旅行精选");
    let err = fetch_album_rename(&state, a.id, "家庭").unwrap_err();
    assert!(err.contains("同名相册"), "{err}");
    let err = fetch_album_rename(&state, a.id, "  ").unwrap_err();
    assert!(err.contains("不能为空"), "{err}");
    let err = fetch_album_rename(&state, 9999, "新名").unwrap_err();
    assert!(err.contains("相册不存在"), "{err}");
}

#[test]
fn album_cover_set_clear_and_validation() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "封面册").unwrap();
    let asset = ins(
        &open_db(db_dir.path()),
        "X:/p/cover.jpg",
        None,
        AssetKind::Photo,
    );

    fetch_album_cover_set(&state, album.id, Some(asset)).unwrap();
    assert_eq!(
        fetch_album_list(&state).unwrap()[0].cover_asset_id,
        Some(asset)
    );
    // 清回默认
    fetch_album_cover_set(&state, album.id, None).unwrap();
    assert_eq!(fetch_album_list(&state).unwrap()[0].cover_asset_id, None);
    // 资产不存在 / 相册不存在
    let err = fetch_album_cover_set(&state, album.id, Some(9999)).unwrap_err();
    assert!(err.contains("封面资产不存在"), "{err}");
    let err = fetch_album_cover_set(&state, 9999, Some(asset)).unwrap_err();
    assert!(err.contains("相册不存在"), "{err}");
}

// ---------------------------------------------------------------------------
// 引用增删：幂等 / 失效 id / 相册不存在
// ---------------------------------------------------------------------------

#[test]
fn album_add_idempotent_skips_missing_assets_and_requires_album() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "引用册").unwrap();
    let db = open_db(db_dir.path());
    let a1 = ins(&db, "X:/p/1.jpg", None, AssetKind::Photo);
    let a2 = ins(&db, "X:/p/2.jpg", None, AssetKind::Photo);

    // 首次：2 新增（重复入参去重；失效资产 id 静默跳过不计）
    let added = fetch_album_add_assets(&state, album.id, &[a1, a2, a2, 999_999], None).unwrap();
    assert_eq!(added, 2);
    // 再次全量重放：0 新增（INSERT OR IGNORE 幂等）
    let added = fetch_album_add_assets(&state, album.id, &[a1, a2, 999_999], None).unwrap();
    assert_eq!(added, 0);
    assert_eq!(fetch_album_list(&state).unwrap()[0].item_count, 2);
    // 相册不存在
    let err = fetch_album_add_assets(&state, 9999, &[a1], None).unwrap_err();
    assert!(err.contains("相册不存在"), "{err}");

    // 移除（db 层；album_remove_assets 命令已删——一照一册下 UI 走回收站，
    // db 方法仍是归册挪移的核）：单移 + 不在册 id 幂等
    db.album_remove_assets(album.id, &[a1, 888_888]).unwrap();
    assert_eq!(fetch_album_list(&state).unwrap()[0].item_count, 1);
    db.album_remove_assets(album.id, &[a1]).unwrap(); // 已移除：无错
}

// ---------------------------------------------------------------------------
// 级联语义：删资产 → 引用消失 / 删相册 → 只删引用
// ---------------------------------------------------------------------------

#[test]
fn asset_permanent_delete_cascades_references_and_cover() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "级联册").unwrap();

    // 真实临时文件（duplicate_delete 同步删盘 + 删库行）
    let file_dir = tempfile::tempdir().unwrap();
    let path = file_dir.path().join("a.jpg");
    std::fs::write(&path, b"jpeg").unwrap();
    let db = open_db(db_dir.path());
    let id = ins(&db, &path.to_string_lossy(), None, AssetKind::Photo);
    fetch_album_add_assets(&state, album.id, &[id], None).unwrap();
    fetch_album_cover_set(&state, album.id, Some(id)).unwrap();

    // 永久删除路径（duplicate_delete）
    let deleted = ipc::duplicates::fetch_duplicate_delete(&state, &[id]).unwrap();
    assert_eq!(deleted, 1);
    assert!(!path.exists(), "物理文件删除（资产删除语义，与相册无关）");

    // 引用自动消失（FK CASCADE）；相册保留；封面 SET NULL
    assert_eq!(album_item_count(&db), 0);
    let row = fetch_album_list(&state).unwrap();
    assert_eq!(row.len(), 1);
    assert_eq!(row[0].id, album.id);
    assert_eq!(row[0].item_count, 0);
    assert_eq!(row[0].cover_asset_id, None, "封面引用 SET NULL");
}

#[test]
fn album_delete_trashes_members_and_restore_lands_in_default_album() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "待删册").unwrap();
    let db = open_db(db_dir.path());
    let a1 = ins(&db, "X:/p/1.jpg", None, AssetKind::Photo);
    let a2 = ins(&db, "X:/p/2.jpg", None, AssetKind::Photo);
    fetch_album_add_assets(&state, album.id, &[a1, a2], None).unwrap();
    assert_eq!(count_assets(&db), 2);

    // 一照一册模型：删相册 = 成员全部软删入回收站（行保留 in_trash=1），
    // 引用级联消失、列表为空
    fetch_album_delete(&state, album.id).unwrap();
    assert_eq!(count_assets(&db), 2, "软删不删行");
    assert_eq!(
        db.0
            .query_row("SELECT COUNT(*) FROM assets WHERE in_trash = 1", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2,
        "成员全部在回收站"
    );
    assert_eq!(album_item_count(&db), 0);
    assert!(fetch_album_list(&state).unwrap().is_empty());

    // 还原：原册已删 → 落入默认相册「未分组」
    assert_eq!(ipc::selection::fetch_trash_restore(&state, &[a1, a2]).unwrap(), 2);
    let default_id = db.ensure_default_album().unwrap();
    let in_default: i64 = db.0
        .query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1 AND asset_id IN (?2, ?3)",
            rusqlite::params![default_id, a1, a2],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_default, 2, "还原后归属「未分组」");
    assert_eq!(count_assets(&db), 2);

    // 再删（不存在）明确报错
    let err = fetch_album_delete(&state, album.id).unwrap_err();
    assert!(err.contains("相册不存在"), "{err}");
    // 默认相册「未分组」拒删
    let err = fetch_album_delete(&state, default_id).unwrap_err();
    assert!(err.contains("不可删除"), "{err}");
}

// ---------------------------------------------------------------------------
// 相册内时间线分页 + filters 求交 + 全局 albumId 筛选
// ---------------------------------------------------------------------------

#[test]
fn album_assets_page_keyset_order_and_filter_intersection() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "时间线册").unwrap();
    let db = open_db(db_dir.path());
    // id 递增；NULL captured_at 置顶，随后 captured 降序、id 倒序 tiebreak
    let a_null1 = ins(&db, "X:/p/null1.jpg", None, AssetKind::Photo);
    let a_old = ins(
        &db,
        "X:/p/old.jpg",
        Some("2025-01-01T00:00:00.000Z"),
        AssetKind::Photo,
    );
    let a_new = ins(
        &db,
        "X:/p/new.jpg",
        Some("2026-06-01T00:00:00.000Z"),
        AssetKind::Photo,
    );
    let a_video = ins(
        &db,
        "X:/p/v.mp4",
        Some("2026-07-01T00:00:00.000Z"),
        AssetKind::Video,
    );
    let a_out = ins(
        &db,
        "X:/p/out.jpg",
        Some("2026-08-01T00:00:00.000Z"),
        AssetKind::Photo,
    );
    fetch_album_add_assets(&state, album.id, &[a_null1, a_old, a_new, a_video], None).unwrap();

    // 旧视频记录不出现在相册时间线。排序 = captured 降序 + id 倒序
    // tiebreak，NULL captured（未知时间）沉底（fdd827c 2026-10-07 起与
    // 画廊分页/日期分组一致的「unknown 沉底」语义）。
    let page1: Vec<i64> = fetch_album_assets_page(&state, album.id, 0, 2, None)
        .unwrap()
        .iter()
        .map(|d: &AssetDto| d.id)
        .collect();
    assert_eq!(page1, vec![a_new, a_old]);
    // 第二页：仅 a_null1（未知时间沉底）；a_out 不在册
    let page2: Vec<i64> = fetch_album_assets_page(&state, album.id, a_old, 2, None)
        .unwrap()
        .iter()
        .map(|d: &AssetDto| d.id)
        .collect();
    assert_eq!(page2, vec![a_null1]);
    assert!(!page2.contains(&a_out), "相册外资产不进入相册时间线");
    // 尾页空
    assert!(fetch_album_assets_page(&state, album.id, a_null1, 2, None)
        .unwrap()
        .is_empty());

    // 旧视频筛选条件也不能让视频重新出现。
    let filters = AssetFilters {
        kinds: vec![AssetKind::Video],
        ..AssetFilters::default()
    };
    let only_video: Vec<i64> = fetch_album_assets_page(&state, album.id, 0, 10, Some(filters))
        .unwrap()
        .iter()
        .map(|d: &AssetDto| d.id)
        .collect();
    assert!(only_video.is_empty());

    // 空相册合法：空页
    let empty = fetch_album_create(&state, "空册").unwrap();
    assert!(fetch_album_assets_page(&state, empty.id, 0, 10, None)
        .unwrap()
        .is_empty());
    // 相册不存在明确报错
    let err = fetch_album_assets_page(&state, 9999, 0, 10, None).unwrap_err();
    assert!(err.contains("相册不存在"), "{err}");
}

#[test]
fn global_assets_page_filters_by_album_id() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let x = fetch_album_create(&state, "X").unwrap();
    let y = fetch_album_create(&state, "Y").unwrap();
    let db = open_db(db_dir.path());
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    let c = ins(&db, "X:/p/c.mp4", None, AssetKind::Video);
    let shared = ins(&db, "X:/p/shared.jpg", None, AssetKind::Photo);
    fetch_album_add_assets(&state, x.id, &[a, shared], None).unwrap();
    fetch_album_add_assets(&state, y.id, &[b, c, shared], None).unwrap(); // 同一照片可入多相册

    let in_x = AssetFilters {
        album_id: Some(x.id),
        ..AssetFilters::default()
    };
    let ids: Vec<i64> = fetch_assets_page(&state, 0, 50, in_x)
        .unwrap()
        .iter()
        .map(|d| d.id)
        .collect();
    assert!(ids.contains(&a) && ids.contains(&shared) && !ids.contains(&b) && !ids.contains(&c));

    // 组合既有维度：albumId(Y) ∩ 旧视频筛选条件 → 空集。
    let y_video = AssetFilters {
        album_id: Some(y.id),
        kinds: vec![AssetKind::Video],
        ..AssetFilters::default()
    };
    let ids: Vec<i64> = fetch_assets_page(&state, 0, 50, y_video)
        .unwrap()
        .iter()
        .map(|d| d.id)
        .collect();
    assert!(ids.is_empty());

    // 不存在的相册 = 空集（筛选语义，非报错）
    let none = AssetFilters {
        album_id: Some(9999),
        ..AssetFilters::default()
    };
    assert!(fetch_assets_page(&state, 0, 50, none).unwrap().is_empty());

    // camelCase 契约：albumId 反序列化
    let parsed: AssetFilters =
        serde_json::from_str(r#"{"albumId": 7, "kinds": ["photo"]}"#).unwrap();
    assert_eq!(parsed.album_id, Some(7));
}

// ---------------------------------------------------------------------------
// 导入挂相册（可选）：同事务 / resume 不重不漏 / 优雅降级 / 不挂不变
// ---------------------------------------------------------------------------

/// 引擎直跑工具：album 覆盖 + 会话一取消（首个文件完成后）→ 返回
/// (job_id, 部分统计)。
fn import_cancelled_after_first(
    src: &std::path::Path,
    db_dir: &std::path::Path,
    target: &std::path::Path,
    n: usize,
    album_id: Option<i64>,
) -> (i64, events::JobStats) {
    build_many(src, n);
    let db = open_db(db_dir);
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut plan = plan_for(&db, db_dir, target);
    plan.album_id = album_id;
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src),
            delay: Duration::from_millis(12),
        }),
        plan,
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());
    wait_completed(&mut rx);
    controls.cancel();
    let stats = handle.join().unwrap();
    (job_id, stats)
}

#[test]
fn import_with_album_resume_is_exact_no_dup_no_miss() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "导入册").unwrap();
    let n = 10;

    // 会话一：挂相册导入，首个文件完成后中断
    let (job_id, partial) =
        import_cancelled_after_first(src.path(), db_dir.path(), target.path(), n, Some(album.id));
    assert!(partial.done_files < n as u64, "软取消应停在部分完成");

    // 同事务保证：已 verified 的文件引用恰好入册（无「有资产无引用」态）
    let db = open_db(db_dir.path());
    let verified = common::journal_states(&db, job_id)
        .into_iter()
        .filter(|s| *s == FileState::Verified)
        .count() as i64;
    let in_album: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM album_item WHERE album_id = ?1",
            [album.id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(in_album, verified, "中断时引用数 == 已完成文件数");

    // job 持久化了 album_id（resume 据此重建，不重不漏的前提）
    let plan_json = db.job_plan_json(job_id).unwrap().unwrap();
    let plan: import::engine::ImportPlan = serde_json::from_str(&plan_json).unwrap();
    assert_eq!(plan.album_id, Some(album.id));

    // 会话二：resume 续跑（plan 从 journal 重建，与 ipc::resume_import 同构）
    let bus = EventBus::new();
    let engine = Engine::resume(
        open_db(db_dir.path()),
        bus,
        Box::new(VolumeSource::new(src.path())),
        plan,
        job_id,
    )
    .unwrap();
    let stats = engine.run();
    assert_eq!(stats.done_files, n as u64, "续传后无遗漏");
    assert_eq!(stats.failed_files, 0);

    // 不重不漏：每资产恰一条引用；itemCount == 资产总数
    let db = open_db(db_dir.path());
    assert_eq!(count_assets(&db), n as i64);
    assert_eq!(album_item_count(&db), n as i64);
    assert_eq!(fetch_album_list(&state).unwrap()[0].item_count, n as u64);
}

#[test]
fn import_without_album_keeps_behavior_unchanged() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let _state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    // 2026-10-09 §一/§三：相册纯逻辑引用——album_id 可选（None = 不挂相册，
    // 画廊全局视图自然可见）；带 album_id 时契约仍序列化 albumId（resume 依赖）
    build_many(src.path(), 6);
    let db = open_db(db_dir.path());
    let plan = plan_for(&db, db_dir.path(), target.path());
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    engine.begin().expect("无 album_id 应正常导入（相册可选）");

    let state_db = open_db(db_dir.path());
    let album_id = state_db.ensure_default_album().unwrap();
    let mut plan = plan_for(&state_db, db_dir.path(), target.path());
    plan.album_id = Some(album_id);
    let plan_json = serde_json::to_string(&plan).unwrap();
    assert!(
        plan_json.contains("albumId"),
        "plan 契约应序列化 albumId 字段: {plan_json}"
    );
}

#[test]
fn import_into_deleted_album_degrades_gracefully() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let album = fetch_album_create(&state, "将被删").unwrap();

    // 相册在导入启动前已被用户删除（并发窗口）：0018 起 begin 阶段显式
    // 拒绝（相册目录名缺失，落点无从谈起）；导入中途删相册的窗口仍由
    // insert_asset_on 的 album 存活检查优雅降级（跳过挂载不判失败）。
    fetch_album_delete(&state, album.id).unwrap();
    build_many(src.path(), 6);
    let db = open_db(db_dir.path());
    let mut plan = plan_for(&db, db_dir.path(), target.path());
    plan.album_id = Some(album.id);
    let mut engine = Engine::new(
        db,
        EventBus::new(),
        Box::new(VolumeSource::new(src.path())),
        plan,
    );
    let err = engine.begin().expect_err("已删相册应拒绝导入");
    assert!(err.to_string().contains("不存在"), "{err}");
    // 拒绝未产生任务
    let db = open_db(db_dir.path());
    assert_eq!(
        db.0.query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

// ---------------------------------------------------------------------------
// 侧栏计数：albums 真实 COUNT(album)，tags 维持词表常量
// ---------------------------------------------------------------------------

#[test]
fn sidebar_albums_count_is_real_count() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let counts = ipc::insights::fetch_sidebar_counts(&state).unwrap();
    assert_eq!(counts.albums, 0);
    fetch_album_create(&state, "一册").unwrap();
    fetch_album_create(&state, "二册").unwrap();
    let counts = ipc::insights::fetch_sidebar_counts(&state).unwrap();
    assert_eq!(counts.albums, 2, "真实 COUNT(album)");
    assert_eq!(
        counts.tags,
        ipc::insights::SMART_ALBUM_TAG_COUNT,
        "tags 不动"
    );
    // 删相册后计数回落
    let id = fetch_album_list(&state).unwrap()[0].id;
    fetch_album_delete(&state, id).unwrap();
    assert_eq!(
        ipc::insights::fetch_sidebar_counts(&state).unwrap().albums,
        1
    );
}

// ---------------------------------------------------------------------------
// 资产 → 所属相册反查（asset_albums；查看器详情「所属相册」行）
// ---------------------------------------------------------------------------

#[test]
fn asset_albums_reverse_lookup_order_and_empty() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    let a1 = ins(&db, "X:/p/1.jpg", None, AssetKind::Photo);
    let a2 = ins(&db, "X:/p/2.jpg", None, AssetKind::Photo);
    let first = fetch_album_create(&state, "甲册").unwrap();
    let second = fetch_album_create(&state, "乙册").unwrap();
    fetch_album_add_assets(&state, first.id, &[a1], None).unwrap();
    fetch_album_add_assets(&state, second.id, &[a1, a2], None).unwrap();

    // a1 入两册：createdAt DESC → 乙册在前；itemCount 反映全量引用
    let albums = db.asset_albums(a1).unwrap();
    assert_eq!(
        albums.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["乙册", "甲册"]
    );
    assert_eq!(albums[0].item_count, 2);

    // a2 只入一册；未入册资产返回空（不报错）
    assert_eq!(db.asset_albums(a2).unwrap().len(), 1);
    let a3 = ins(&db, "X:/p/3.jpg", None, AssetKind::Photo);
    assert!(db.asset_albums(a3).unwrap().is_empty());

    // 移出后反查同步收窄
    db.album_remove_assets(second.id, &[a1]).unwrap();
    assert_eq!(db.asset_albums(a1).unwrap().len(), 1);
}
