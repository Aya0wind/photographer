//! M6 连拍分组：pHash 稳定性（重压缩/平移/异场景）、双因子分组（时间切/
//! 场景切/配对跳过/组上限/min_size）、重组幂等与参数变更、IPC 载荷
//! （burst_stats / AssetDto burstId+burstCount）、phash 通道回填 + 重组触发。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::time::Duration;

use bursts::BurstParams;
use common::open_db;
use db::AssetRow;
use events::AssetKind;
use metadata::phash::{hamming, phash_of_gray};
use settings::AiSettings;

fn asset(path: &str, kind: AssetKind, captured_at: Option<&str>) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-21T00:00:00.000Z".into(),
        xxhash: 1,
        kind,
        captured_at: captured_at.map(Into::into),
        camera: Some("Sony A7M4".into()),
        source: "imported".into(),
        created_at: "2026-09-21T00:00:00.000Z".into(),
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

fn params() -> BurstParams {
    BurstParams {
        gap_ms: 2000,
        hamming_max: 10,
        min_size: 2,
    }
}

fn burst_of(db: &db::Db, path: &str) -> Option<i64> {
    db.0.query_row("SELECT burst_id FROM assets WHERE path = ?1", [path], |r| {
        r.get(0)
    })
    .unwrap()
}

fn set_phash(db: &db::Db, path: &str, phash: u64) {
    db.0.execute(
        "UPDATE assets SET phash = ?2 WHERE path = ?1",
        [path.to_string(), (phash as i64).to_string()],
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// pHash 稳定性
// ---------------------------------------------------------------------------

/// 合成场景：渐变背景 + 若干实心几何形状（低频主导，JPEG 重压缩稳定）。
fn scene_image(seed: u8, offset: i32) -> image::GrayImage {
    let mut img = image::GrayImage::new(200, 200);
    for (x, y, px) in img.enumerate_pixels_mut() {
        let gx = (x as i32 + offset).rem_euclid(200) as u32;
        *px = image::Luma([(gx / 8 + y / 16 + u32::from(seed) * 3) as u8]);
    }
    // 几何形状（错位平移随 offset）
    for (cx, cy, r) in [(60u32, 60u32, 24u32), (140, 130, 18)] {
        let cx = ((cx as i32 + offset).rem_euclid(200)) as u32;
        for (x, y, px) in img.enumerate_pixels_mut() {
            let (dx, dy) = (x.abs_diff(cx) as f32, y.abs_diff(cy) as f32);
            if (dx * dx + dy * dy) as u32 <= r * r {
                *px = image::Luma([230]);
            }
        }
    }
    img
}

fn jpeg_roundtrip(img: &image::GrayImage, quality: u8) -> image::GrayImage {
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    image::DynamicImage::ImageLuma8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    image::ImageReader::new(std::io::Cursor::new(buf))
        .with_guessed_format()
        .unwrap()
        .decode()
        .unwrap()
        .to_luma8()
}

#[test]
fn phash_stability_same_scene_and_distance_across_scenes() {
    let base = scene_image(0, 0);
    let h_base = phash_of_gray(&base);

    // 确定性：同图同哈希
    assert_eq!(phash_of_gray(&base), h_base);

    // JPEG 重压缩（q90 / q60）：汉明 ≤ 10
    let re90 = jpeg_roundtrip(&base, 90);
    let re60 = jpeg_roundtrip(&base, 60);
    assert!(
        hamming(h_base, phash_of_gray(&re90)) <= 10,
        "q90 距离 {}",
        hamming(h_base, phash_of_gray(&re90))
    );
    assert!(
        hamming(h_base, phash_of_gray(&re60)) <= 10,
        "q60 距离 {}",
        hamming(h_base, phash_of_gray(&re60))
    );

    // 小平移（±3px）：汉明 ≤ 10
    let shifted = scene_image(0, 3);
    assert!(
        hamming(h_base, phash_of_gray(&shifted)) <= 10,
        "平移距离 {}",
        hamming(h_base, phash_of_gray(&shifted))
    );

    // 不同场景（形状位置/灰度结构大改 + 噪声）：> 20
    let mut other = scene_image(90, 77);
    for (x, y, px) in other.enumerate_pixels_mut() {
        if (x + y) % 7 == 0 {
            *px = image::Luma([px.0[0].wrapping_add(120)]);
        }
    }
    assert!(
        hamming(h_base, phash_of_gray(&other)) > 20,
        "异场景距离 {}",
        hamming(h_base, phash_of_gray(&other))
    );
}

// ---------------------------------------------------------------------------
// 分组引擎
// ---------------------------------------------------------------------------

const SAME: u64 = 0x00ff_00ff_00ff_00ff;
/// 与 SAME 汉明距离 4（≤10 同场景，>2 可被收紧阈值切）。
const SAME_NEAR: u64 = 0x00ff_00ff_00ff_00f0;
/// 与 SAME 汉明距离 32（>10 场景切换）。
const OTHER: u64 = 0xff00_ff00_ff00_ff33;

#[test]
fn grouping_time_and_scene_cuts_with_min_size() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    // 时间链：t=0.0s / 1.5s / 1.9s（同场景）→ 4.5s（同场景但 gap 2.6s 切）→
    // 5.0s（异场景）→ 孤儿 9.0s（同场景，前距 4s 且后无 → 单张不成组）
    let rows = [
        ("a.jpg", "2026-09-21T10:00:00.000Z", SAME),
        ("b.jpg", "2026-09-21T10:00:01.500Z", SAME_NEAR),
        ("c.jpg", "2026-09-21T10:00:01.900Z", SAME),
        ("d.jpg", "2026-09-21T10:00:04.500Z", SAME), // gap 2.6s > 2000ms → 断
        ("e.jpg", "2026-09-21T10:00:05.000Z", OTHER), // gap 0.5s 但场景切 → 断
        ("f.jpg", "2026-09-21T10:00:09.000Z", SAME), // 孤儿
    ];
    for (name, at, hash) in rows {
        db.insert_asset(&asset(&format!("X:/p/{name}"), AssetKind::Photo, Some(at)))
            .unwrap();
        set_phash(&db, &format!("X:/p/{name}"), hash);
    }

    let (groups, photos) = bursts::regroup_bursts(&db, &params()).unwrap();
    assert_eq!(
        (groups, photos),
        (1, 3),
        "只 a/b/c 成组（d 断时间、e 断场景、f 孤儿）"
    );
    let group = burst_of(&db, "X:/p/a.jpg").expect("a 入组");
    assert_eq!(burst_of(&db, "X:/p/b.jpg"), Some(group));
    assert_eq!(burst_of(&db, "X:/p/c.jpg"), Some(group));
    assert_eq!(burst_of(&db, "X:/p/d.jpg"), None, "d 断链");
    assert_eq!(burst_of(&db, "X:/p/e.jpg"), None, "e 场景切换");
    assert_eq!(burst_of(&db, "X:/p/f.jpg"), None, "f 孤儿不成组");
    // bursts 行内容
    let (count, started, ended): (i64, String, String) =
        db.0.query_row(
            "SELECT asset_count, started_at, ended_at FROM bursts WHERE id = ?1",
            [group],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(count, 3);
    assert!(started.starts_with("2026-09-21T10:00:00"));
    assert!(ended.starts_with("2026-09-21T10:00:01.900"));

    // 幂等：重组结果一致（清旧重写不留孤儿组）
    let (groups2, photos2) = bursts::regroup_bursts(&db, &params()).unwrap();
    assert_eq!((groups2, photos2), (1, 3));
    let total: i64 =
        db.0.query_row("SELECT COUNT(*) FROM bursts", [], |r| r.get(0))
            .unwrap();
    assert_eq!(total, 1, "重组清旧组");

    // 参数变更（gap 放宽到 3000ms）：d 加入 a/b/c；e/f 仍在外
    let wide = BurstParams {
        gap_ms: 3000,
        hamming_max: 10,
        min_size: 2,
    };
    let (groups3, photos3) = bursts::regroup_bursts(&db, &wide).unwrap();
    assert_eq!((groups3, photos3), (1, 4), "gap 3s → d 并入");
    assert_eq!(burst_of(&db, "X:/p/d.jpg"), burst_of(&db, "X:/p/a.jpg"));

    // hamming 收紧到 2：b（距离 4）被切
    let strict = BurstParams {
        gap_ms: 3000,
        hamming_max: 2,
        min_size: 2,
    };
    bursts::regroup_bursts(&db, &strict).unwrap();
    // 链在 b 处断：a 孤儿化、b 孤儿化；c/d（同哈希 + gap 2.6s ≤ 3s）仍同组
    assert_eq!(burst_of(&db, "X:/p/b.jpg"), None, "汉明收紧切 b");
    assert_eq!(burst_of(&db, "X:/p/a.jpg"), None, "a 被 b 断链孤儿化");
    assert_eq!(
        burst_of(&db, "X:/p/c.jpg"),
        burst_of(&db, "X:/p/d.jpg"),
        "c/d 仍同组"
    );
    assert!(burst_of(&db, "X:/p/c.jpg").is_some());
}

#[test]
fn grouping_raw_twin_skipped_and_chain_cap() {
    let dir = tempfile::tempdir().unwrap();
    let db = open_db(dir.path());
    // 6 连拍 JPG + 其中 2 张有 RAW 孪生（pair 双向）+ 1 张独立 RAW 无配对（入链）
    for i in 0..6 {
        let at = format!("2026-09-21T10:00:{i:02}.000Z");
        db.insert_asset(&asset(
            &format!("X:/p/{i}.jpg"),
            AssetKind::Photo,
            Some(&at),
        ))
        .unwrap();
        set_phash(&db, &format!("X:/p/{i}.jpg"), SAME);
    }
    for i in [1, 3] {
        let at = format!("2026-09-21T10:00:0{i}.000Z");
        db.insert_asset(&asset(&format!("X:/p/{i}.NEF"), AssetKind::Raw, Some(&at)))
            .unwrap();
        set_phash(&db, &format!("X:/p/{i}.NEF"), SAME);
    }
    // 双向配对
    let id = |p: &str| db.asset_id_by_path(p).unwrap().unwrap();
    for i in [1, 3] {
        db.0.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE path = ?1",
            rusqlite::params![format!("X:/p/{i}.jpg"), id(&format!("X:/p/{i}.NEF"))],
        )
        .unwrap();
        db.0.execute(
            "UPDATE assets SET pair_asset_id = ?2 WHERE path = ?1",
            rusqlite::params![format!("X:/p/{i}.NEF"), id(&format!("X:/p/{i}.jpg"))],
        )
        .unwrap();
    }
    // 独立 RAW（无 pair）
    db.insert_asset(&asset(
        "X:/p/9.NEF",
        AssetKind::Raw,
        Some("2026-09-21T10:00:05.000Z"),
    ))
    .unwrap();
    set_phash(&db, "X:/p/9.NEF", SAME);

    let (groups, photos) = bursts::regroup_bursts(&db, &params()).unwrap();
    assert_eq!(
        (groups, photos),
        (1, 7),
        "6 JPG + 独立 RAW；2 个 RAW 孪生跳链"
    );
    let group = burst_of(&db, "X:/p/0.jpg").unwrap();
    assert_eq!(burst_of(&db, "X:/p/1.NEF"), None, "RAW 孪生不入组");
    assert_eq!(burst_of(&db, "X:/p/3.NEF"), None, "RAW 孪生不入组");
    assert_eq!(burst_of(&db, "X:/p/9.NEF"), Some(group), "独立 RAW 入组");
}

// ---------------------------------------------------------------------------
// IPC：burst_stats + AssetDto burstId/burstCount
// ---------------------------------------------------------------------------

#[test]
fn burst_stats_and_asset_dto_payload() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let state = common::state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let db = open_db(db_dir.path());
    for name in ["a.jpg", "b.jpg", "c.jpg"] {
        db.insert_asset(&asset(
            &format!("X:/p/{name}"),
            AssetKind::Photo,
            Some("2026-09-21T10:00:00.000Z"),
        ))
        .unwrap();
        set_phash(&db, &format!("X:/p/{name}"), SAME);
    }
    // captured_at 相同会破时间序排序？regroup 按 captured_at 排序同值 id 序
    db.0.execute(
        "UPDATE assets SET captured_at = ?2 WHERE path = 'X:/p/b.jpg'",
        ["", "2026-09-21T10:00:01.000Z"],
    )
    .unwrap();
    db.0.execute(
        "UPDATE assets SET captured_at = ?2 WHERE path = 'X:/p/c.jpg'",
        ["", "2026-09-21T10:00:02.000Z"],
    )
    .unwrap();
    bursts::regroup_bursts(&db, &params()).unwrap();

    let stats = ipc::assets::fetch_burst_stats(&state).unwrap();
    assert_eq!(stats.groups, 1);
    assert_eq!(stats.photos_in_bursts, 3);
    let json = serde_json::to_value(stats).unwrap();
    assert_eq!(json["groups"], 1);
    assert_eq!(json["photosInBursts"], 3);

    // AssetDto burstId + burstCount（页内批量装配，免 N+1）
    let page = ipc::assets::fetch_assets_page(&state, 0, 10, db::AssetFilters::default()).unwrap();
    assert_eq!(page.len(), 3);
    let group = page[0].burst_id.expect("入组");
    for dto in &page {
        assert_eq!(dto.burst_id, Some(group));
        assert_eq!(dto.burst_count, Some(3));
    }
    let json = serde_json::to_value(&page[0]).unwrap();
    assert_eq!(json["burstId"], group);
    assert_eq!(json["burstCount"], 3);
}

// ---------------------------------------------------------------------------
// phash 通道：回填 + 重组触发（启动代际路径 E2E）
// ---------------------------------------------------------------------------

#[test]
fn phash_backfill_and_regroup_via_generation_hook() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    // 真实 JPEG × 2（同场景，1s 间隔——captured 无 EXIF 由 created_at 无法入链？
    // 分组用 captured_at；无 EXIF 时为 NULL → 断链。补 EXIF 时间戳或直接写列）
    let photo = |name: &str| dir.path().join(name);
    let mut img = scene_image(0, 0);
    let mut buf = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
    image::DynamicImage::ImageLuma8(img.clone())
        .write_with_encoder(encoder)
        .unwrap();
    std::fs::write(photo("one.jpg"), &buf).unwrap();
    img = scene_image(0, 1);
    let mut buf2 = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf2, 90);
    image::DynamicImage::ImageLuma8(img)
        .write_with_encoder(encoder)
        .unwrap();
    std::fs::write(photo("two.jpg"), &buf2).unwrap();

    let db = open_db(&db_dir);
    db.insert_asset(&asset(
        &photo("one.jpg").to_string_lossy(),
        AssetKind::Photo,
        Some("2026-09-21T10:00:00.000Z"),
    ))
    .unwrap();
    db.insert_asset(&asset(
        &photo("two.jpg").to_string_lossy(),
        AssetKind::Photo,
        Some("2026-09-21T10:00:01.000Z"),
    ))
    .unwrap();

    // 建任务 + worker 消费（process_phash_task → 256 档缩略图 → pHash）
    assert_eq!(db.create_phash_tasks_for_unindexed().unwrap(), 2);
    assert_eq!(db.create_phash_tasks_for_unindexed().unwrap(), 0, "去重");
    index::run_pending(&db_dir, 2);
    let phash_null: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets WHERE phash IS NULL", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(phash_null, 0, "pHash 全部落库");
    let (h1, h2): (i64, i64) =
        db.0.query_row(
            "SELECT (SELECT phash FROM assets WHERE path LIKE '%one.jpg'), \
             (SELECT phash FROM assets WHERE path LIKE '%two.jpg')",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert!(
        hamming(h1 as u64, h2 as u64) <= 10,
        "同场景重压缩距离 {}",
        hamming(h1 as u64, h2 as u64)
    );

    // 重组：两图成组
    let (groups, photos) = bursts::regroup_bursts(&db, &params()).unwrap();
    assert_eq!((groups, photos), (1, 2));

    // 代际钩子：marker 落盘后二次调用短路
    let bus = events::EventBus::new();
    let supervisor = tasks::TaskSupervisor::new(bus.clone());
    index::refresh_phash_for_generation(db_dir.clone(), &bus, &supervisor, params());
    let marker = db_dir.join("phash-gen-1.marker");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !marker.is_file() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    index::refresh_phash_for_generation(db_dir.clone(), &bus, &supervisor, params());
    // 已算完的 phash 不重算：requeue 幂等（任务账不新增）
    let tasks: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'phash'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tasks, 2, "每资产一条 phash 任务（不重复建）");
}

// ---------------------------------------------------------------------------
// 参数指纹：burst= 路（只重组不重算 pHash）
// ---------------------------------------------------------------------------

#[test]
fn burst_fingerprint_change_triggers_regroup_only() {
    let src = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    std::fs::create_dir_all(&db_dir).unwrap();
    let state = common::state_with_library(&db_dir, src.path(), Duration::from_millis(1));
    let db = open_db(&db_dir);
    for (name, at) in [
        ("a.jpg", "2026-09-21T10:00:00.000Z"),
        ("b.jpg", "2026-09-21T10:00:01.000Z"),
        ("c.jpg", "2026-09-21T10:00:08.000Z"),
    ] {
        db.insert_asset(&asset(&format!("X:/p/{name}"), AssetKind::Photo, Some(at)))
            .unwrap();
        set_phash(&db, &format!("X:/p/{name}"), SAME);
    }
    // 写 marker（含旧 burst 指纹 = gap 2000 形态的分组已完成）
    let ai_old = AiSettings::default();
    let marker = db_dir.join("index-params.marker");
    std::fs::write(
        &marker,
        format!(
            "semantic={}\nface={}\nburst={}\n",
            ipc::indexing::semantic_params_fingerprint(&ai_old),
            ipc::indexing::face_params_fingerprint(&ai_old),
            ipc::indexing::burst_params_fingerprint(&ai_old),
        ),
    )
    .unwrap();

    // 改参数（gap 放宽到 8000ms → c 并入 a/b）→ check 触发 regroup
    let ai_new = AiSettings {
        burst_gap_ms: 8000,
        ..AiSettings::default()
    };
    assert_ne!(
        ipc::indexing::burst_params_fingerprint(&ai_old),
        ipc::indexing::burst_params_fingerprint(&ai_new),
    );
    ipc::indexing::check_params_and_rebuild(&state, &db_dir, &ai_new);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while burst_of(&db, "X:/p/c.jpg").is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "参数变更应触发重组并入 c"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    // marker 更新为新 burst 指纹；phash 未被动过（无重算）
    let content = std::fs::read_to_string(&marker).unwrap();
    assert!(content.contains(&format!(
        "burst={}",
        ipc::indexing::burst_params_fingerprint(&ai_new)
    )));
    let phash_rows: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM assets WHERE phash IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(phash_rows, 3, "pHash 保留（只重组不重算）");
    let phash_tasks: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE kind = 'phash'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(phash_tasks, 0, "未建重算任务");
}
