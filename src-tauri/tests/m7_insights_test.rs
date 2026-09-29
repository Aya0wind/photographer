//! M7：F6 那年今天 + F9 器材统计（桶边界/解析守卫/空态 null）+ F8 两级去重
//! （exact 分组、similar 多探针近重复分组、孪生排除、删除级联、分页）。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::AssetRow;
use events::AssetKind;

fn asset(path: &str, captured_at: Option<&str>) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-22T00:00:00.000Z".into(),
        xxhash: 1,
        kind: AssetKind::Photo,
        captured_at: captured_at.map(Into::into),
        camera: Some("Sony A7M4".into()),
        source: "imported".into(),
        created_at: "2026-09-22T00:00:00.000Z".into(),
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
    }
}

// ---------------------------------------------------------------------------
// F6 那年今天
// ---------------------------------------------------------------------------

#[test]
fn on_this_day_selects_local_month_day_across_years() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    let today = chrono::Local::now().format("%m-%d").to_string();
    let md = |y: &str| format!("{y}-{today}T10:00:00.000Z");
    // 三年同月日 + 一个他日 + 一个无时间
    db.insert_asset(&asset("X:/p/y2024.jpg", Some(&md("2024"))))
        .unwrap();
    db.insert_asset(&asset("X:/p/y2025a.jpg", Some(&md("2025"))))
        .unwrap();
    db.insert_asset(&asset("X:/p/y2025b.jpg", Some(&md("2025"))))
        .unwrap();
    db.insert_asset(&asset("X:/p/other.jpg", Some("2020-01-01T10:00:00.000Z")))
        .unwrap();
    db.insert_asset(&asset("X:/p/notime.jpg", None)).unwrap();

    let list = ipc::insights::fetch_on_this_day(&state).unwrap();
    let names: Vec<&str> = list.iter().map(|a| a.name.as_str()).collect();
    // 年份 DESC；同年内 captured ASC（构造串同序）
    assert_eq!(names.len(), 3, "只今天同月日命中（本地时区）");
    assert!(names.contains(&"y2024.jpg"));
    assert!(names.contains(&"y2025a.jpg"));
    assert!(!names.contains(&"other.jpg"));
    // 年份 DESC：2025 组在前
    let pos = |n: &str| names.iter().position(|x| *x == n).unwrap();
    assert!(pos("y2025a.jpg") < pos("y2024.jpg"), "年份降序");

    // 今天无历史 → []（清库重验）
    db.0.execute("DELETE FROM assets", []).unwrap();
    assert!(ipc::insights::fetch_on_this_day(&state).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// 侧栏一次性计数（2026-09-21）
// ---------------------------------------------------------------------------

#[test]
fn sidebar_counts_empty_library() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let counts = ipc::insights::fetch_sidebar_counts(&state).unwrap();
    assert_eq!(counts.assets, 0);
    assert_eq!(counts.recent_viewed, 0);
    assert_eq!(counts.on_this_day, 0);
    // 标签墙 = 预置词表全量；相册 = 真实 COUNT(album)（0015 起为 DB 实体）
    assert_eq!(counts.tags, ipc::insights::SMART_ALBUM_TAG_COUNT);
    assert_eq!(counts.albums, 0);
}

/// 当前本地年（闰日 2/29 场景 md 内部自回落）。
fn today_year() -> i32 {
    use chrono::Datelike;
    chrono::Local::now().year()
}

#[test]
fn sidebar_counts_semantics_and_tz_boundary() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());

    // 本地时区今天 "%m-%d"（与实现同口径）
    let today = chrono::Local::now().format("%m-%d").to_string();
    let md = |y: i32| {
        // 本地时区今天 12:00 的历年同月日（带本地偏移的 RFC3339——SQLite
        // 先归 UTC 再经 'localtime' 回本地，口径闭环）。闰日 2/29 在非闰
        // 年回落到 2024（闰年，必有 2/29）
        use chrono::{Datelike, TimeZone};
        let now = chrono::Local::now();
        let (m, d) = (now.month(), now.day());
        let year = if chrono::NaiveDate::from_ymd_opt(y, m, d).is_some() {
            y
        } else {
            2024
        };
        chrono::Local
            .with_ymd_and_hms(year, m, d, 12, 0, 0)
            .single()
            .unwrap()
            .to_rfc3339()
    };
    // 3 条历年同月日 + 1 条他日 + 1 条今天但 video（on_this_day 排除、
    // assets 计入）+ 1 条本地今天 00:30 的时区边界样本（其 UTC 日期在
    // 东八区落在前一天——SQL 必须经 'localtime' 才能命中）
    for (path, captured) in [
        ("X:/p/y2020.jpg", Some(md(2020))),
        ("X:/p/y2021.jpg", Some(md(2021))),
        ("X:/p/yThisYear.jpg", Some(md(today_year()))),
        ("X:/p/other.jpg", Some("2020-01-01T10:00:00.000Z".into())),
        ("X:/p/boundary.jpg", {
            let now = chrono::Local::now();
            now.date_naive()
                .and_hms_opt(0, 30, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .single()
                .map(|t| t.to_rfc3339())
        }),
    ] {
        db.insert_asset(&asset(path, captured.as_deref())).unwrap();
    }
    let mut video = asset("X:/p/today.mp4", Some(&md(2020)));
    video.kind = AssetKind::Video;
    db.insert_asset(&video).unwrap();
    // 浏览历史 2 行
    db.mark_asset_viewed(1).unwrap();
    db.mark_asset_viewed(2).unwrap();

    let counts = ipc::insights::fetch_sidebar_counts(&state).unwrap();
    assert_eq!(counts.assets, 5, "旧视频不计入资产总数");
    assert_eq!(counts.recent_viewed, 2);
    // 仅往年两张；今年今天与 video、他日均不计
    assert_eq!(
        counts.on_this_day, 2,
        "本地时区同月日（含 UTC 落前一天的边界样本）: {today}"
    );
    assert_eq!(counts.tags, 40);
    assert_eq!(counts.albums, 0, "真实 COUNT(album)——本测试未建相册");

    // 列表与计数同口径（共用的 WHERE 片段）：列表行数 == 计数
    let list = db.assets_on_this_day(&today).unwrap();
    assert_eq!(list.len() as i64, counts.on_this_day);
}

#[test]
fn sidebar_counts_library_not_open_is_error() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    // 撤掉活动库（模拟引导向导前的侧栏拉取）
    *state.settings.lock().unwrap() = settings::Settings::default();
    let err = ipc::insights::fetch_sidebar_counts(&state).unwrap_err();
    assert!(!err.is_empty(), "库未开必须明确报错而非全 0: {err}");
}

// ---------------------------------------------------------------------------
// F9 器材统计
// ---------------------------------------------------------------------------

#[test]
fn gear_stats_buckets_and_null_state() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());

    // 无任何 EXIF → None（相机也缺——camera 本身就是 EXIF 数据）
    let mut bare = asset("X:/p/none.jpg", None);
    bare.camera = None;
    db.insert_asset(&bare).unwrap();
    assert!(ipc::insights::fetch_gear_stats(&state).unwrap().is_none());

    // 造全维度数据
    let rows = [
        // (focal, iso, aperture, shutter)
        ("16", "100", "1.4", "2"),           // 超广/ISO≤100/≤1.4/慢门>1s
        ("35", "200", "2.8", "1/2"),         // 广角/100-200/1.4-2.8/1-1/2
        ("60", "400", "4", "1/8"),           // 标准/200-400/2.8-4/1/2-1/8
        ("100", "800", "5.6", "1/60"),       // 中长焦/400-800/4-5.6/1/8-1/60
        ("150", "1600", "8", "1/250"),       // 长焦/800-1600/5.6-8/1/60-1/500
        ("300", "3200", "11", "1/1000"),     // 超长焦/1600-3200/>8/≤1/500
        ("300", "6400", "11", "1/1000"),     // >3200
        ("garbage", "6400", "11", "1/1000"), // focal 解析失败不计（ISO 仍计）
    ];
    for (i, (focal, iso, ap, shutter)) in rows.iter().enumerate() {
        let mut row = asset(&format!("X:/p/g{i}.jpg"), None);
        row.focal_length = Some(focal.to_string());
        row.iso = iso.parse().ok();
        row.f_number = Some(ap.to_string());
        row.exposure_time = Some(shutter.to_string());
        db.insert_asset(&row).unwrap();
    }

    let stats = ipc::insights::fetch_gear_stats(&state)
        .unwrap()
        .expect("有 EXIF");
    // 相机聚合（8 行带相机；bare 行 camera=None 不入榜）
    assert_eq!(stats.cameras[0].name, "Sony A7M4");
    assert_eq!(stats.cameras[0].count, 8, "8 行带相机（bare 行无 EXIF）");
    assert!(stats.lenses.is_empty(), "夹具无 lens → 空镜头榜");

    fn labels(b: &[ipc::insights::GearLabelBucketDto]) -> Vec<(String, u64)> {
        b.iter().map(|x| (x.label.clone(), x.count)).collect()
    }
    // 焦段：<24=1, 24-50=1, 50-85=1, 85-135=1, 135-200=1, 200+=2（garbage 不计）
    let focal: Vec<_> = stats
        .focal_buckets
        .iter()
        .map(|b| (b.label.as_str(), b.min, b.max, b.count))
        .collect();
    assert_eq!(
        focal,
        vec![
            ("<24mm", 0, Some(24), 1),
            ("24-50mm", 24, Some(50), 1),
            ("50-85mm", 50, Some(85), 1),
            ("85-135mm", 85, Some(135), 1),
            ("135-200mm", 135, Some(200), 1),
            ("200+mm", 200, None, 2),
        ]
    );
    // ISO：≤100=1, 100-200=1, …, 1600-3200=1, 3200+=2（garbage 行 ISO 6400 计入）
    let iso = labels(&stats.iso_buckets);
    assert_eq!(iso[0], ("≤100".to_string(), 1));
    assert_eq!(iso[6], ("3200+".to_string(), 2));
    // 光圈
    let ap = labels(&stats.aperture_buckets);
    assert_eq!(ap[0], ("≤f/1.4".to_string(), 1));
    assert_eq!(ap[5], (">f/8".to_string(), 3), "g5/g6/g7 三行 f/11");
    // 快门（"2" 秒 → >1s；1/2 → 1-1/2；…；1/1000 → ≤1/500）
    let sh = labels(&stats.shutter_buckets);
    assert_eq!(sh[0], (">1s".to_string(), 1));
    assert_eq!(sh[1], ("1-1/2s".to_string(), 1));
    assert_eq!(sh[5], ("≤1/500s".to_string(), 3), "三行 1/1000");

    // camelCase 载荷形状
    let json = serde_json::to_value(&stats).unwrap();
    assert!(json["focalBuckets"].is_array());
    assert_eq!(json["focalBuckets"][5]["max"], serde_json::Value::Null);
    assert_eq!(json["isoBuckets"][0]["label"], "≤100");
}

// ---------------------------------------------------------------------------
// F8 两级去重
// ---------------------------------------------------------------------------

/// pHash 三族：A（基准）、A'（汉明 2，近重复）、B（汉明 32，无关）。
const HASH_A: u64 = 0x00ff_00ff_00ff_00ff;
const HASH_A_NEAR: u64 = 0x00ff_00ff_00ff_00f0; // 汉明 4 ≤6
const HASH_B: u64 = 0xff00_ff00_ff00_ff33;

fn set_phash(db: &db::Db, path: &str, phash: u64) {
    db.0.execute(
        "UPDATE assets SET phash = ?2 WHERE path = ?1",
        rusqlite::params![path, phash as i64],
    )
    .unwrap();
}

fn setup_dup_library() -> (tempfile::TempDir, std::path::PathBuf, ipc::AppState) {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    // exact 组：d1/d2/d3 同 (size, xxhash)；d4 单独
    for name in ["d1.jpg", "d2.jpg", "d3.jpg"] {
        let mut row = asset(&format!("X:/p/{name}"), Some("2026-09-22T10:00:00.000Z"));
        row.size = 999;
        row.xxhash = 0xdead;
        db.insert_asset(&row).unwrap();
    }
    db.insert_asset(&asset("X:/p/solo.jpg", Some("2026-09-22T10:00:00.000Z")))
        .unwrap();
    let _ = db;
    (src, db_dir.keep(), state)
}

#[test]
fn duplicates_exact_groups_by_size_xxhash() {
    let (_src, _db_dir, state) = setup_dup_library();
    let groups = ipc::duplicates::fetch_duplicates_list(&state, "exact", 0, 50).unwrap();
    assert_eq!(groups.len(), 1, "一组三胞胎");
    assert_eq!(groups[0].kind, "exact");
    let names: Vec<&str> = groups[0].assets.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["d1.jpg", "d2.jpg", "d3.jpg"],
        "组内 created_at 升序"
    );

    // 未知档位
    assert!(ipc::duplicates::fetch_duplicates_list(&state, "nope", 0, 50).is_err());
    // 分页：after=1 → 空
    assert!(
        ipc::duplicates::fetch_duplicates_list(&state, "exact", 1, 50)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn duplicates_similar_groups_twin_exclusion_and_delete() {
    let (src, db_dir, state) = setup_dup_library();
    let db = open_db(&db_dir);
    // similar 族：a1(A) a2(A') a3(A) 同族；b1(B) 孤儿
    for (name, hash, at) in [
        ("a1.jpg", HASH_A, "2026-09-22T11:00:00.000Z"),
        ("a2.jpg", HASH_A_NEAR, "2026-09-22T11:00:01.000Z"),
        ("a3.jpg", HASH_A, "2026-09-22T11:00:02.000Z"),
        ("b1.jpg", HASH_B, "2026-09-22T11:00:03.000Z"),
    ] {
        db.insert_asset(&asset(&format!("X:/p/{name}"), Some(at)))
            .unwrap();
        set_phash(&db, &format!("X:/p/{name}"), hash);
    }
    // 孪生：a3 的 RAW 版本（pair 双向）——同拍摄不算重复（不与 a3 连边）
    let mut raw = asset("X:/p/a3.NEF", Some("2026-09-22T11:00:02.000Z"));
    raw.kind = AssetKind::Raw;
    db.insert_asset(&raw).unwrap();
    set_phash(&db, "X:/p/a3.NEF", HASH_A);
    let id_a3 = db.asset_id_by_path("X:/p/a3.jpg").unwrap().unwrap();
    let id_raw = db.asset_id_by_path("X:/p/a3.NEF").unwrap().unwrap();
    db.0.execute(
        "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
        rusqlite::params![id_a3, id_raw],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET pair_asset_id = ?2 WHERE id = ?1",
        rusqlite::params![id_raw, id_a3],
    )
    .unwrap();

    let groups = ipc::duplicates::fetch_duplicates_list(&state, "similar", 0, 50).unwrap();
    assert_eq!(
        groups.len(),
        1,
        "一族近重复（b1 汉明 32 不入；RAW 孪生被排除边）"
    );
    let names: Vec<&str> = groups[0].assets.iter().map(|a| a.name.as_str()).collect();
    assert!(!names.contains(&"b1.jpg"));
    assert!(!names.contains(&"a3.NEF"), "孪生不成组员");
    assert!(names.contains(&"a1.jpg") && names.contains(&"a2.jpg") && names.contains(&"a3.jpg"));

    // 桶表懒重建：直接清桶再查 → 自动重建同结果
    db.0.execute("DELETE FROM similar_bucket", []).unwrap();
    let again = ipc::duplicates::fetch_duplicates_list(&state, "similar", 0, 50).unwrap();
    assert_eq!(again.len(), 1);

    // 删除：删 a1/a2（文件不存在——path 是 X:/ 假路径，容忍缺失）→ 行删 + 级联
    let id_a1 = db.asset_id_by_path("X:/p/a1.jpg").unwrap().unwrap();
    let id_a2 = db.asset_id_by_path("X:/p/a2.jpg").unwrap().unwrap();
    let deleted =
        ipc::duplicates::fetch_duplicate_delete(&state, &[id_a1, id_a2, 999_999]).unwrap();
    assert_eq!(deleted, 2, "删 2（999999 幂等跳过）");
    let remains: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
    assert_eq!(remains, 7, "exact 4(d1-d3+solo) + a3 + b1 + raw");
    // 级联：similar_bucket 无悬挂行
    let dangling: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM similar_bucket sb \
             WHERE NOT EXISTS (SELECT 1 FROM assets a WHERE a.id = sb.asset_id)",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(dangling, 0);
    // 日志记账
    let logs: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM logs WHERE message LIKE '重复删除：%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(logs, 2);
    // 重查 similar：a3 单张不成组 → 空列表
    assert!(
        ipc::duplicates::fetch_duplicates_list(&state, "similar", 0, 50)
            .unwrap()
            .is_empty()
    );
    let _ = src;
}

#[test]
fn similar_bucket_incremental_and_ensure_paths() {
    let (src, db_dir, _state) = setup_dup_library();
    let db = open_db(&db_dir);
    // 模拟 phash 任务路径：insert_similar_buckets 增量（4 段）
    db.insert_asset(&asset("X:/p/x.jpg", None)).unwrap();
    let id = db.asset_id_by_path("X:/p/x.jpg").unwrap().unwrap();
    db.insert_similar_buckets(id, HASH_A).unwrap();
    let rows: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM similar_bucket WHERE asset_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(rows, 4, "4 段");
    // 幂等
    db.insert_similar_buckets(id, HASH_A).unwrap();
    let rows: i64 =
        db.0.query_row("SELECT COUNT(*) FROM similar_bucket", [], |r| r.get(0))
            .unwrap();
    assert_eq!(rows, 4);
    // 行数守卫：x 设了 phash 但桶已齐 → 不重建；把 x 的 phash 清了制造不齐 → 重建
    db.0.execute("UPDATE assets SET phash = NULL WHERE id = ?1", [id])
        .unwrap();
    // x 现在有桶无 phash：phash_rows=0, buckets=4 → 不等 → 全量重建（清空）
    assert!(db.ensure_similar_buckets().unwrap(), "不齐触发重建");
    let rows: i64 =
        db.0.query_row("SELECT COUNT(*) FROM similar_bucket", [], |r| r.get(0))
            .unwrap();
    assert_eq!(rows, 0, "重建后无 phash 行 → 桶清空");
    assert!(!db.ensure_similar_buckets().unwrap(), "对齐后短路");
    let _ = src;
}

/// 汉明边界：距离 7 不入组（SIMILAR_HAMMING_MAX=6）。
#[test]
fn similar_hamming_boundary_excludes_distance_7() {
    let (src, db_dir, state) = setup_dup_library();
    let db = open_db(&db_dir);
    // 0x00ff ^ 0x00f8 = 0x07 → 3 位；取低段大差：构造距离 7
    let h1: u64 = 0x0000_0000_0000_0000;
    let h2: u64 = 0x0000_0000_0000_007f; // 汉明 7——但同段(段3)不等 → 无候选 → 恰好也测多探针
    db.insert_asset(&asset("X:/p/e1.jpg", None)).unwrap();
    db.insert_asset(&asset("X:/p/e2.jpg", None)).unwrap();
    set_phash(&db, "X:/p/e1.jpg", h1);
    set_phash(&db, "X:/p/e2.jpg", h2);
    let groups = ipc::duplicates::fetch_duplicates_list(&state, "similar", 0, 50).unwrap();
    assert!(groups.is_empty(), "距离 7（且四段全异）不成组");
    let _ = src;
}

#[test]
fn missing_equipment_is_excluded_from_lists_buckets_and_ranges_in_existing_libraries() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    // Direct inserts exercise historical database rows without an EXIF rebuild.
    let invalid = [
        None,
        Some("---"),
        Some("  --  "),
        Some("N/A"),
        Some("unknown"),
        Some("0"),
        Some("0.0"),
        Some("0/0"),
        Some("1/0"),
        Some("12junk"),
        Some("1.2.3"),
        Some("-1"),
        Some("1/2/3"),
    ];
    for (i, value) in invalid.iter().enumerate() {
        let mut row = asset(&format!("invalid{i}.jpg"), None);
        row.camera = Some(["---", "N/A", "  unknown  "][i % 3].into());
        row.lens = Some("---".into());
        row.iso = Some(0);
        row.f_number = value.map(str::to_string);
        row.focal_length = value.map(str::to_string);
        row.exposure_time = value.map(str::to_string);
        db.insert_asset(&row).unwrap();
    }
    let mut valid = asset("valid.jpg", None);
    valid.lens = Some("  Third-party 23mm F1.4  ".into());
    valid.focal_length = Some("23".into());
    valid.f_number = Some("1.4".into());
    valid.iso = Some(100);
    valid.exposure_time = Some("2/1000".into());
    db.insert_asset(&valid).unwrap();
    let valid_id = db.asset_id_by_path("valid.jpg").unwrap().unwrap();

    // Missing one field must not discard valid fields of the same photograph.
    let mut partial = asset("partial.jpg", None);
    partial.lens = Some("N/A".into());
    partial.iso = Some(200);
    db.insert_asset(&partial).unwrap();
    assert_eq!(db.camera_list().unwrap()[0].count, 2);
    let lenses = db.lens_list().unwrap();
    assert_eq!(lenses.len(), 1);
    assert_eq!(lenses[0].camera, "Third-party 23mm F1.4");
    assert_eq!(lenses[0].count, 1);
    let buckets = db.gear_bucket_counts().unwrap();
    assert_eq!(buckets[..6].iter().sum::<i64>(), 1);
    assert_eq!(buckets[6..13].iter().sum::<i64>(), 2);
    assert_eq!(buckets[13..19].iter().sum::<i64>(), 1);
    assert_eq!(buckets[19..].iter().sum::<i64>(), 1);
    assert_eq!(buckets[0], 1);
    assert_eq!(buckets[13], 1);
    assert_eq!(buckets[24], 1);

    let cases = [
        db::AssetFilters {
            aperture_max: Some(1.4),
            ..Default::default()
        },
        db::AssetFilters {
            focal_max: Some(24.0),
            ..Default::default()
        },
        db::AssetFilters {
            iso_max: Some(100),
            ..Default::default()
        },
        db::AssetFilters {
            shutter_max: Some(1.0 / 500.0),
            ..Default::default()
        },
        db::AssetFilters {
            lenses: vec!["Third-party 23mm F1.4".into()],
            ..Default::default()
        },
    ];
    for filters in cases {
        assert_eq!(db.assets_count(&filters).unwrap(), 1);
        let rows = db.assets_page(0, 100, &filters).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, valid_id);
    }
    for placeholder in ["---", "N/A", "unknown"] {
        for filters in [
            db::AssetFilters {
                lenses: vec![placeholder.into()],
                ..Default::default()
            },
            db::AssetFilters {
                cameras: vec![placeholder.into()],
                ..Default::default()
            },
        ] {
            assert_eq!(db.assets_count(&filters).unwrap(), 0);
        }
    }
    assert_eq!(
        db.assets_count(&db::AssetFilters::default()).unwrap(),
        invalid.len() as u64 + 2
    );
}
