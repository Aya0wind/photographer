//! 画廊数据源（M3）：assets_page keyset 分页（未知日期沉底 +
//! captured DESC + id tiebreak）、AssetFilters（kind/camera/时间范围）参数化
//! 过滤、asset_group_dates 本地时区日期分组降序（unknown 组沉底）、
//! asset_detail 全字段 + 同指纹 (size,xxh) 重复计数、DTO camelCase 契约。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan,
    settings, tasks, thumbs,
};

use std::time::Duration;

use chrono::{Datelike, Local};
use common::{state_with_library, utc};
use db::AssetRow;
use events::AssetKind;
use ipc::assets::{AssetDetailDto, AssetDto, AssetFilters, DateGroupDto};

#[test]
fn timeline_seek_is_inclusive_and_loads_nearest_newer_page_without_leaking_filters() {
    let dir = tempfile::tempdir().unwrap();
    let database = common::open_db(dir.path());
    let mut ids = Vec::new();
    for day in 1..=8 {
        let date = format!("2026-01-{day:02}T12:00:00.000Z");
        ids.push(ins(
            &database,
            &format!("keep-{day}.jpg"),
            Some(&date),
            AssetKind::Photo,
            Some("Keep"),
            10,
            day,
        ));
        ins(
            &database,
            &format!("exclude-{day}.jpg"),
            Some(&date),
            AssetKind::Photo,
            Some("Exclude"),
            10,
            day + 20,
        );
    }
    let filters = AssetFilters {
        cameras: vec!["Keep".into()],
        ..Default::default()
    };
    let first = database.assets_seek(ids[3], 3, &filters, false).unwrap();
    assert_eq!(
        first.iter().map(|asset| asset.id).collect::<Vec<_>>(),
        vec![ids[3], ids[2], ids[1]]
    );
    let next = database.assets_page(ids[1], 3, &filters).unwrap();
    assert_eq!(
        next.iter().map(|asset| asset.id).collect::<Vec<_>>(),
        vec![ids[0]]
    );
    let newer = database.assets_seek(ids[3], 2, &filters, true).unwrap();
    assert_eq!(
        newer.iter().map(|asset| asset.id).collect::<Vec<_>>(),
        vec![ids[5], ids[4]]
    );
    assert!(database
        .assets_seek(999999, 10, &filters, false)
        .unwrap()
        .is_empty());

    let catalog = database.asset_group_dates_filtered(&filters).unwrap();
    assert_eq!(catalog.len(), 8);
    assert!(catalog
        .iter()
        .all(|entry| entry.count == 1 && ids.contains(&entry.cover_asset_id)));
    assert_eq!(catalog[0].cover_asset_id, ids[7]);
    database
        .0
        .execute("UPDATE assets SET in_trash = 1 WHERE id = ?1", [ids[7]])
        .unwrap();
    assert_eq!(
        database.asset_group_dates_filtered(&filters).unwrap().len(),
        7
    );
}

/// 直插一行资产（path 唯一），返回自增 id。
fn ins(
    db: &db::Db,
    path: &str,
    captured: Option<&str>,
    kind: AssetKind,
    camera: Option<&str>,
    size: u64,
    xxhash: u64,
) -> i64 {
    db.insert_asset(&AssetRow {
        path: path.to_string(),
        filename: path.rsplit(['\\', '/']).next().unwrap_or(path).to_string(),
        size,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash,
        kind,
        captured_at: captured.map(str::to_string),
        camera: camera.map(str::to_string),
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

fn page(state: &ipc::AppState, after_id: i64, limit: u32, filters: AssetFilters) -> Vec<AssetDto> {
    ipc::assets::fetch_assets_page(state, after_id, limit, filters).unwrap()
}

/// 造一个已迁移库的 AppState（库 db_dir 指向 db_dir）。
fn query_state(db_dir: &std::path::Path) -> ipc::AppState {
    let src = tempfile::tempdir().unwrap();
    state_with_library(db_dir, src.path(), Duration::from_millis(1))
}

#[test]
fn count_matches_filtered_pages_and_exif_display_orientation() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let portrait = ins(
        &database,
        "portrait.jpg",
        None,
        AssetKind::Photo,
        None,
        10,
        1,
    );
    let landscape = ins(
        &database,
        "landscape.jpg",
        None,
        AssetKind::Photo,
        None,
        10,
        2,
    );
    database.0.execute(
        "UPDATE assets SET width = 6000, height = 4000, orientation = 6, flagged = 1, rating = 5 WHERE id = ?1",
        [portrait],
    ).unwrap();
    database
        .0
        .execute(
            "UPDATE assets SET width = 6000, height = 4000, orientation = 1 WHERE id = ?1",
            [landscape],
        )
        .unwrap();
    let filters = AssetFilters {
        orientation: Some("portrait".to_string()),
        flagged: Some(true),
        rating_min: Some(5),
        ..Default::default()
    };
    let state = query_state(db_dir.path());
    let items = page(&state, 0, 100, filters.clone());
    assert_eq!(
        items.iter().map(|a| a.id).collect::<Vec<_>>(),
        vec![portrait]
    );
    assert!(items[0].flagged);
    assert_eq!(items[0].rating, 5);
    assert_eq!(database.assets_count(&filters).unwrap(), items.len() as u64);
    assert_eq!(database.assets_count(&AssetFilters::default()).unwrap(), 2);
}

#[test]
fn page_orders_captured_desc_with_id_tiebreak_then_unknown_dates() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    // 3 个 NULL + 4 个有日期（含 captured 相同的 id tiebreak 对）
    let null_a = ins(&database, "a.jpg", None, AssetKind::Photo, None, 10, 1);
    let dated_1 = ins(
        &database,
        "b.jpg",
        Some("2026-01-02T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        2,
    );
    let dated_2 = ins(
        &database,
        "c.jpg",
        Some("2026-01-02T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        3,
    );
    let dated_3 = ins(
        &database,
        "d.jpg",
        Some("2026-01-01T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        4,
    );
    let null_b = ins(&database, "e.jpg", None, AssetKind::Photo, None, 10, 5);
    let state = query_state(db_dir.path());

    // captured 降序，同时间 id 倒序；NULL 沉底且组内 id 倒序。
    let all = page(&state, 0, 100, AssetFilters::default());
    let ids: Vec<i64> = all.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![dated_2, dated_1, dated_3, null_b, null_a]);
    // Jumping to unknown dates must still allow scrolling upward into dated photos.
    let older_window = database
        .assets_seek(null_b, 2, &AssetFilters::default(), false)
        .unwrap();
    assert_eq!(
        older_window
            .iter()
            .map(|asset| asset.id)
            .collect::<Vec<_>>(),
        vec![null_b, null_a]
    );
    let above_unknown = database
        .assets_seek(null_b, 2, &AssetFilters::default(), true)
        .unwrap();
    assert_eq!(
        above_unknown
            .iter()
            .map(|asset| asset.id)
            .collect::<Vec<_>>(),
        vec![dated_1, dated_3]
    );

    // keyset 翻页：limit=2 走完 5 条，不重不漏
    let mut cursor = 0i64;
    let mut walked = Vec::new();
    loop {
        let page_items = page(&state, cursor, 2, AssetFilters::default());
        if page_items.is_empty() {
            break;
        }
        cursor = page_items.last().unwrap().id;
        walked.extend(page_items.iter().map(|a| a.id));
    }
    assert_eq!(walked, ids, "翻页拼接必须与全量序一致");
}

#[test]
fn page_filters_kind_camera_and_time_range() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    ins(
        &database,
        "p1.jpg",
        Some("2026-01-01T00:00:00.000Z"),
        AssetKind::Photo,
        Some("Sony A7R5"),
        10,
        1,
    );
    ins(
        &database,
        "v1.mp4",
        Some("2026-02-01T00:00:00.000Z"),
        AssetKind::Video,
        Some("Sony A7R5"),
        10,
        2,
    );
    ins(
        &database,
        "r1.cr3",
        Some("2026-03-01T00:00:00.000Z"),
        AssetKind::Raw,
        None,
        10,
        3,
    );
    ins(
        &database,
        "p2.jpg",
        Some("2026-04-01T00:00:00.000Z"),
        AssetKind::Photo,
        Some("Fuji X-T5"),
        10,
        4,
    );
    let state = query_state(db_dir.path());

    // kind 过滤
    let photos = page(
        &state,
        0,
        100,
        AssetFilters {
            kinds: vec![AssetKind::Photo],
            ..Default::default()
        },
    );
    assert_eq!(photos.len(), 2);
    assert!(photos.iter().all(|a| a.kind == AssetKind::Photo));

    // camera 精确匹配
    let sony = page(
        &state,
        0,
        100,
        AssetFilters {
            cameras: vec!["Sony A7R5".into()],
            ..Default::default()
        },
    );
    assert_eq!(sony.len(), 1);

    // 时间范围（RFC3339 带偏移的输入也要正确归一比较）
    let range = AssetFilters {
        captured_after: Some("2026-01-15T00:00:00+08:00".into()),
        captured_before: Some("2026-03-15T00:00:00Z".into()),
        ..Default::default()
    };
    let in_range = page(&state, 0, 100, range);
    let names: Vec<&str> = in_range.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["r1.cr3"], "旧视频记录不进入日期筛选结果");

    // 组合过滤
    let combo = page(
        &state,
        0,
        100,
        AssetFilters {
            kinds: vec![AssetKind::Photo],
            cameras: vec!["Sony A7R5".into()],
            ..Default::default()
        },
    );
    assert_eq!(combo.len(), 1);
    assert_eq!(combo[0].name, "p1.jpg");

    // 日期过滤出现时 NULL captured_at 被排除（无日期不落任何区间）
    ins(&database, "n.jpg", None, AssetKind::Photo, None, 10, 9);
    let with_null = page(
        &state,
        0,
        100,
        AssetFilters {
            captured_after: Some("2000-01-01T00:00:00Z".into()),
            ..Default::default()
        },
    );
    assert!(with_null.iter().all(|a| a.name != "n.jpg"));
}

#[test]
fn page_rejects_invalid_date_filter_and_missing_cursor_falls_back_to_first_page() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let first = ins(
        &database,
        "a.jpg",
        Some("2026-01-01T00:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        1,
    );
    let state = query_state(db_dir.path());

    // 非法 RFC3339 → 明确报错
    let err = ipc::assets::fetch_assets_page(
        &state,
        0,
        10,
        AssetFilters {
            captured_after: Some("not-a-date".into()),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(err.contains("日期"), "错误信息应指向日期过滤: {err}");

    // 游标行已被删除 → 回退为第一页（而非空页/报错）
    database
        .0
        .execute("DELETE FROM assets WHERE id = ?1", [first + 100])
        .unwrap();
    let fallback = page(&state, first + 100, 10, AssetFilters::default());
    assert_eq!(fallback.len(), 1, "游标缺失按第一页处理");
}

#[test]
fn group_dates_local_timezone_desc_with_unknown_group() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    // 选取 UTC 20:00 的两个时刻：+08:00 本地必跨到次日；期望日期用 Local 动态推导
    let t1 = utc(2026, 9, 18, 20, 0, 0);
    let t2 = utc(2026, 9, 19, 20, 0, 0);
    let local_day = |t: chrono::DateTime<chrono::Utc>| {
        let d = t.with_timezone(&Local);
        format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
    };
    ins(
        &database,
        "d1.jpg",
        Some(&t1.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        AssetKind::Photo,
        None,
        10,
        1,
    );
    ins(
        &database,
        "d2.jpg",
        Some(&t1.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        AssetKind::Photo,
        None,
        10,
        2,
    );
    ins(
        &database,
        "d3.jpg",
        Some(&t2.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)),
        AssetKind::Photo,
        None,
        10,
        3,
    );
    ins(&database, "n1.jpg", None, AssetKind::Photo, None, 10, 4);
    let unknown_cover = ins(&database, "n2.jpg", None, AssetKind::Photo, None, 10, 5);
    let state = query_state(db_dir.path());

    let groups: Vec<DateGroupDto> = ipc::assets::fetch_asset_group_dates(&state).unwrap();
    assert_eq!(groups.len(), 3, "unknown + 两个本地日期组: {groups:?}");
    // 日期降序，unknown 沉底（与画廊分页一致）。
    assert_eq!(groups[2].date, "unknown");
    assert_eq!(groups[2].count, 2);
    assert_eq!(
        groups[2].cover_asset_id, unknown_cover,
        "unknown 组 cover=组内 id 最大（同序首张）"
    );
    assert_eq!(groups[0].date, local_day(t2), "最近日期在前");
    assert_eq!(groups[0].count, 1);
    assert_eq!(groups[1].date, local_day(t1));
    assert_eq!(groups[1].count, 2, "同日合并计数");
    // cover 是组内最新那张（captured 相同 → id 大者）
    let cover_of_day1 = groups[1].cover_asset_id;
    let cover_name: String = database
        .0
        .query_row(
            "SELECT filename FROM assets WHERE id = ?1",
            [cover_of_day1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(cover_name, "d2.jpg");
}

#[test]
fn detail_returns_full_row_and_fingerprint_duplicate_count() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let a = ins(
        &database,
        "a.jpg",
        Some("2026-01-01T00:00:00.000Z"),
        AssetKind::Raw,
        Some("Sony A7R5"),
        4242,
        99,
    );
    ins(
        &database,
        "b.jpg",
        Some("2026-01-02T00:00:00.000Z"),
        AssetKind::Photo,
        None,
        4242,
        99,
    ); // 同指纹
    ins(&database, "c.jpg", None, AssetKind::Photo, None, 4242, 100); // xxh 不同
    let state = query_state(db_dir.path());

    let detail: AssetDetailDto = ipc::assets::fetch_asset_detail(&state, a)
        .unwrap()
        .expect("应找到资产");
    assert_eq!(detail.id, a);
    assert_eq!(detail.asset.path, "a.jpg");
    assert_eq!(detail.asset.filename, "a.jpg");
    assert_eq!(detail.asset.size, 4242);
    assert_eq!(detail.asset.mtime, "2026-09-01T00:00:00.000Z");
    assert_eq!(detail.asset.kind, AssetKind::Raw);
    assert_eq!(detail.asset.camera.as_deref(), Some("Sony A7R5"));
    assert_eq!(detail.asset.origin, "imported");
    assert_eq!(detail.duplicate_count, 1, "同 (size,xxh) 重复计数不含自身");

    assert!(
        ipc::assets::fetch_asset_detail(&state, 9999)
            .unwrap()
            .is_none(),
        "不存在返回 None"
    );
}

#[test]
fn dtos_serialize_camel_case() {
    let dto = AssetDto {
        id: 3,
        path: "X:\\a.jpg".into(),
        name: "a.jpg".into(),
        kind: AssetKind::Video,
        captured_at: Some("2026-01-01T00:00:00.000Z".into()),
        camera: Some("cam".into()),
        size_bytes: 123,
        width: Some(6000),
        height: Some(4000),
        iso: Some(200),
        f_number: Some("2.8".into()),
        exposure_time: Some("1/250".into()),
        focal_length: Some("85".into()),
        lens: Some("FE 85mm".into()),
        pair_id: None,
        thumb_state: 1,
        burst_id: None,
        burst_count: None,
        flagged: false,
        rating: 0,
        color_label: None,
        rejected: false,
        missing: false,
        library_id: Some("photo-library".into()),
    };
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["libraryId"], "photo-library");
    assert_eq!(json["capturedAt"], "2026-01-01T00:00:00.000Z");
    assert_eq!(json["sizeBytes"], 123);
    assert_eq!(json["kind"], "video");
    assert_eq!(json["pairId"], serde_json::Value::Null);
    assert_eq!(json["colorLabel"], serde_json::Value::Null);
    assert_eq!(json["rejected"], false);
    assert_eq!(json["missing"], false, "缺失标记随 DTO（camelCase）");

    let filters: AssetFilters = serde_json::from_str(
        r#"{"kinds":["raw","photo"],"capturedAfter":"2026-01-01T00:00:00Z","cameras":["c"]}"#,
    )
    .unwrap();
    assert_eq!(filters.kinds, vec![AssetKind::Raw, AssetKind::Photo]);
    assert_eq!(filters.cameras, vec!["c"]);
    assert_eq!(
        filters.captured_after.as_deref(),
        Some("2026-01-01T00:00:00Z")
    );

    let group = DateGroupDto {
        date: "unknown".into(),
        count: 2,
        cover_asset_id: 7,
    };
    let json = serde_json::to_value(&group).unwrap();
    assert_eq!(json["coverAssetId"], 7);
}

/// 真实库只读核对（本地 I:\SmartPhoto\主库\library.db，直连单文件不遍历
/// 照片目录）：117 资产 / 5 个 NULL captured_at / 4 个 UTC 日期组（本地
/// 时区 +08 下 2026-06-28 的 UTC 晚间拍摄跨入 06-29，本地分组为 5 组——
/// 契约即本地时区分组）。默认忽略；`--ignored` 手动跑。
#[test]
#[ignore = "依赖本机真实库（只读）"]
fn real_library_contract_smoke() {
    let db_path = std::path::PathBuf::from(r"I:\SmartPhoto\主库\library.db");
    if !db_path.is_file() {
        eprintln!("真实库不存在，跳过: {}", db_path.display());
        return;
    }
    let database = db::Db::open(&db_path).unwrap();
    let total: i64 = database
        .0
        .query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap();
    let nulls: i64 = database
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE captured_at IS NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let groups = database.asset_group_dates().unwrap();
    let dated = groups.iter().filter(|g| g.date != "unknown").count();
    let unknown = groups.iter().find(|g| g.date == "unknown");
    eprintln!("真实库: 总数 {total}，NULL {nulls}，本地日期组 {dated}，unknown {unknown:?}");
    // 结构性不变量（真实库为活数据，不 pin 具体数量；基线 2026-09-19：
    // 117 资产 / 5 NULL / UTC 4 组 = 本地 5 组）
    assert!(total >= 117, "资产总数只增不减");
    assert_eq!(
        nulls as u64,
        unknown.map(|g| g.count).unwrap_or(0),
        "unknown 组 count == NULL 数"
    );
    let dated_sum: u64 = groups
        .iter()
        .filter(|g| g.date != "unknown")
        .map(|g| g.count)
        .sum();
    assert_eq!(dated_sum, total as u64 - nulls as u64, "日期组计数闭合");
    assert!(
        groups
            .windows(2)
            .all(|w| w[1].date == "unknown" || (w[0].date != "unknown" && w[0].date > w[1].date)),
        "日期组降序、unknown 沉底: {groups:?}"
    );
    // 首页为有日期的照片，未知日期在末尾。
    let page = database
        .assets_page(0, 5, &db::AssetFilters::default())
        .unwrap();
    assert!(page[0].captured_at.is_some(), "有日期的资产排最前");
}

// ---------------------------------------------------------------------------
// M3.5：拍摄参数 / kinds 多选 / 纯日期过滤 / camera_list / RAW-JPG 配对
// ---------------------------------------------------------------------------

use db::CameraCountRow;

/// 插入带拍摄参数的资产（其余字段为默认值）。
fn ins_full(db: &db::Db, path: &str, meta: AssetRow) -> i64 {
    let mut row = meta;
    row.path = path.to_string();
    row.filename = path.rsplit(['\\', '/']).next().unwrap_or(path).to_string();
    row.size = 10;
    row.mtime = "2026-09-01T00:00:00.000Z".to_string();
    row.xxhash = 1;
    row.source = "imported".into();
    row.created_at = "2026-09-01T00:00:00.000Z".into();
    db.insert_asset(&row).unwrap();
    db.asset_id_by_path(path).unwrap().unwrap()
}

fn base_row() -> AssetRow {
    AssetRow {
        path: String::new(),
        filename: String::new(),
        size: 10,
        mtime: "2026-09-01T00:00:00.000Z".into(),
        xxhash: 1,
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
        source: "imported".into(),
        created_at: "2026-09-01T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: None,
        height: None,
        iso: None,
        f_number: None,
        exposure_time: None,
        focal_length: None,
        lens: None,
        pair_asset_id: None,
        thumb_state: 0,
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
    }
}

#[test]
fn page_and_detail_carry_shooting_params() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let mut rich = base_row();
    rich.kind = AssetKind::Raw;
    rich.width = Some(6048);
    rich.height = Some(8064);
    rich.iso = Some(1600);
    rich.f_number = Some("2.8".into());
    rich.exposure_time = Some("1/250".into());
    rich.focal_length = Some("85".into());
    rich.lens = Some("FE 85mm F1.8".into());
    let a = ins_full(&database, r"X:\p\rich.arw", rich);
    let b = ins(&database, "plain.jpg", None, AssetKind::Photo, None, 10, 2);
    let state = query_state(db_dir.path());

    let page_items = page(&state, 0, 100, AssetFilters::default());
    let rich_dto = page_items.iter().find(|x| x.id == a).unwrap();
    assert_eq!(rich_dto.width, Some(6048));
    assert_eq!(rich_dto.height, Some(8064));
    assert_eq!(rich_dto.iso, Some(1600));
    assert_eq!(rich_dto.f_number.as_deref(), Some("2.8"));
    assert_eq!(rich_dto.exposure_time.as_deref(), Some("1/250"));
    assert_eq!(rich_dto.focal_length.as_deref(), Some("85"));
    assert_eq!(rich_dto.lens.as_deref(), Some("FE 85mm F1.8"));
    let plain = page_items.iter().find(|x| x.id == b).unwrap();
    assert_eq!(plain.iso, None, "无参数资产字段为 null");

    let detail = ipc::assets::fetch_asset_detail(&state, a).unwrap().unwrap();
    assert_eq!(detail.asset.iso, Some(1600));
    assert_eq!(detail.asset.lens.as_deref(), Some("FE 85mm F1.8"));
}

#[test]
fn kinds_filter_merges_photo_and_raw() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    ins(&database, "a.jpg", None, AssetKind::Photo, None, 10, 1);
    ins(&database, "b.arw", None, AssetKind::Raw, None, 10, 2);
    ins(&database, "c.mp4", None, AssetKind::Video, None, 10, 3);
    let state = query_state(db_dir.path());

    // 用户分类「照片」= photo + raw 合并（SQL IN 多选）
    let photos = page(
        &state,
        0,
        100,
        AssetFilters {
            kinds: vec![AssetKind::Photo, AssetKind::Raw],
            ..Default::default()
        },
    );
    assert_eq!(photos.len(), 2, "photo+raw 合并为「照片」");
    let videos = page(
        &state,
        0,
        100,
        AssetFilters {
            kinds: vec![AssetKind::Video],
            ..Default::default()
        },
    );
    assert!(videos.is_empty(), "旧视频记录不应出现在图库");
}

#[test]
fn date_only_filter_means_local_day_bounds() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    // UTC 2026-09-11T17:30Z = 本地(+08) 09-12 01:30 → 落本地 09-12
    ins(
        &database,
        "d1.jpg",
        Some("2026-09-11T17:30:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        1,
    );
    ins(
        &database,
        "d2.jpg",
        Some("2026-09-12T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        2,
    );
    // UTC 2026-09-10T20:00Z = 本地 09-11 04:00 → 只落本地 09-11
    ins(
        &database,
        "d0.jpg",
        Some("2026-09-10T20:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        3,
    );
    let state = query_state(db_dir.path());

    let day12 = ipc::assets::fetch_assets_page(
        &state,
        0,
        100,
        AssetFilters {
            captured_after: Some("2026-09-12".into()),
            captured_before: Some("2026-09-12".into()),
            ..Default::default()
        },
    )
    .unwrap();
    let mut names: Vec<&str> = day12.iter().map(|a| a.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec!["d1.jpg", "d2.jpg"],
        "纯日期区间=本地当日全天（含 +08 跨日）"
    );

    let day11 = ipc::assets::fetch_assets_page(
        &state,
        0,
        100,
        AssetFilters {
            captured_after: Some("2026-09-11".into()),
            captured_before: Some("2026-09-11".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        day11.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(),
        vec!["d0.jpg"]
    );
}

#[test]
fn camera_list_groups_by_camera_desc() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    ins(
        &database,
        "a.jpg",
        None,
        AssetKind::Photo,
        Some("Sony A7R5"),
        10,
        1,
    );
    ins(
        &database,
        "b.jpg",
        None,
        AssetKind::Photo,
        Some("Sony A7R5"),
        10,
        2,
    );
    ins(
        &database,
        "c.jpg",
        None,
        AssetKind::Photo,
        Some("Fuji X-T5"),
        10,
        3,
    );
    ins(&database, "d.jpg", None, AssetKind::Photo, None, 10, 4); // NULL camera 不入组
    database
        .0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, captured_at, \
             camera, source, created_at, origin) VALUES ('e.jpg','e.jpg',1,'2026',1, \
             'photo',NULL,'','imported','2026','imported')",
            [],
        )
        .unwrap(); // 空串 camera 不入组
    let state = query_state(db_dir.path());

    let list = ipc::assets::fetch_camera_list(&state).unwrap();
    assert_eq!(list.len(), 2, "NULL 与空串相机不入组");
    assert_eq!(list[0].camera, "Sony A7R5");
    assert_eq!(list[0].count, 2);
    assert_eq!(list[1].camera, "Fuji X-T5");
    assert_eq!(list[1].count, 1);

    // 仓储行 camelCase 序列化契约
    let row = CameraCountRow {
        camera: "c".into(),
        count: 3,
    };
    assert_eq!(serde_json::to_value(&row).unwrap()["count"], 3);
}

#[test]
fn raw_jpg_pairing_bidirectional_and_replace_remap() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    // 先 RAW 后 JPG：双向配对
    let raw_id = ins(
        &database,
        r"X:\p\IMG_0001.NEF",
        None,
        AssetKind::Raw,
        None,
        10,
        1,
    );
    let jpg_id = ins(
        &database,
        r"X:\p\IMG_0001.JPG",
        None,
        AssetKind::Photo,
        None,
        10,
        2,
    );
    let raw_pair = database.asset_by_id(raw_id).unwrap().unwrap().pair_asset_id;
    let jpg_pair = database.asset_by_id(jpg_id).unwrap().unwrap().pair_asset_id;
    assert_eq!(raw_pair, Some(jpg_id), "RAW 侧配对指向 JPG");
    assert_eq!(jpg_pair, Some(raw_id), "JPG 侧配对指向 RAW");

    // 无关文件不配对（不同 stem / 不同目录）
    let other = ins(
        &database,
        r"X:\p\IMG_0002.NEF",
        None,
        AssetKind::Raw,
        None,
        10,
        3,
    );
    assert_eq!(
        database.asset_by_id(other).unwrap().unwrap().pair_asset_id,
        None
    );
    let other_dir = ins(
        &database,
        r"X:\q\IMG_0001.HEIC",
        None,
        AssetKind::Photo,
        None,
        10,
        4,
    );
    assert_eq!(
        database
            .asset_by_id(other_dir)
            .unwrap()
            .unwrap()
            .pair_asset_id,
        None
    );

    // 同路径重复导入（REPLACE 换 id）：既有配对引用自动重指新 id
    let old_jpg = database.asset_by_id(jpg_id).unwrap().unwrap();
    database
        .insert_asset(&AssetRow {
            size: 99,
            ..old_jpg.clone()
        })
        .unwrap();
    let new_jpg_id = database
        .asset_id_by_path(r"X:\p\IMG_0001.JPG")
        .unwrap()
        .unwrap();
    assert_ne!(new_jpg_id, jpg_id, "REPLACE 应产生新 id");
    let raw_after = database.asset_by_id(raw_id).unwrap().unwrap().pair_asset_id;
    assert_eq!(raw_after, Some(new_jpg_id), "RAW 侧引用重指新 id");

    // 页面 DTO 带 pairId
    let state = query_state(db_dir.path());
    let page_items = page(&state, 0, 100, AssetFilters::default());
    let raw_dto = page_items
        .iter()
        .find(|x| x.path.ends_with("IMG_0001.NEF"))
        .unwrap();
    assert_eq!(raw_dto.pair_id, Some(raw_id.min(new_jpg_id)));
    let jpg_dto = page_items.iter().find(|x| x.id == new_jpg_id).unwrap();
    assert_eq!(jpg_dto.pair_id, raw_dto.pair_id, "两侧共有一个展示分组 ID");
}

#[test]
fn assets_by_ids_preserves_order_and_skips_missing() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let a = ins(&database, "a.jpg", None, AssetKind::Photo, None, 10, 1);
    let b = ins(&database, "b.jpg", None, AssetKind::Photo, None, 10, 2);
    let state = query_state(db_dir.path());

    let got = ipc::assets::fetch_assets_by_ids(&state, &[b, 9999, a]).unwrap();
    let ids: Vec<i64> = got.iter().map(|d| d.id).collect();
    assert_eq!(ids, vec![b, a], "保持入参顺序、失效 id 跳过");
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].thumb_state, 0);
}

// ---------------------------------------------------------------------------
// M2c（§五 缺失处理 + §一 库归属筛选）：missing 三态 / libraryIds 过滤 /
// 访问时惰性缺失检测（两轮确认）
// ---------------------------------------------------------------------------

#[test]
fn missing_filter_three_states_and_library_filter() {
    // 库根与数据库目录必须互斥（§八-6）——各自独立 tempdir（兄弟目录）。
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    let libs = tempfile::tempdir().unwrap();
    let root_a = libs.path().join("lib-a");
    let root_b = libs.path().join("lib-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let lib_a = database
        .photos_library_register("库甲", &root_a.to_string_lossy(), db_dir.path())
        .unwrap()
        .id;
    let lib_b = database
        .photos_library_register("库乙", &root_b.to_string_lossy(), db_dir.path())
        .unwrap()
        .id;
    let a = ins(&database, "a.jpg", None, AssetKind::Photo, None, 10, 1);
    let b = ins(&database, "b.jpg", None, AssetKind::Photo, None, 10, 2);
    let c = ins(&database, "c.jpg", None, AssetKind::Photo, None, 10, 3);
    database
        .0
        .execute(
            "UPDATE assets SET library_id = ?2 WHERE id = ?1",
            rusqlite::params![a, lib_a],
        )
        .unwrap();
    database
        .0
        .execute(
            "UPDATE assets SET library_id = ?2 WHERE id IN (?1, ?3)",
            rusqlite::params![b, lib_b, c],
        )
        .unwrap();
    database
        .0
        .execute("UPDATE assets SET missing = 1 WHERE id = ?1", [b])
        .unwrap();
    let state = query_state(db_dir.path());

    // 缺失三态：true → 仅 b；false → a/c；缺省 → 全部
    let names_of = |filters: AssetFilters| {
        let mut v: Vec<String> = page(&state, 0, 100, filters)
            .iter()
            .map(|d| d.name.clone())
            .collect();
        v.sort();
        v
    };
    assert_eq!(
        names_of(AssetFilters {
            missing: Some(true),
            ..Default::default()
        }),
        vec!["b.jpg".to_string()]
    );
    assert_eq!(
        names_of(AssetFilters {
            missing: Some(false),
            ..Default::default()
        }),
        vec!["a.jpg".to_string(), "c.jpg".to_string()]
    );
    assert_eq!(names_of(AssetFilters::default()).len(), 3);
    // DTO 带缺失角标数据
    let page_items = page(&state, 0, 100, AssetFilters::default());
    assert!(page_items.iter().find(|d| d.id == b).unwrap().missing);
    assert!(!page_items.iter().find(|d| d.id == a).unwrap().missing);

    // 库归属多选（OR）：甲 + 乙 = 全部；仅甲 = a；仅乙 = b/c
    assert_eq!(
        names_of(AssetFilters {
            library_ids: vec![lib_a.clone(), lib_b.clone()],
            ..Default::default()
        })
        .len(),
        3
    );
    assert_eq!(
        names_of(AssetFilters {
            library_ids: vec![lib_a],
            ..Default::default()
        }),
        vec!["a.jpg".to_string()]
    );
    assert_eq!(
        names_of(AssetFilters {
            library_ids: vec![lib_b.clone()],
            ..Default::default()
        }),
        vec!["b.jpg".to_string(), "c.jpg".to_string()]
    );

    // 计数与分页同口径（assets_count 复用条件构造）
    assert_eq!(
        database
            .assets_count(&AssetFilters {
                missing: Some(true),
                library_ids: vec![lib_b],
                ..Default::default()
            })
            .unwrap(),
        1
    );
}

#[test]
fn detail_access_lazily_detects_missing_with_two_round_confirmation() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let root = dir.path().join("lib");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let database = common::open_db(&db_dir);
    let library = database
        .photos_library_register("惰性检测", &root.to_string_lossy(), &db_dir)
        .unwrap();
    // 资产指向从未存在的路径（库在线）；指纹按「之后放回的同内容」预记，
    // 恢复校验（§八-1 同哈希）才能走单纯恢复分支。
    let gone = root.join("2026").join("06").join("gone.jpg");
    let content = common::shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096);
    let xxh = {
        use xxhash_rust::xxh64::Xxh64;
        let mut hasher = Xxh64::new(0);
        hasher.update(&content);
        hasher.digest()
    };
    let id = ins(
        &database,
        &gone.to_string_lossy(),
        None,
        AssetKind::Photo,
        None,
        content.len() as u64,
        xxh,
    );
    database
        .0
        .execute(
            "UPDATE assets SET library_id = ?2 WHERE id = ?1",
            rusqlite::params![id, library.id],
        )
        .unwrap();
    let state = state_with_library(&db_dir, &root, Duration::from_millis(1));

    let missing_flag = || -> i64 {
        database
            .0
            .query_row("SELECT missing FROM assets WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .unwrap()
    };
    // 首次访问：缺席记账，尚未标（§八-7 两轮确认）
    let _ = ipc::assets::fetch_asset_detail(&state, id).unwrap().unwrap();
    assert_eq!(missing_flag(), 0, "首轮只记账");
    // 第二次访问：确认 → 标 missing；详情 DTO 如实携带
    let detail = ipc::assets::fetch_asset_detail(&state, id).unwrap().unwrap();
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["missing"], 1);
    assert_eq!(missing_flag(), 1, "两轮确认后标缺失");

    // 文件回到在位：访问清缺席账但**不**清 missing（恢复走库扫描的哈希
    // 校验路径，§八-1 同名不同内容保护）；库扫描确认后清除。
    std::fs::create_dir_all(gone.parent().unwrap()).unwrap();
    std::fs::write(&gone, &content).unwrap();
    let _ = ipc::assets::fetch_asset_detail(&state, id).unwrap().unwrap();
    assert_eq!(missing_flag(), 1, "惰性检测不越权恢复（留给扫描校验）");
    let report = scan::scan_library_once(
        &database,
        &db_dir,
        &library,
        &scan::ScanOptions {
            cooldown: Duration::ZERO,
            now: None,
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.restored, 1, "库扫描恢复（同哈希）");
    assert_eq!(missing_flag(), 0);
}

#[test]
fn detail_access_skips_detection_for_offline_library() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let root = dir.path().join("lib");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&root).unwrap();
    let database = common::open_db(&db_dir);
    let library = database
        .photos_library_register("离线库", &root.to_string_lossy(), &db_dir)
        .unwrap();
    let gone = root.join("vanish.jpg");
    database
        .0
        .execute(
            "INSERT INTO assets (path, filename, size, mtime, xxhash, kind, source, created_at, \
             origin, library_id) VALUES (?1, 'vanish.jpg', 4, '2026-01-01T00:00:00Z', 2, 'photo', \
             'imported', '2026-01-01T00:00:00Z', 'imported', ?2)",
            rusqlite::params![gone.to_string_lossy().to_string(), library.id],
        )
        .unwrap();
    let id = database
        .asset_id_by_path(&gone.to_string_lossy())
        .unwrap()
        .unwrap();
    // 整库离线（外置卷拔出）：文件在盘性不可信 → 不标单文件缺失
    database
        .photos_library_set_status(&library.id, "offline")
        .unwrap();
    let state = state_with_library(&db_dir, &root, Duration::from_millis(1));
    for _ in 0..3 {
        let _ = ipc::assets::fetch_asset_detail(&state, id).unwrap().unwrap();
    }
    let missing_flag: i64 = database
        .0
        .query_row("SELECT missing FROM assets WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(missing_flag, 0, "整库离线不判单文件缺失（§五 两级语义）");
}

#[test]
fn editor_ownership_is_preserved_by_gallery_and_direct_asset_queries() {
    let dir=tempfile::tempdir().unwrap();
    let database=common::open_db(dir.path());
    let id=ins(&database,"editor.jpg",None,AssetKind::Photo,None,10,1);
    database.0.execute("UPDATE assets SET library_id='photo-library' WHERE id=?1",[id]).unwrap();
    let state=query_state(dir.path());
    let page=database.assets_page(0,10,&AssetFilters::default()).unwrap();
    assert_eq!(page[0].library_id.as_deref(),Some("photo-library"));
    let dto=ipc::assets::page_row_to_dto(page[0].clone());
    assert_eq!(serde_json::to_value(dto).unwrap()["libraryId"],"photo-library");
    let by_id=ipc::assets::fetch_assets_by_ids(&state,&[id]).unwrap();
    assert_eq!(by_id[0].library_id.as_deref(),Some("photo-library"));
}

// ---------------------------------------------------------------------------
// 地区维度筛选（AssetFilters.region_id：地图子页与画廊共用）
// ---------------------------------------------------------------------------

/// 直插 region 树节点（绕过 geo 数据包回填——筛选只认 asset_regions 的
/// 挂账形态，不依赖经纬度反查，直接按表结构造树）。
fn ins_region(db: &db::Db, id: i64, parent: Option<i64>, level: i64, name: &str) {
    db.0.execute(
        "INSERT INTO regions (id, parent_id, level, name, lat, lon, source) \
         VALUES (?1, ?2, ?3, ?4, 0, 0, 'test')",
        rusqlite::params![id, parent, level, name],
    )
    .unwrap();
}

/// 挂账一行（geo 回填后的形态：每资产每层一行 PK(asset_id, level)、
/// 祖先节点 id 同步在挂）。
fn attach_region(db: &db::Db, asset: i64, region: i64, level: i64) {
    db.0.execute(
        "INSERT INTO asset_regions (asset_id, region_id, level) VALUES (?1, ?2, ?3)",
        rusqlite::params![asset, region, level],
    )
    .unwrap();
}

#[test]
fn region_filter_hits_node_covers_subtree_and_excludes_unattached() {
    let db_dir = tempfile::tempdir().unwrap();
    let database = common::open_db(db_dir.path());
    // 树：中国(0) > 京省(1) > 州市(2) > 甲区(3)；测试国(0) > 北省(1)
    ins_region(&database, 100, None, 0, "中国");
    ins_region(&database, 110, Some(100), 1, "京省");
    ins_region(&database, 120, Some(110), 2, "州市");
    ins_region(&database, 130, Some(120), 3, "甲区");
    ins_region(&database, 200, None, 0, "测试国");
    ins_region(&database, 210, Some(200), 1, "北省");
    let jia = ins(
        &database,
        "jia.jpg",
        Some("2026-01-01T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        1,
    );
    let bei = ins(
        &database,
        "bei.jpg",
        Some("2026-01-02T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        2,
    );
    let _nowhere = ins(
        &database,
        "nowhere.jpg",
        Some("2026-01-03T10:00:00.000Z"),
        AssetKind::Photo,
        None,
        10,
        3,
    );
    // 挂账按回填形态：甲四层全挂、北两层（无省市细分）、无 GPS 零挂接
    for (level, region) in [(0, 100i64), (1, 110), (2, 120), (3, 130)] {
        attach_region(&database, jia, region, level);
    }
    for (level, region) in [(0, 200i64), (1, 210)] {
        attach_region(&database, bei, region, level);
    }
    let state = query_state(db_dir.path());

    // 县级节点：单点命中，排除其他地区与无地区照片
    let county = page(
        &state,
        0,
        100,
        AssetFilters {
            region_id: Some(130),
            ..Default::default()
        },
    );
    assert_eq!(county.iter().map(|a| a.id).collect::<Vec<_>>(), vec![jia]);
    // 树语义：0 级中国 = 全国照片——甲照片 0 级挂的就是父节点 id，
    // 单行挂账即含全部子孙，无需递归展开
    let china = page(
        &state,
        0,
        100,
        AssetFilters {
            region_id: Some(100),
            ..Default::default()
        },
    );
    assert_eq!(china.iter().map(|a| a.id).collect::<Vec<_>>(), vec![jia]);
    // 中间层（省级）同形态命中；跨国不串
    let province = page(
        &state,
        0,
        100,
        AssetFilters {
            region_id: Some(210),
            ..Default::default()
        },
    );
    assert_eq!(province.iter().map(|a| a.id).collect::<Vec<_>>(), vec![bei]);
    // 不存在的 region 节点 = 空集（不是全量兜底）
    assert!(page(
        &state,
        0,
        100,
        AssetFilters {
            region_id: Some(999),
            ..Default::default()
        }
    )
    .is_empty());

    // 计数与分页同口径（共享条件构造器，含空集）
    for region in [100i64, 130, 210, 999] {
        let filters = AssetFilters {
            region_id: Some(region),
            ..Default::default()
        };
        assert_eq!(
            database.assets_count(&filters).unwrap(),
            page(&state, 0, 100, filters.clone()).len() as u64,
            "region={region} 计数=分页行数"
        );
    }
    // None 不过滤：三张全量（含无地区照片）
    assert_eq!(database.assets_count(&AssetFilters::default()).unwrap(), 3);

    // 日期分组链路同样吃该条件：中国仅 1 组 1 张、cover 即甲照片
    let groups = database
        .asset_group_dates_filtered(&AssetFilters {
            region_id: Some(100),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].count, 1);
    assert_eq!(groups[0].cover_asset_id, jia);

    // IPC 契约：camelCase 键 regionId 进（缺省=None 出）
    let filters: AssetFilters = serde_json::from_str(r#"{"regionId":130}"#).unwrap();
    assert_eq!(filters.region_id, Some(130));
    assert_eq!(
        serde_json::to_value(AssetFilters::default()).unwrap()["regionId"],
        serde_json::Value::Null
    );
}
