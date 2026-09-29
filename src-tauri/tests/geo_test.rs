//! 拍摄地图管线集成测试（树形地区索引，docs/plans/map-module.md）：
//! 树加载入库 → 增量回填 → 分层聚合/下钻 → 编辑联动重索引 → 树缓存导出。
//! fixture 与 geo::tests 同构（内嵌小 GeoJSON，不依赖网络/真实数据包）。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, migrate, platform,
    settings, tasks, thumbs,
};

use std::io::Write;
use std::path::Path;

use common::open_db;
use db::AssetRow;
use events::{AssetKind, EventBus};
use geo::backfill;

fn asset(path: &str, lat: Option<f64>, lon: Option<f64>) -> AssetRow {
    AssetRow {
        path: path.into(),
        filename: path.rsplit(['/', '\\']).next().unwrap_or(path).into(),
        size: 100,
        mtime: "2026-09-29T00:00:00.000Z".into(),
        xxhash: 1,
        kind: AssetKind::Photo,
        captured_at: Some("2026-09-01T10:00:00.000Z".into()),
        camera: Some("Sony A7M4".into()),
        source: "imported".into(),
        created_at: "2026-09-29T00:00:00.000Z".into(),
        origin: "imported".into(),
        width: Some(6000),
        height: Some(4000),
        iso: Some(400),
        f_number: Some("2.8".into()),
        exposure_time: Some("1/250".into()),
        focal_length: Some("50".into()),
        lens: None,
        pair_asset_id: None,
        thumb_state: 1,
        orientation: Some(1),
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: lat,
        gps_lon: lon,
        rating: 0,
        flagged: 0,
        color_label: None,
        rejected: 0,
    }
}

/// 与 src/geo/mod.rs 单测同构的包目录：测试国/远方国/中国 + 北省 + 京省/州市/甲区。
fn write_geo_fixture(dir: &Path) {
    let adm0 = r#"{"features":[
        {"properties":{"ADMIN":"Testland","NAME_ZH":"测试国","ADM0_A3":"TST","LABEL_Y":5.0,"LABEL_X":5.0},
         "geometry":{"type":"Polygon","coordinates":[[[0,0],[10,0],[10,10],[0,10],[0,0]]]}},
        {"properties":{"ADMIN":"Farland","NAME_ZH":"远方国","ADM0_A3":"FAR","LABEL_Y":-20.0,"LABEL_X":50.0},
         "geometry":{"type":"Polygon","coordinates":[[[45,-25],[55,-25],[55,-15],[45,-15],[45,-25]]]}},
        {"properties":{"ADMIN":"China","NAME_ZH":"中国","ADM0_A3":"CHN","LABEL_Y":30.0,"LABEL_X":104.0},
         "geometry":{"type":"Polygon","coordinates":[[[100,20],[110,20],[110,40],[100,40],[100,20]]]}}
    ]}"#;
    let adm1 = r#"{"features":[
        {"properties":{"name_zh":"北省","name":"North","adm0_a3":"TST","iso_3166_2":"TST-N"},
         "geometry":{"type":"Polygon","coordinates":[[[0,5],[10,5],[10,10],[0,10],[0,5]]]}},
        {"properties":{"name_zh":"旧省","name":"Old","adm0_a3":"CHN","iso_3166_2":"CN-OLD"},
         "geometry":{"type":"Polygon","coordinates":[[[100,36],[104,36],[104,39],[100,39],[100,36]]]}}
    ]}"#;
    let cn_root = r#"{"features":[
        {"properties":{"adcode":"110000","name":"京省","center":[104.0,30.0]},
         "geometry":{"type":"Polygon","coordinates":[[[100,25],[108,25],[108,35],[100,35],[100,25]]]}}
    ]}"#;
    let cn_prov = r#"{"features":[
        {"properties":{"adcode":"110100","name":"州市","center":[102.0,30.0]},
         "geometry":{"type":"Polygon","coordinates":[[[100,25],[104,25],[104,30],[100,30],[100,25]]]}}
    ]}"#;
    let cn_city = r#"{"features":[
        {"properties":{"adcode":"110101","name":"甲区","center":[101.0,27.0]},
         "geometry":{"type":"Polygon","coordinates":[[[100,25],[102,25],[102,27],[100,27],[100,25]]]}}
    ]}"#;
    for (name, body) in [
        ("world-adm0.geojson", adm0),
        ("world-adm1.geojson", adm1),
        ("datav-100000.json", cn_root),
        ("datav-110000.json", cn_prov),
        ("datav-110100.json", cn_city),
    ] {
        let mut f = std::fs::File::create(dir.join(name)).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }
}

#[test]
fn pipeline_backfill_clusters_drilldown_reindex_and_cache() {
    let root = tempfile::tempdir().unwrap();
    let db_dir = root.path().join("lib");
    let config_dir = root.path().join("config");
    std::fs::create_dir_all(&db_dir).unwrap();
    let geo_dir = geo::geo_dir(&config_dir);
    std::fs::create_dir_all(&geo_dir).unwrap();
    write_geo_fixture(&geo_dir);

    let db = open_db(&db_dir);
    // 3 张甲区 + 2 张州市非甲区 + 1 张测试国北省 + 1 张无 GPS + 1 张海上
    for (path, lat, lon) in [
        ("X:/p/j1.jpg", Some(26.0), Some(101.0)),
        ("X:/p/j2.jpg", Some(26.5), Some(101.5)),
        ("X:/p/j3.jpg", Some(25.5), Some(100.5)),
        ("X:/p/z1.jpg", Some(28.0), Some(103.0)),
        ("X:/p/z2.jpg", Some(29.0), Some(102.0)),
        ("X:/p/n1.jpg", Some(7.0), Some(5.0)),
        ("X:/p/nogps.jpg", None, None),
        ("X:/p/sea.jpg", Some(0.0), Some(-140.0)),
    ] {
        db.insert_asset(&asset(path, lat, lon)).unwrap();
    }

    let bus = EventBus::new();
    backfill::run_pipeline(&db_dir, &config_dir, &bus, &|| false);

    // —— 挂接断言：甲区 4 行/张（国省市县）、北省 2 行、海上 0 行 ——
    let count = |sql: &str| -> i64 { db.0.query_row(sql, [], |r| r.get(0)).unwrap() };
    assert_eq!(
        count("SELECT COUNT(*) FROM regions WHERE name='旧省'"),
        0,
        "DataV 替换中国子树后，NE 旧省是 Arena 孤儿：不入库"
    );
    assert_eq!(
        count("SELECT COUNT(*) FROM asset_regions"),
        3 * 4 + 2 * 3 + 2,
        "甲区3张×4层 + 州市2张×3层（无县） + 北省1张×2层"
    );
    let j1: i64 =
        db.0.query_row("SELECT id FROM assets WHERE path='X:/p/j1.jpg'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM asset_regions WHERE asset_id={j1} AND level=3"
        )),
        1,
        "甲区照片应挂到县级行"
    );

    // —— 聚合：国家层 ——
    let national = ipc::map::clusters(&db, 0, None).unwrap();
    assert_eq!(national.len(), 2, "中国 + 测试国");
    let china = national.iter().find(|c| c.name == "中国").unwrap();
    assert_eq!(china.count, 5);
    assert!(china.samples.len() <= 3, "样本上限 3");
    for s in &china.samples {
        assert!(s.path.starts_with("X:/p/") || s.path.starts_with("X:/p/"));
    }
    let testland = national.iter().find(|c| c.name == "测试国").unwrap();
    assert_eq!(testland.count, 1);

    // —— 下钻：中国 → 京省(5) → 州市(5) → 甲区(3)；parent 外不串组 ——
    let provinces = ipc::map::clusters(&db, 1, Some(china.region_id)).unwrap();
    assert_eq!(provinces.len(), 1);
    assert_eq!(provinces[0].name, "京省");
    assert_eq!(provinces[0].count, 5);
    let cities = ipc::map::clusters(&db, 2, Some(provinces[0].region_id)).unwrap();
    assert_eq!(cities.len(), 1);
    assert_eq!(cities[0].name, "州市");
    let districts = ipc::map::clusters(&db, 3, Some(cities[0].region_id)).unwrap();
    assert_eq!(districts.len(), 1);
    assert_eq!(districts[0].name, "甲区");
    assert_eq!(districts[0].count, 3);
    // parent 限定：测试国下钻省级不应看到京省
    let tst_provinces = ipc::map::clusters(&db, 1, Some(testland.region_id)).unwrap();
    assert_eq!(tst_provinces.len(), 1);
    assert_eq!(tst_provinces[0].name, "北省");

    // —— 层级越界拒绝 ——
    assert!(ipc::map::clusters(&db, 4, None).is_err());

    // —— 编辑联动：甲区一张改到测试国南半部（仅国家级）——
    // 真实链路 save() 会同步 UPDATE assets.gps；测试同步改，二次回填才不误捞
    db.0.execute(
        "UPDATE assets SET gps_lat=2.0, gps_lon=5.0 WHERE id=?1",
        [&j1],
    )
    .unwrap();
    assert!(backfill::reindex_asset(&db, j1, Some(2.0), Some(5.0)));
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM asset_regions WHERE asset_id={j1}"
        )),
        1,
        "南半部无省：只剩国家级挂接"
    );
    let china_after = ipc::map::clusters(&db, 0, None).unwrap();
    let china_after = china_after.iter().find(|c| c.name == "中国").unwrap();
    assert_eq!(china_after.count, 4, "j1 迁走后中国 4 张");
    // 清除 GPS：挂接全删
    db.0.execute(
        "UPDATE assets SET gps_lat=NULL, gps_lon=NULL WHERE id=?1",
        [&j1],
    )
    .unwrap();
    assert!(backfill::reindex_asset(&db, j1, None, None));
    assert_eq!(
        count(&format!(
            "SELECT COUNT(*) FROM asset_regions WHERE asset_id={j1}"
        )),
        0
    );

    // —— 树缓存：文件存在，行数与 regions 一致 ——
    let cache_path = geo_dir.join(backfill::CACHE_FILE);
    assert!(cache_path.is_file(), "缓存应导出");
    let regions_count = count("SELECT COUNT(*) FROM regions");
    let raw = std::fs::read_to_string(&cache_path).unwrap();
    let rows: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
    assert_eq!(rows.len() as i64, regions_count);
    assert!(
        rows.iter()
            .all(|r| r.get("name").is_some() && r.get("lat").is_some()),
        "缓存行含展示字段"
    );

    // —— 二次管线：幂等（同构跳过重刷），新资产增量补 ——
    db.insert_asset(&asset("X:/p/j4.jpg", Some(26.2), Some(101.2)))
        .unwrap();
    backfill::run_pipeline(&db_dir, &config_dir, &bus, &|| false);
    assert_eq!(
        count("SELECT COUNT(*) FROM asset_regions"),
        3 * 4 + 2 * 3 + 2 + 4 - 4,
        "j4 补 4 行；j1 已被清除为 0"
    );
    // 指纹未变 + 行数同构：regions 未被重刷（id 稳定）
    let china_id_stable =
        db.0.query_row(
            "SELECT id FROM regions WHERE name='中国' AND level=0",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(china_id_stable, china.region_id, "同构跳过：id 稳定");
}

/// 内置数据包端到端：解压真实嵌入 zip（400 文件）→ 树可加载。
/// 同时守住两个历史事故路径：台湾 710000（无 _full，包内为轮廓文件）与
/// 济源市 419001（自引用轮廓，曾致递归栈溢出——现无递归）。
#[test]
fn embedded_geo_package_extracts_and_loads() {
    let dir = tempfile::tempdir().unwrap();
    let geo_dir = dir.path().join("geo");
    let count = geo::install::extract_embedded(&geo_dir).unwrap();
    assert!(count >= 390, "嵌入包应基本齐备：{count}");
    assert!(geo_dir.join("world-adm0.geojson").is_file());
    assert!(geo_dir.join("world-adm1.geojson").is_file());
    assert!(geo_dir.join("datav-710000.json").is_file(), "台湾轮廓应在包内");
    assert!(geo_dir.join("datav-419001.json").is_file(), "济源市应在包内");
    let index = geo::GeoIndex::load(&geo_dir).expect("嵌入包解压后树应可加载");
    assert!(index.nodes.len() > 300, "地区节点应成规模：{}", index.nodes.len());
    // 幂等：重复解压不报错不缺文件
    assert_eq!(geo::install::extract_embedded(&geo_dir).unwrap(), count);
}
