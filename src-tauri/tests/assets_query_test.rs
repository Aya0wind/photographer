//! 画廊数据源（M3）：assets_page keyset 分页（NULL captured_at 最先 +
//! captured DESC + id tiebreak）、AssetFilters（kind/camera/时间范围）参数化
//! 过滤、asset_group_dates 本地时区日期分组降序（unknown 组置顶）、
//! asset_detail 全字段 + 同指纹 (size,xxh) 重复计数、DTO camelCase 契约。

mod common;

pub use common::{
    ai, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use chrono::{Datelike, Local};
use common::{state_with_library, utc};
use db::AssetRow;
use events::AssetKind;
use ipc::assets::{AssetDetailDto, AssetDto, AssetFilters, DateGroupDto};

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
        sha256: [7; 32],
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
fn page_orders_nulls_first_then_captured_desc_with_id_tiebreak() {
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

    // NULL 最前（id 倒序），随后 captured 降序；captured 相同按 id 倒序
    let all = page(&state, 0, 100, AssetFilters::default());
    let ids: Vec<i64> = all.iter().map(|a| a.id).collect();
    assert_eq!(ids, vec![null_b, null_a, dated_2, dated_1, dated_3]);

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
    assert_eq!(sony.len(), 2);

    // 时间范围（RFC3339 带偏移的输入也要正确归一比较）
    let range = AssetFilters {
        captured_after: Some("2026-01-15T00:00:00+08:00".into()),
        captured_before: Some("2026-03-15T00:00:00Z".into()),
        ..Default::default()
    };
    let in_range = page(&state, 0, 100, range);
    let names: Vec<&str> = in_range.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["r1.cr3", "v1.mp4"], "区间内按 captured 降序");

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
    // unknown 置顶（与画廊页序一致），随后日期降序
    assert_eq!(groups[0].date, "unknown");
    assert_eq!(groups[0].count, 2);
    assert_eq!(
        groups[0].cover_asset_id, unknown_cover,
        "unknown 组 cover=组内 id 最大（同序首张）"
    );
    assert_eq!(groups[1].date, local_day(t2), "最近日期在前");
    assert_eq!(groups[1].count, 1);
    assert_eq!(groups[2].date, local_day(t1));
    assert_eq!(groups[2].count, 2, "同日合并计数");
    // cover 是组内最新那张（captured 相同 → id 大者）
    let cover_of_day1 = groups[2].cover_asset_id;
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
    };
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["capturedAt"], "2026-01-01T00:00:00.000Z");
    assert_eq!(json["sizeBytes"], 123);
    assert_eq!(json["kind"], "video");
    assert_eq!(json["pairId"], serde_json::Value::Null);

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
            .all(|w| (w[0].date == "unknown") || (w[1].date != "unknown" && w[0].date > w[1].date)),
        "日期组降序、unknown 置顶: {groups:?}"
    );
    // 首页 NULL 优先：分页第一页首个必为 NULL 资产
    let page = database
        .assets_page(0, 5, &db::AssetFilters::default())
        .unwrap();
    assert!(page[0].captured_at.is_none(), "NULL 资产排最前");
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
    row.sha256 = [3; 32];
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
        sha256: [3; 32],
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
    assert_eq!(videos.len(), 1);
    assert_eq!(
        videos[0].kind,
        AssetKind::Video,
        "DTO kind 仍为真实格式分类"
    );
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
            "INSERT INTO assets (path, filename, size, mtime, xxhash, sha256, kind, captured_at, \
             camera, source, created_at, origin) VALUES ('e.jpg','e.jpg',1,'2026',1,x'00', \
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
    assert_eq!(raw_dto.pair_id, Some(new_jpg_id));
}

#[test]
fn legacy_v3_library_migrates_to_v4_and_stays_readable() {
    let dir = tempfile::tempdir().unwrap();
    // 手工建 user_version=3 的旧库（0001..0003 形态）+ 一行存量资产
    let conn = rusqlite::Connection::open(dir.path().join("library.db")).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE assets (
            id INTEGER PRIMARY KEY, path TEXT NOT NULL UNIQUE, filename TEXT NOT NULL,
            size INTEGER NOT NULL, mtime TEXT NOT NULL, xxhash INTEGER NOT NULL,
            sha256 BLOB NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('photo','raw','video','other')),
            captured_at TEXT, camera TEXT, source TEXT NOT NULL, created_at TEXT NOT NULL
        );
        CREATE INDEX idx_assets_sha256 ON assets (sha256);
        CREATE INDEX idx_assets_captured_at ON assets (captured_at);
        CREATE INDEX idx_assets_xxhash ON assets (xxhash);
        CREATE TABLE jobs (
            id INTEGER PRIMARY KEY, kind TEXT NOT NULL, device_id TEXT NOT NULL,
            device_name TEXT NOT NULL, status TEXT NOT NULL CHECK (status IN ('running','paused','done','cancelled','failed')),
            total_files INTEGER NOT NULL, total_bytes INTEGER NOT NULL, stats_json TEXT,
            started_at TEXT NOT NULL, finished_at TEXT
        );
        CREATE TABLE job_files (
            job_id INTEGER NOT NULL REFERENCES jobs (id) ON DELETE CASCADE,
            src TEXT NOT NULL, dst TEXT NOT NULL, size INTEGER NOT NULL,
            state TEXT NOT NULL CHECK (state IN ('pending','copying','verified','skipped','failed')),
            error TEXT, xxhash INTEGER, sha256 BLOB, PRIMARY KEY (job_id, src)
        );
        CREATE INDEX idx_job_files_state ON job_files (job_id, state);
        CREATE TABLE logs (
            id INTEGER PRIMARY KEY, ts TEXT NOT NULL, level TEXT NOT NULL,
            job_id INTEGER, message TEXT NOT NULL
        );
        CREATE INDEX idx_logs_job_id ON logs (job_id, id);
        ALTER TABLE jobs ADD COLUMN plan_json TEXT;
        CREATE INDEX idx_assets_size_filename ON assets (size, filename);
        ALTER TABLE assets ADD COLUMN origin TEXT NOT NULL DEFAULT 'imported';
        ALTER TABLE job_files ADD COLUMN dst2 TEXT NOT NULL DEFAULT '';
        PRAGMA user_version = 3;
        INSERT INTO assets (path, filename, size, mtime, xxhash, sha256, kind, captured_at, camera, source, created_at)
        VALUES ('X:\old\a.jpg', 'a.jpg', 5, '2026', 1,
                x'0101010101010101010101010101010101010101010101010101010101010101',
                'photo', NULL, NULL, 'imported', '2026');
        "#,
    )
    .unwrap();
    drop(conn);

    // 开库自动迁移到 v4：旧行可读（新列全 None）、新行可写（拍摄参数+配对）
    let database = common::open_db(dir.path());
    let version: i64 = database
        .0
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert!(version >= 4, "迁移应推进到 0004+");

    let old_id = database.asset_id_by_path(r"X:\old\a.jpg").unwrap().unwrap();
    let old_row = database.asset_by_id(old_id).unwrap().unwrap();
    assert_eq!(old_row.iso, None);
    assert_eq!(old_row.width, None);
    assert_eq!(old_row.pair_asset_id, None);
    assert_eq!(old_row.kind, AssetKind::Photo);

    let mut rich = base_row();
    rich.iso = Some(800);
    let _ = ins_full(&database, r"X:\old\a.jpg", rich); // 同路径覆盖（REPLACE 换 id）
    let replaced = database.asset_by_id(old_id).unwrap();
    assert!(replaced.is_none(), "REPLACE 换 id 后旧 id 失效");
    let new_id = database.asset_id_by_path(r"X:\old\a.jpg").unwrap().unwrap();
    assert_eq!(
        database.asset_by_id(new_id).unwrap().unwrap().iso,
        Some(800)
    );
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
