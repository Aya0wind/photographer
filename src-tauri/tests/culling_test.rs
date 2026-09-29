//! 选片会话 V1（0024）：快照语义（album 含子组过滤 / query 传入序）、决定
//! 批量 upsert / 回未定 / 快照外忽略计数、进度纯派生、默认名轮次（初选/
//! 复选）与同名后缀、列表序（进行中在前）、收尾三开关映射 + 幂等 + XMP
//! 即时投影（星级/拒绝断言边车）、改名/弃置、资产删除级联收敛。
//! V2/V3：open items 连拍组字段（burstId/burstSize 快照口径聚合）+
//! AI 挑图预扫规则引擎（三档敏感度值集 / 合影豁免边界 / 连拍留最锐 /
//! maxAccepted 封顶 / manual 不动 / apply 落 origin='ai' / 非法规则）。

mod common;

use common::library_fixture as setup;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use db::culling::CullScope;
use db::AssetRow;
use events::AssetKind;
use ipc::culling::{
    fetch_cull_ai_prescan, fetch_cull_decision_apply, fetch_cull_session_create,
    fetch_cull_session_discard, fetch_cull_session_finish, fetch_cull_session_list,
    fetch_cull_session_open, fetch_cull_session_rename, CullDecisionInput, CullFinishApply,
    CullPrescanRulesInput, CullRuleSwitch,
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

// ---------------------------------------------------------------------------
// V2：open items 连拍组字段
// ---------------------------------------------------------------------------

/// 建一个连拍组（直接落 bursts + 回填 assets.burst_id），返回组 id。
fn burst_of(db: &db::Db, ids: &[i64]) -> i64 {
    db.0.execute(
        "INSERT INTO bursts (asset_count, started_at, ended_at) VALUES (?1, NULL, NULL)",
        [ids.len() as i64],
    )
    .unwrap();
    let bid = db.0.last_insert_rowid();
    for id in ids {
        db.0.execute("UPDATE assets SET burst_id = ?1 WHERE id = ?2", [bid, *id]).unwrap();
    }
    bid
}

#[test]
fn open_items_carry_burst_id_and_snapshot_burst_size() {
    let (_dir, state, db) = setup();
    // 组 G：3 张成组；快照 A 只含 x1/x3（组员部分在册）；快照 B 全含
    let x1 = ins(&db, "X:/p/x1.jpg", None, AssetKind::Photo);
    let x2 = ins(&db, "X:/p/x2.jpg", None, AssetKind::Photo);
    let x3 = ins(&db, "X:/p/x3.jpg", None, AssetKind::Photo);
    let solo = ins(&db, "X:/p/solo.jpg", None, AssetKind::Photo);
    let g = burst_of(&db, &[x1, x2, x3]);

    // 快照 A：x1, x3, solo（x2 不在册 → 组快照内成员数 = 2）
    let sa = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![x1, x3, solo] },
    )
    .unwrap();
    let items: Vec<(i64, Option<i64>, u32)> = fetch_cull_session_open(&state, sa.id)
        .unwrap()
        .items
        .into_iter()
        .map(|i| (i.asset_id, i.burst_id, i.burst_size))
        .collect();
    assert_eq!(
        items,
        vec![(x1, Some(g), 2), (x3, Some(g), 2), (solo, None, 1)],
        "burstSize 按会话快照口径聚合（部分在册 = 在册数），无组 = 1"
    );

    // 快照 B：全组在册 → 3
    let sb = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![x1, x2, x3] },
    )
    .unwrap();
    let sizes: Vec<u32> = fetch_cull_session_open(&state, sb.id)
        .unwrap()
        .items
        .iter()
        .map(|i| i.burst_size)
        .collect();
    assert_eq!(sizes, vec![3, 3, 3]);

    // camelCase 契约
    let detail = fetch_cull_session_open(&state, sa.id).unwrap();
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["items"][0]["burstId"], g);
    assert_eq!(json["items"][0]["burstSize"], 2);
    assert_eq!(json["items"][2]["burstId"], serde_json::Value::Null);
    assert_eq!(json["items"][2]["burstSize"], 1);
}

// ---------------------------------------------------------------------------
// V3：AI 挑图预扫规则引擎
// ---------------------------------------------------------------------------

/// 落一条 eyes 分析行。
fn eyes(db: &db::Db, id: i64, value: &str, score: f64) {
    db.set_ai_analysis(id, "eyes", Some(value), Some(score), "t").unwrap();
}

/// 落一条 blur 分析行。
fn blur(db: &db::Db, id: i64, value: &str, score: f64) {
    db.set_ai_analysis(id, "blur", Some(value), Some(score), "t").unwrap();
}

/// 给资产插 n 行 faces（合影豁免的面数口径 = faces 行数）。
fn faces_n(db: &db::Db, id: i64, n: u32) {
    for _ in 0..n {
        db.insert_face(id, 0.1, 0.1, 0.2, 0.2, &[0.0f32; 4], None).unwrap();
    }
}

/// 规则构造：eyes/blur = Some(敏感度) 即 enabled。
fn rules(
    eyes_sens: Option<&str>,
    blur_sens: Option<&str>,
    burst_keep_sharpest: bool,
    group_exempt_faces: u32,
    max_accepted: Option<i64>,
) -> CullPrescanRulesInput {
    CullPrescanRulesInput {
        eyes: CullRuleSwitch {
            enabled: eyes_sens.is_some(),
            sensitivity: eyes_sens.unwrap_or("normal").to_string(),
        },
        blur: CullRuleSwitch {
            enabled: blur_sens.is_some(),
            sensitivity: blur_sens.unwrap_or("normal").to_string(),
        },
        burst_keep_sharpest,
        group_exempt_faces,
        max_accepted,
    }
}

fn decision_rows(db: &db::Db) -> Vec<(i64, String, String)> {
    let mut stmt = db
        .0
        .prepare("SELECT asset_id, decision, origin FROM cull_decision ORDER BY asset_id")
        .unwrap();
    let rows = stmt
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    rows.collect::<Result<Vec<_>, _>>().unwrap()
}

/// 表驱动：eyes 三档敏感度值集（weak={closed}+score≥0.5 / normal={closed}
/// / strong={closed,maybe}；unknown 与无记录永不命中）。
#[test]
fn prescan_eyes_sensitivity_value_sets() {
    let (_dir, state, db) = setup();
    // (value, score, weak 命中, normal 命中, strong 命中)
    struct EyeCase(&'static str, f64, bool, bool, bool);
    let cases = [
        EyeCase("closed", 0.93, true, true, true),
        EyeCase("closed", 0.30, false, true, true), // weak 额外要求 score ≥ 0.5
        EyeCase("closed", 0.50, true, true, true),  // 边界：= 0.5 命中
        EyeCase("maybe", 0.70, false, false, true), // maybe 只进 strong 值集
        EyeCase("unknown", 0.0, false, false, false),
    ];
    let ids: Vec<i64> = cases
        .iter()
        .map(|c| {
            let id = ins(&db, &format!("X:/p/e{}{}.jpg", c.0, c.1), None, AssetKind::Photo);
            eyes(&db, id, c.0, c.1);
            id
        })
        .collect();
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: ids.clone() })
        .unwrap();

    for (sens, pick) in [("weak", 2usize), ("normal", 3usize), ("strong", 4usize)] {
        let expected: Vec<i64> = ids
            .iter()
            .enumerate()
            .filter(|&(i, _)| match pick { 2 => cases[i].2, 3 => cases[i].3, _ => cases[i].4 })
            .map(|(_, &id)| id)
            .collect();
        let dto = fetch_cull_ai_prescan(&state, s.id, rules(Some(sens), None, false, 0, None), false)
            .unwrap();
        assert_eq!(
            dto.suggested_rejected.asset_ids, expected,
            "eyes {sens} 档值集（快照序）"
        );
        assert_eq!(dto.suggested_accepted.asset_ids, Vec::<i64>::new());
        assert_eq!(dto.suggested_accepted.count, 0);
        assert_eq!(dto.applied, None, "apply=false 不携带 applied");
        assert_eq!(dto.rules_echo.eyes.sensitivity, sens, "规则回显");
    }
    // 纯预览不落任何决定行
    assert!(decision_rows(&db).is_empty(), "apply=false 零写入");
}

/// 表驱动：blur 三档值集（weak={} 关 / normal=strong={soft}；sharp/
/// unknown/无记录永不命中）。
#[test]
fn prescan_blur_sensitivity_value_sets() {
    let (_dir, state, db) = setup();
    let soft = ins(&db, "X:/p/soft.jpg", None, AssetKind::Photo);
    let sharp = ins(&db, "X:/p/sharp.jpg", None, AssetKind::Photo);
    let unk = ins(&db, "X:/p/unk.jpg", None, AssetKind::Photo);
    let bare = ins(&db, "X:/p/bb.jpg", None, AssetKind::Photo);
    blur(&db, soft, "soft", 12.0);
    blur(&db, sharp, "sharp", 80.0);
    blur(&db, unk, "unknown", 0.0);
    let s = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![soft, sharp, unk, bare] },
    )
    .unwrap();

    for (sens, expected) in [
        ("weak", Vec::<i64>::new()), // weak 值集为空 = 不启用
        ("normal", vec![soft]),
        ("strong", vec![soft]),
    ] {
        let dto = fetch_cull_ai_prescan(&state, s.id, rules(None, Some(sens), false, 0, None), false)
            .unwrap();
        assert_eq!(dto.suggested_rejected.asset_ids, expected, "blur {sens} 档值集");
        assert_eq!(dto.suggested_rejected.count, expected.len() as u64);
        assert_eq!(dto.suggested_accepted.asset_ids, Vec::<i64>::new());
    }
}

/// 合影豁免边界：faces = N 不豁免、> N 豁免（N=0 整体关）。
#[test]
fn prescan_group_exempt_boundary() {
    let (_dir, state, db) = setup();
    let e2 = ins(&db, "X:/p/e2.jpg", None, AssetKind::Photo); // faces = 2
    let e3 = ins(&db, "X:/p/e3.jpg", None, AssetKind::Photo); // faces = 3
    let e5 = ins(&db, "X:/p/e5.jpg", None, AssetKind::Photo); // faces = 5
    for (id, n) in [(e2, 2u32), (e3, 3), (e5, 5)] {
        eyes(&db, id, "closed", 0.9);
        faces_n(&db, id, n);
    }
    let s = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![e2, e3, e5] },
    )
    .unwrap();

    // N=2：=2 不豁免（照判闭眼）、>2 豁免（跳过 eyes）
    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), None, false, 2, None),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_rejected.asset_ids, vec![e2], "= N 边界不豁免");
    assert_eq!(dto.exempted_group.asset_ids, vec![e3, e5], "> N 豁免（只跳 eyes）");
    assert_eq!(dto.exempted_group.count, 2);

    // N=0：豁免整体关（哪怕 5 张脸也照判）
    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), None, false, 0, None),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_rejected.asset_ids, vec![e2, e3, e5], "0 = 豁免关");
    assert_eq!(dto.exempted_group.asset_ids, Vec::<i64>::new());

    // 豁免只旁路 eyes：blur 规则照判豁免项（四桶非互斥）
    blur(&db, e3, "soft", 10.0);
    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), Some("normal"), false, 2, None),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_rejected.asset_ids, vec![e2, e3], "blur 不受合影豁免");
    assert_eq!(dto.exempted_group.asset_ids, vec![e3, e5]);
}

/// 连拍留最锐：组内 blur score 最高 → accepted、其余组员 → rejected；
/// 无 blur 分的组不动；冲突保守取剔除；已决定的最锐不晋升次锐。
#[test]
fn prescan_burst_keep_sharpest() {
    let (_dir, state, db) = setup();
    // G1：三分组（50/90/70）→ 90 留、其余剔
    let g1a = ins(&db, "X:/p/g1a.jpg", None, AssetKind::Photo);
    let g1b = ins(&db, "X:/p/g1b.jpg", None, AssetKind::Photo);
    let g1c = ins(&db, "X:/p/g1c.jpg", None, AssetKind::Photo);
    burst_of(&db, &[g1a, g1b, g1c]);
    blur(&db, g1a, "sharp", 50.0);
    blur(&db, g1b, "sharp", 90.0);
    blur(&db, g1c, "sharp", 70.0);
    // G2：全组无 blur 分 → 不动
    let g2a = ins(&db, "X:/p/g2a.jpg", None, AssetKind::Photo);
    let g2b = ins(&db, "X:/p/g2b.jpg", None, AssetKind::Photo);
    burst_of(&db, &[g2a, g2b]);
    // 孤儿组员（组另一成员不在快照 → burstSize=1 → 不算组）
    let l1 = ins(&db, "X:/p/l1.jpg", None, AssetKind::Photo);
    let l2 = ins(&db, "X:/p/l2_out.jpg", None, AssetKind::Photo); // 不入快照
    burst_of(&db, &[l1, l2]);
    blur(&db, l1, "sharp", 60.0);
    // G3：最锐那张闭眼（normal）→ 冲突保守取剔除（不 accepted）
    let g3a = ins(&db, "X:/p/g3a.jpg", None, AssetKind::Photo);
    let g3b = ins(&db, "X:/p/g3b.jpg", None, AssetKind::Photo);
    burst_of(&db, &[g3a, g3b]);
    blur(&db, g3a, "sharp", 95.0);
    blur(&db, g3b, "sharp", 60.0);
    eyes(&db, g3a, "closed", 0.9);
    // G4：最锐已被 manual 剔除 → 不给次锐 accepted、其余组员照剔
    let g4a = ins(&db, "X:/p/g4a.jpg", None, AssetKind::Photo);
    let g4b = ins(&db, "X:/p/g4b.jpg", None, AssetKind::Photo);
    burst_of(&db, &[g4a, g4b]);
    blur(&db, g4a, "sharp", 90.0);
    blur(&db, g4b, "sharp", 70.0);
    // G5：部分有分（有分者留、无分组员照剔）
    let g5a = ins(&db, "X:/p/g5a.jpg", None, AssetKind::Photo);
    let g5b = ins(&db, "X:/p/g5b.jpg", None, AssetKind::Photo);
    burst_of(&db, &[g5a, g5b]);
    blur(&db, g5a, "sharp", 80.0);

    let s = fetch_cull_session_create(
        &state,
        CullScope::Query {
            asset_ids: vec![g1a, g1b, g1c, g2a, g2b, l1, g3a, g3b, g4a, g4b, g5a, g5b],
        },
    )
    .unwrap();
    fetch_cull_decision_apply(&state, s.id, &[dec(g4a, Some("rejected"))]).unwrap();

    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), None, true, 0, None),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_accepted.asset_ids, vec![g1b, g5a], "组内最锐（含部分有分组）");
    assert_eq!(
        dto.suggested_rejected.asset_ids,
        vec![g1a, g1c, g3a, g3b, g4b, g5b],
        "其余组员剔（含无分者）；孤儿/无分组/已决定不动"
    );
    assert_eq!(dto.skipped_manual.asset_ids, vec![g4a], "manual 决定跳过");
    assert_eq!(dto.exempted_group.asset_ids, Vec::<i64>::new());
    // G3 冲突：最锐 g3a 剔除建议覆盖 accepted（保守）→ 不在 accepted 桶
    assert!(!dto.suggested_accepted.asset_ids.contains(&g3a));
}

/// maxAccepted 封顶：accepted 建议按快照序先到先得，封顶后转不动；
/// rejected 不受限。
#[test]
fn prescan_max_accepted_caps_in_snapshot_order() {
    let (_dir, state, db) = setup();
    let a1 = ins(&db, "X:/p/a1.jpg", None, AssetKind::Photo);
    let a2 = ins(&db, "X:/p/a2.jpg", None, AssetKind::Photo);
    let b1 = ins(&db, "X:/p/b1.jpg", None, AssetKind::Photo);
    let b2 = ins(&db, "X:/p/b2.jpg", None, AssetKind::Photo);
    burst_of(&db, &[a1, a2]);
    burst_of(&db, &[b1, b2]);
    blur(&db, a1, "sharp", 80.0);
    blur(&db, a2, "sharp", 60.0);
    blur(&db, b1, "sharp", 90.0);
    blur(&db, b2, "sharp", 70.0);
    let s = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![a1, a2, b1, b2] },
    )
    .unwrap();

    // 封顶 1：a1（快照序先）留；b1 的 accepted 转不动（不在任何桶）
    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(None, None, true, 0, Some(1)),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_accepted.asset_ids, vec![a1], "先到先得");
    assert_eq!(dto.suggested_accepted.count, 1);
    assert_eq!(dto.suggested_rejected.asset_ids, vec![a2, b2], "rejected 不受限");
    for bucket in [&dto.skipped_manual, &dto.exempted_group] {
        assert_eq!(bucket.asset_ids, Vec::<i64>::new());
    }
    // b1 转不动 = 不出现在任何桶
    let all: Vec<i64> = dto
        .suggested_accepted
        .asset_ids
        .iter()
        .chain(dto.suggested_rejected.asset_ids.iter())
        .chain(dto.skipped_manual.asset_ids.iter())
        .chain(dto.exempted_group.asset_ids.iter())
        .copied()
        .collect();
    assert!(!all.contains(&b1), "封顶后的 accepted 转不动（不入桶）");

    // 封顶 0：全部 accepted 转不动
    let dto = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(None, None, true, 0, Some(0)),
        false,
    )
    .unwrap();
    assert_eq!(dto.suggested_accepted.asset_ids, Vec::<i64>::new());
    assert_eq!(dto.suggested_rejected.asset_ids, vec![a2, b2]);

    // 不封顶：两个 accepted 都在
    let dto = fetch_cull_ai_prescan(&state, s.id, rules(None, None, true, 0, None), false).unwrap();
    assert_eq!(dto.suggested_accepted.asset_ids, vec![a1, b1]);
}

/// apply 语义：预览零写入；apply 只写未定项、origin='ai'、既有决定
/// （manual 与 ai 皆）与未建议项不动。
#[test]
fn prescan_apply_writes_ai_origin_and_leaves_decided_untouched() {
    let (_dir, state, db) = setup();
    let m1 = ins(&db, "X:/p/m1.jpg", None, AssetKind::Photo);
    let m2 = ins(&db, "X:/p/m2.jpg", None, AssetKind::Photo);
    let m3 = ins(&db, "X:/p/m3.jpg", None, AssetKind::Photo);
    let u1 = ins(&db, "X:/p/u1.jpg", None, AssetKind::Photo); // 闭眼 → 剔
    let u3 = ins(&db, "X:/p/u3.jpg", None, AssetKind::Photo); // 最锐 → 选
    let u4 = ins(&db, "X:/p/u4.jpg", None, AssetKind::Photo); // 次锐 → 剔
    let u5 = ins(&db, "X:/p/u5.jpg", None, AssetKind::Photo); // 干净未定 → 不动
    eyes(&db, m1, "closed", 0.9); // 已决定 → 也不该被建议
    eyes(&db, u1, "closed", 0.9);
    burst_of(&db, &[u3, u4]);
    blur(&db, u3, "sharp", 90.0);
    blur(&db, u4, "sharp", 60.0);
    let s = fetch_cull_session_create(
        &state,
        CullScope::Query { asset_ids: vec![m1, m2, m3, u1, u3, u4, u5] },
    )
    .unwrap();
    fetch_cull_decision_apply(&state, s.id, &[dec(m1, Some("accepted"))]).unwrap();
    fetch_cull_decision_apply(&state, s.id, &[dec(m2, Some("rejected"))]).unwrap();
    fetch_cull_decision_apply(
        &state,
        s.id,
        &[CullDecisionInput {
            asset_id: m3,
            decision: Some("accepted".into()),
            origin: Some("ai".into()), // 既有 ai 决定同样不动
        }],
    )
    .unwrap();

    // 预览：零写入
    let preview = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), None, true, 0, None),
        false,
    )
    .unwrap();
    assert_eq!(preview.suggested_accepted.asset_ids, vec![u3]);
    assert_eq!(preview.suggested_rejected.asset_ids, vec![u1, u4]);
    assert_eq!(preview.skipped_manual.asset_ids, vec![m1, m2, m3], "manual 与既有 ai 都跳过");
    assert_eq!(preview.applied, None);
    assert_eq!(decision_rows(&db).len(), 3, "apply=false 零写入");

    // 落地：origin='ai'；未建议的 u5 不动
    let applied = fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), None, true, 0, None),
        true,
    )
    .unwrap();
    assert_eq!(applied.applied, Some(3), "写入决定行数 = 建议数");
    assert_eq!(applied.suggested_accepted.asset_ids, vec![u3], "同形返回");
    assert_eq!(applied.suggested_rejected.asset_ids, vec![u1, u4]);

    let detail = fetch_cull_session_open(&state, s.id).unwrap();
    let by_id = |id: i64| {
        detail
            .items
            .iter()
            .find(|i| i.asset_id == id)
            .map(|i| (i.decision.as_deref(), i.origin.as_deref()))
            .unwrap()
    };
    assert_eq!(by_id(m1), (Some("accepted"), Some("manual")), "manual 不被覆盖");
    assert_eq!(by_id(m2), (Some("rejected"), Some("manual")));
    assert_eq!(by_id(m3), (Some("accepted"), Some("ai")), "既有 ai 不被覆盖");
    assert_eq!(by_id(u1), (Some("rejected"), Some("ai")), "AI 建议落 origin=ai");
    assert_eq!(by_id(u3), (Some("accepted"), Some("ai")));
    assert_eq!(by_id(u4), (Some("rejected"), Some("ai")));
    assert_eq!(by_id(u5), (None, None), "未建议项保持未定");
    assert_eq!(
        (detail.session.accepted, detail.session.rejected, detail.session.undecided),
        (3, 3, 1)
    );
}

/// 非法规则与终态会话：敏感度/负数上限报错；不存在/已收尾拒绝。
#[test]
fn prescan_invalid_rules_and_session_state() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();

    assert!(fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("ultra"), None, false, 0, None),
        false
    )
    .is_err(), "eyes 敏感度非法");
    assert!(fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(None, Some(""), false, 0, None),
        false
    )
    .is_err(), "blur 敏感度非法");
    assert!(
        fetch_cull_ai_prescan(&state, s.id, rules(None, None, false, 0, Some(-1)), false).is_err(),
        "maxAccepted 负数非法"
    );
    assert!(fetch_cull_ai_prescan(&state, 9999, rules(None, None, false, 0, None), false).is_err());

    // 已收尾拒绝（预览与 apply 同拒）
    fetch_cull_session_finish(
        &state,
        s.id,
        CullFinishApply { accepted_flag: false, accepted_rating: None, reject_rejected: false },
    )
    .unwrap();
    assert!(fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), Some("normal"), false, 0, None),
        false
    )
    .is_err());
    assert!(fetch_cull_ai_prescan(
        &state,
        s.id,
        rules(Some("normal"), Some("normal"), false, 0, None),
        true
    )
    .is_err());
}

/// DTO 契约：camelCase 键名 + 桶 {count, assetIds} + rulesEcho + applied
/// 仅 apply 携带。
#[test]
fn prescan_dto_serialization_shape() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    eyes(&db, a, "closed", 0.9);
    let s = fetch_cull_session_create(&state, CullScope::Query { asset_ids: vec![a] }).unwrap();
    let input = rules(Some("normal"), Some("weak"), false, 0, None);
    let dto = fetch_cull_ai_prescan(&state, s.id, input.clone(), false).unwrap();
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["suggestedRejected"]["count"], 1);
    assert_eq!(json["suggestedRejected"]["assetIds"], serde_json::json!([a]));
    assert_eq!(json["rulesEcho"]["eyes"]["sensitivity"], "normal");
    assert_eq!(json["rulesEcho"]["blur"]["sensitivity"], "weak");
    assert_eq!(json["rulesEcho"]["burstKeepSharpest"], false);
    assert_eq!(json["rulesEcho"]["groupExemptFaces"], 0);
    assert_eq!(json["rulesEcho"]["maxAccepted"], serde_json::Value::Null);
    assert!(json.get("applied").is_none(), "apply=false 不占 applied 载荷");
}
