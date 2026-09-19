//! 画廊数据源（M3）：assets_page keyset 分页（NULL captured_at 最先 +
//! captured DESC + id tiebreak）、AssetFilters（kind/camera/时间范围）参数化
//! 过滤、asset_group_dates 本地时区日期分组降序（unknown 组置顶）、
//! asset_detail 全字段 + 同指纹 (size,xxh) 重复计数、DTO camelCase 契约。

mod common;

pub use common::{
    ai, db, devices, events, import, ipc, metadata, migrate, settings, tasks, thumbs,
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
            kind: Some(AssetKind::Photo),
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
            camera: Some("Sony A7R5".into()),
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
            kind: Some(AssetKind::Photo),
            camera: Some("Sony A7R5".into()),
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
    };
    let json = serde_json::to_value(&dto).unwrap();
    assert_eq!(json["capturedAt"], "2026-01-01T00:00:00.000Z");
    assert_eq!(json["sizeBytes"], 123);
    assert_eq!(json["kind"], "video");

    let filters: AssetFilters = serde_json::from_str(
        r#"{"kind":"raw","capturedAfter":"2026-01-01T00:00:00Z","camera":"c"}"#,
    )
    .unwrap();
    assert_eq!(filters.kind, Some(AssetKind::Raw));
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
    eprintln!("真实库: 总数 {total}，NULL {nulls}，日期组 {dated}，unknown {unknown:?}");
    assert_eq!(total, 117, "资产总数");
    assert_eq!(nulls, 5, "NULL captured_at 数");
    assert_eq!(
        dated, 5,
        "本地日期组数（UTC 为 4：06-28 晚间 UTC 拍摄跨入本地 06-29）"
    );
    assert!(unknown.is_some_and(|g| g.count == 5), "unknown 组 count=5");
    assert!(
        groups.iter().all(|g| g.cover_asset_id > 0),
        "每组 cover 必须有效"
    );
    // 首页 NULL 优先：分页第一页首个必为 NULL 资产
    let page = database
        .assets_page(0, 5, &db::AssetFilters::default())
        .unwrap();
    assert!(page[0].captured_at.is_none(), "NULL 资产排最前");
}
