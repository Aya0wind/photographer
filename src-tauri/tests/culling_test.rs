//! 选片会话 V1（0024）：快照语义（album 含子组过滤 / query 传入序）、决定
//! 批量 upsert / 回未定 / 快照外忽略计数、进度纯派生、默认名轮次（初选/
//! 复选）与同名后缀、列表序（进行中在前）、收尾三开关映射 + 幂等 + XMP
//! 即时投影（星级/拒绝断言边车）、改名/弃置、资产删除级联收敛。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::culling::CullScope;
use db::AssetRow;
use events::AssetKind;
use ipc::culling::{
    fetch_cull_decision_apply, fetch_cull_session_create, fetch_cull_session_discard,
    fetch_cull_session_finish, fetch_cull_session_list, fetch_cull_session_open,
    fetch_cull_session_rename, CullDecisionInput, CullFinishApply,
};
use metadata::xmp;

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

fn wait_until(deadline: Duration, mut pred: impl FnMut() -> bool) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if pred() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    pred()
}

fn dec(asset_id: i64, decision: Option<&str>) -> CullDecisionInput {
    CullDecisionInput {
        asset_id,
        decision: decision.map(str::to_string),
        origin: None,
    }
}

// ---------------------------------------------------------------------------
// 快照创建
// ---------------------------------------------------------------------------

#[test]
fn album_snapshot_orders_by_captured_desc_and_filters_subgroup() {
    let (_dir, state, db) = setup();
    let album = db.album_create("旅拍").unwrap();
    let t1 = ins(&db, "X:/p/t1.jpg", Some("2026-05-01T10:00:00.000Z"), AssetKind::Photo);
    let t2 = ins(&db, "X:/p/t2.jpg", Some("2026-05-01T09:00:00.000Z"), AssetKind::Photo);
    let t3 = ins(&db, "X:/p/t3.jpg", None, AssetKind::Photo); // captured NULL → 最先
    let t4 = ins(&db, "X:/p/t4.jpg", Some("2026-05-01T09:00:00.000Z"), AssetKind::Photo);
    // 同刻 tiebreak：id DESC（t4 > t2 → t4 在前）
    let video = ins(&db, "X:/p/v.mp4", Some("2026-05-02T00:00:00.000Z"), AssetKind::Video);
    db.album_add_assets(album.id, &[t1, t2, t3, t4, video], Some("成片")).unwrap();
    let root_only = ins(&db, "X:/p/root.jpg", Some("2026-05-01T08:00:00.000Z"), AssetKind::Photo);
    db.album_add_assets(album.id, &[root_only], None).unwrap();

    // 整册：拍摄时间序（NULL 最先）+ kind 过滤（video 不入）+ 相册外不入
    let dto = fetch_cull_session_create(
        &state,
        CullScope::Album {
            album_id: album.id,
            subgroup: None,
        },
    )
    .unwrap();
    assert_eq!(
        fetch_cull_session_open(&state, dto.id).unwrap().items.iter().map(|i| i.asset_id).collect::<Vec<_>>(),
        vec![t3, t1, t4, t2, root_only],
        "整册快照 = NULL 最先 + 时间降序 + id 降序 tiebreak"
    );
    assert_eq!(dto.total, 5);
    assert_eq!(dto.name, "相册「旅拍」 · 初选");

    // 子分组：只看成片组
    let dto2 = fetch_cull_session_create(
        &state,
        CullScope::Album {
            album_id: album.id,
            subgroup: Some("成片".into()),
        },
    )
    .unwrap();
    let ids: Vec<i64> = fetch_cull_session_open(&state, dto2.id)
        .unwrap()
        .items
        .iter()
        .map(|i| i.asset_id)
        .collect();
    assert_eq!(ids, vec![t3, t1, t4, t2], "子组快照过滤 root_only/video/outside");
    assert_eq!(dto2.name, "相册「旅拍」/成片 · 初选", "子组是独立来源池");

    // 相册不存在
    assert!(fetch_cull_session_create(
        &state,
        CullScope::Album {
            album_id: 9999,
            subgroup: None,
        },
    )
    .is_err());
}

#[test]
fn query_snapshot_keeps_input_order_dedupes_and_drops_missing() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    let c = ins(&db, "X:/p/c.jpg", None, AssetKind::Photo);

    let dto = fetch_cull_session_create(
        &state,
        CullScope::Query {
            // 传入序 c,a,b,a + 不存在的 4242：去重保序 + 存在性过滤
            asset_ids: vec![c, a, b, a, 4242],
        },
    )
    .unwrap();
    let ids: Vec<i64> = fetch_cull_session_open(&state, dto.id)
        .unwrap()
        .items
        .iter()
        .map(|i| i.asset_id)
        .collect();
    assert_eq!(ids, vec![c, a, b], "query 快照 = 传入序（去重 + 不存在剔除）");
    assert_eq!(dto.total, 3);
    assert_eq!(dto.name, "筛选快照 5 张 · 初选", "默认名按传入张数描述");

    // 空 assetIds 拒绝
    assert!(fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![] },
    )
    .is_err());
    // 全不存在拒绝
    assert!(fetch_cull_session_create(
        &state,
        CullScope::Query {
            asset_ids: vec![777, 888],
        },
    )
    .is_err());
}

#[test]
fn default_name_rounds_and_suffixes_duplicates() {
    let (_dir, state, db) = setup();
    let album = db.album_create("婚礼").unwrap();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    db.album_add_assets(album.id, &[a], None).unwrap();

    let s1 = fetch_cull_session_create(
        &state,
        CullScope::Album { album_id: album.id, subgroup: None },
    )
    .unwrap();
    assert_eq!(s1.name, "相册「婚礼」 · 初选");
    let s2 = fetch_cull_session_create(
        &state,
        CullScope::Album { album_id: album.id, subgroup: None },
    )
    .unwrap();
    assert_eq!(s2.name, "相册「婚礼」 · 复选2", "同来源第二个 = 复选2");
    let _s3 = fetch_cull_session_create(
        &state,
        CullScope::Album { album_id: album.id, subgroup: None },
    )
    .unwrap();

    // 改名占位下一轮将生成的名字（复选4）→ 新建自动「·2」后缀
    fetch_cull_session_rename(&state, s1.id, "相册「婚礼」 · 复选4").unwrap();
    let s4 = fetch_cull_session_create(
        &state,
        CullScope::Album { album_id: album.id, subgroup: None },
    )
    .unwrap();
    assert_eq!(s4.name, "相册「婚礼」 · 复选4·2");
}

// ---------------------------------------------------------------------------
// 决定读写 + 计数派生
// ---------------------------------------------------------------------------

#[test]
fn decisions_upsert_revert_ignore_and_counts_derive() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    let c = ins(&db, "X:/p/c.jpg", None, AssetKind::Photo);
    let outside = ins(&db, "X:/p/out.jpg", None, AssetKind::Photo);
    let s = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![a, b, c] },
    )
    .unwrap();

    // 批量：a 选入、b 剔除；outside 不在快照 → 忽略计数 1
    let dto = fetch_cull_decision_apply(
        &state,
        s.id,
        &[dec(a, Some("accepted")), dec(b, Some("rejected")), dec(outside, Some("accepted"))],
    )
    .unwrap();
    assert_eq!((dto.total, dto.accepted, dto.rejected, dto.undecided), (3, 1, 1, 1));
    assert_eq!(dto.ignored, Some(1), "快照外 id 忽略并计数返回");

    // 翻转 + 覆盖：b 改选入、a 改剔除 → 计数随之
    let dto = fetch_cull_decision_apply(
        &state,
        s.id,
        &[dec(b, Some("accepted")), dec(a, Some("rejected"))],
    )
    .unwrap();
    assert_eq!((dto.accepted, dto.rejected, dto.undecided), (1, 1, 1));

    // 回未定（null 删行）+ 同值重写幂等
    let dto = fetch_cull_decision_apply(&state, s.id, &[dec(a, None), dec(b, None), dec(c, None)])
        .unwrap();
    assert_eq!((dto.accepted, dto.rejected, dto.undecided), (0, 0, 3));

    // open：条目快照序 + 决定/origin（未定 null）
    fetch_cull_decision_apply(
        &state,
        s.id,
        &[dec(a, Some("accepted")), dec(c, Some("rejected"))],
    )
    .unwrap();
    let detail = fetch_cull_session_open(&state, s.id).unwrap();
    assert_eq!(detail.session.total, 3);
    let items: Vec<(i64, Option<&str>, Option<&str>)> = detail
        .items
        .iter()
        .map(|i| (i.asset_id, i.decision.as_deref(), i.origin.as_deref()))
        .collect();
    assert_eq!(
        items,
        vec![
            (a, Some("accepted"), Some("manual")),
            (b, None, None),
            (c, Some("rejected"), Some("manual")),
        ],
        "items 快照序、不含资产字段、未定 = null/null"
    );

    // 非法值拒绝（decision / origin）
    assert!(fetch_cull_decision_apply(
        &state,
        s.id,
        &[CullDecisionInput {
            asset_id: a,
            decision: Some("maybe".into()),
            origin: None,
        }],
    )
    .is_err());
    assert!(fetch_cull_decision_apply(
        &state,
        s.id,
        &[CullDecisionInput {
            asset_id: a,
            decision: Some("accepted".into()),
            origin: Some("robot".into()),
        }],
    )
    .is_err());
    // ai origin 合法（V3 预标记通道，v1 允许写入）
    assert!(fetch_cull_decision_apply(
        &state,
        s.id,
        &[CullDecisionInput {
            asset_id: b,
            decision: Some("accepted".into()),
            origin: Some("ai".into()),
        }],
    )
    .is_ok());

    // 不存在的会话
    assert!(fetch_cull_decision_apply(&state, 9999, &[]).is_err());
}

#[test]
fn list_orders_running_first_then_updated_desc() {
    let (_dir, state, db) = setup();
    let album = db.album_create("A").unwrap();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    db.album_add_assets(album.id, &[a], None).unwrap();

    let s1 = fetch_cull_session_create(&state, CullScope::Album { album_id: album.id, subgroup: None }).unwrap();
    std::thread::sleep(Duration::from_millis(5));
    let s2 = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();
    // s1 有决定 → updated_at 最新但仍进行中；s2 先收尾
    fetch_cull_decision_apply(&state, s1.id, &[dec(a, Some("accepted"))]).unwrap();
    fetch_cull_session_finish(
        &state,
        s2.id,
        CullFinishApply { accepted_flag: false, accepted_rating: None, reject_rejected: false },
    )
    .unwrap();

    let list = fetch_cull_session_list(&state).unwrap();
    let ids: Vec<i64> = list.iter().map(|d| d.id).collect();
    assert_eq!(ids, vec![s1.id, s2.id], "进行中在前（即便 finished 的 updated_at 更晚）");
    assert!(list[0].finished_at.is_none());
    assert!(list[1].finished_at.is_some());
    // list DTO 计数也是派生的
    assert_eq!((list[0].accepted, list[0].undecided), (1, 0));
}

// ---------------------------------------------------------------------------
// 收尾映射 + 幂等 + XMP 投影
// ---------------------------------------------------------------------------

#[test]
fn finish_maps_flag_rating_reject_with_xmp_projection_and_is_idempotent() {
    let (dir, state, db) = setup();
    // 真实文件（XMP 边车写盘断言）
    let photo_ok = dir.path().join("photos/keep.jpg");
    let photo_bad = dir.path().join("photos/drop.jpg");
    std::fs::write(&photo_ok, b"jpeg").unwrap();
    std::fs::write(&photo_bad, b"jpeg").unwrap();
    let a = ins(&db, &photo_ok.to_string_lossy(), None, AssetKind::Photo);
    let b = ins(&db, &photo_bad.to_string_lossy(), None, AssetKind::Photo);
    let c = ins(&db, "X:/p/c.jpg", None, AssetKind::Photo); // 未定
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a, b, c] })
        .unwrap();
    fetch_cull_decision_apply(
        &state,
        s.id,
        &[dec(a, Some("accepted")), dec(b, Some("rejected"))],
    )
    .unwrap();

    // 三开关全开
    let result = fetch_cull_session_finish(
        &state,
        s.id,
        CullFinishApply {
            accepted_flag: true,
            accepted_rating: Some(4),
            reject_rejected: true,
        },
    )
    .unwrap();
    assert_eq!((result.applied_flag, result.applied_rating, result.rejected), (1, Some(1), 1));

    // DB 映射：accepted → flag+rating；rejected → 拒绝态；未定不动
    let (flag, rating, rejected): (i64, i64, i64) = db
        .0
        .query_row("SELECT flagged, rating, rejected FROM assets WHERE id = ?1", [a], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((flag, rating, rejected), (1, 4, 0), "已选 → 旗标 + 星级");
    let (flag, rating, rejected): (i64, i64, i64) = db
        .0
        .query_row("SELECT flagged, rating, rejected FROM assets WHERE id = ?1", [b], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((flag, rating, rejected), (0, 0, 1), "已剔除 → 拒绝态（星级 DB 保留 0）");
    let (flag, rating, rejected): (i64, i64, i64) = db
        .0
        .query_row("SELECT flagged, rating, rejected FROM assets WHERE id = ?1", [c], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((flag, rating, rejected), (0, 0, 0), "未定不映射");

    // XMP 即时投影（supervisor 异步 → 轮询）：已选 → xmp:Rating=4；剔除 → -1
    let ok_side = xmp::sidecar_path(&photo_ok);
    let bad_side = xmp::sidecar_path(&photo_bad);
    assert!(
        wait_until(Duration::from_secs(10), || {
            ok_side.is_file()
                && std::fs::read_to_string(&ok_side).unwrap().contains(r#"xmp:Rating="4""#)
        }),
        "已选星级应投影边车 xmp:Rating=4"
    );
    assert!(
        wait_until(Duration::from_secs(10), || {
            bad_side.is_file()
                && std::fs::read_to_string(&bad_side).unwrap().contains(r#"xmp:Rating="-1""#)
        }),
        "已剔除应投影边车 xmp:Rating=-1"
    );

    // finished_at 落库 + 幂等：重复收尾/决定/改名全拒
    let fresh = fetch_cull_session_open(&state, s.id).unwrap();
    assert!(fresh.session.finished_at.is_some());
    assert!(fetch_cull_session_finish(
        &state,
        s.id,
        CullFinishApply { accepted_flag: true, accepted_rating: None, reject_rejected: true },
    )
    .is_err(), "已收尾拒绝重复映射");
    assert!(fetch_cull_decision_apply(&state, s.id, &[dec(c, Some("accepted"))]).is_err());
    assert!(fetch_cull_session_rename(&state, s.id, "改名").is_err());

    // 星级越界拒绝（未收尾会话上先验）
    let s2 = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();
    assert!(fetch_cull_session_finish(
        &state,
        s2.id,
        CullFinishApply { accepted_flag: false, accepted_rating: Some(9), reject_rejected: false },
    )
    .is_err());
    // 全关开关 = 纯收尾（0 映射）
    let r2 = fetch_cull_session_finish(
        &state,
        s2.id,
        CullFinishApply { accepted_flag: false, accepted_rating: None, reject_rejected: false },
    )
    .unwrap();
    assert_eq!((r2.applied_flag, r2.applied_rating, r2.rejected), (0, None, 0));
}

// ---------------------------------------------------------------------------
// 改名 / 弃置 / 级联
// ---------------------------------------------------------------------------

#[test]
fn rename_discard_and_missing_session_errors() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();

    let renamed = fetch_cull_session_rename(&state, s.id, "  我的选片  ").unwrap();
    assert_eq!(renamed.name, "我的选片", "trim 归一");
    assert!(fetch_cull_session_rename(&state, s.id, "   ").is_err());
    assert!(fetch_cull_session_rename(&state, 9999, "x").is_err());
    assert!(fetch_cull_session_open(&state, 9999).is_err());

    // 弃置：会话+快照+决定全清，主库标记不动
    fetch_cull_decision_apply(&state, s.id, &[dec(a, Some("accepted"))]).unwrap();
    fetch_cull_session_discard(&state, s.id).unwrap();
    assert!(fetch_cull_session_open(&state, s.id).is_err());
    let n: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM cull_session_asset", [], |r| r.get(0))
        .unwrap();
    let d: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM cull_decision", [], |r| r.get(0))
        .unwrap();
    assert_eq!((n, d), (0, 0), "弃置级联清快照与决定");
    assert!(fetch_cull_session_discard(&state, s.id).is_err(), "再弃置报不存在");
}

#[test]
fn asset_delete_cascades_snapshot_and_decisions() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a, b] })
        .unwrap();
    fetch_cull_decision_apply(
        &state,
        s.id,
        &[dec(a, Some("accepted")), dec(b, Some("rejected"))],
    )
    .unwrap();

    // 物理删 b（assets_delete_rows = 永久删除路径）
    let deleted = db.assets_delete_rows(&[b]).unwrap();
    assert_eq!(deleted, 1);
    let detail = fetch_cull_session_open(&state, s.id).unwrap();
    assert_eq!(detail.session.total, 1, "快照行随资产级联消失");
    assert_eq!((detail.session.accepted, detail.session.rejected, detail.session.undecided), (1, 0, 0));
    assert_eq!(detail.items.len(), 1);
    assert_eq!(detail.items[0].asset_id, a);

    // 全删 → total 0，会话仍在（历史可查）
    db.assets_delete_rows(&[a]).unwrap();
    let detail = fetch_cull_session_open(&state, s.id).unwrap();
    assert_eq!(detail.session.total, 0);
    assert!(fetch_cull_session_list(&state).unwrap().iter().any(|d| d.id == s.id));
}

#[test]
fn scope_json_roundtrips_through_dto() {
    let (_dir, state, db) = setup();
    let album = db.album_create("画册").unwrap();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    db.album_add_assets(album.id, &[a], Some("成片")).unwrap();

    let s = fetch_cull_session_create(
        &state,
        CullScope::Album { album_id: album.id, subgroup: Some("成片".into()) },
    )
    .unwrap();
    assert_eq!(
        s.scope,
        CullScope::Album { album_id: album.id, subgroup: Some("成片".into()) }
    );

    let q = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();
    assert_eq!(q.scope, CullScope::Query { asset_ids: vec![a] }, "query 的 assetIds 来源于记录");

    // DTO 序列化形状（camelCase + kind 判别 + 计数字段）
    let json = serde_json::to_value(&q).unwrap();
    assert_eq!(json["scope"]["kind"], "query");
    assert_eq!(json["scope"]["assetIds"], serde_json::json!([a]));
    assert_eq!(json["undecided"], 1);
    assert!(json.get("ignored").is_none(), "非 apply 路径不占 ignored 载荷");
}
