//! reference 导入（只登记不搬文件，2026-10-09 §三新语义）：源树原样保留、
//! 库 root 零物理足迹；资产 origin=external、library_id=目标照片库（同库
//! 去重第二次全跳过）；同名 XMP 边车星级/颜色/关键字随登记读入（不搬边车）；
//! §八-1 导入侧：源目录改名后重新 reference，missing 同哈希原位重绑。

mod common;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan,
    settings, tasks, tethering, thumbs,
};

use std::fs;

use common::open_db;
use import::engine::ImportMode;

#[test]
fn reference_import_registers_in_place_with_library_ownership_and_no_footprint() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let database = temp.path().join("database");
    let photos = temp.path().join("photos");
    let originals = common::build_source(&source);
    fs::create_dir_all(&database).unwrap();
    fs::create_dir_all(&photos).unwrap();

    // 一张同名边车带星级 + 颜色（登记时应读入；边车本体不动）
    fs::write(
        source.join("DCIM/100CANON/IMG_0001.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4" xmp:Label="Red"/>
        </rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();

    let (_, stats) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(stats.done_files, originals.len() as u64);
    assert_eq!(stats.failed_files, 0);

    // 目标照片库 root 完全无物理足迹（reference 不搬文件）
    assert_eq!(fs::read_dir(&photos).unwrap().count(), 0);

    let db = open_db(&database);
    let library_id = db.photos_library_list().unwrap()[0].id.clone();
    let mut rows = db
        .0
        .prepare("SELECT path, origin, library_id, rating, color_label FROM assets ORDER BY path")
        .unwrap();
    let recorded = rows
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(recorded.len(), originals.len());
    for (path, origin, library, _rating, _label) in &recorded {
        assert_eq!(origin, "external");
        assert!(path.starts_with(source.to_str().unwrap()));
        // 归属目标照片库（§一：library_id 静态归属属性）
        assert_eq!(library.as_deref(), Some(library_id.as_str()));
    }
    // 边车读入：IMG_0001 拿到 4 星 + red；无边车的行为 0/None
    let with_sidecar = recorded
        .iter()
        .find(|(p, ..)| p.ends_with("IMG_0001.jpg"))
        .unwrap();
    assert_eq!(with_sidecar.3, 4, "边车星级应随登记读入");
    // 颜色标签取 XMP 标准色名形态（与 index 通道晚到边车读入同口径）
    assert_eq!(with_sidecar.4.as_deref(), Some("Red"), "边车颜色标签应读入");

    // 源树原样保留（本体 + 边车，无任何缓存/副本）
    for (relative, content) in &originals {
        assert_eq!(fs::read(source.join(relative)).unwrap(), *content);
    }
    assert!(source.join("DCIM/100CANON/IMG_0001.xmp").is_file());
    let source_files = walkdir::WalkDir::new(&source)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count();
    assert_eq!(source_files, originals.len() + 1, "本体 + 1 个边车");

    // 同库查重：第二次 reference 全部跳过（同路径/同指纹）
    let (_, again) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(again.done_files, 0);
    assert_eq!(again.skipped_duplicates, originals.len() as u64);
    assert_eq!(common::count_assets(&open_db(&database)), originals.len() as i64);
}

/// §三/§六 关键字读入 + §八-1 导入侧重绑：LR 目录整体改名后重新
/// reference——missing 同哈希原位接管既有资产行（路径重绑、清 missing、
/// 逻辑元数据保留、DB 真值投影补写边车新位置），不产生第二行；关键字
/// （dc:subject）随登记落 asset_metadata（与 album_export 导出读入同一
/// 存储）。
#[test]
fn reference_reimport_after_folder_rename_rebinds_and_reads_keywords() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("database");
    let photos = temp.path().join("photos");
    fs::create_dir_all(&database).unwrap();
    fs::create_dir_all(&photos).unwrap();

    let source = temp.path().join("lr-a");
    let originals = common::build_source(&source);
    // 边车：星级 + 颜色 + 关键字（LR 原生 dc:subject/rdf:Bag 形态）
    fs::write(
        source.join("DCIM/100CANON/IMG_0001.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="4" xmp:Label="Red">
         <dc:subject><rdf:Bag><rdf:li>旅行</rdf:li><rdf:li>wedding</rdf:li></rdf:Bag></dc:subject>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();

    let (_, first) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(first.done_files, originals.len() as u64);

    let db = open_db(&database);
    let (id, _path): (i64, String) = db
        .0
        .query_row(
            "SELECT id, path FROM assets WHERE filename = 'IMG_0001.jpg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    // 关键字已随登记落 asset_metadata（读入方向，LR → Photo Hub）
    let keywords = asset_keywords(&db, id);
    assert_eq!(keywords, vec!["旅行".to_string(), "wedding".to_string()]);

    // LR 目录整体改名（源路径全变）：旧行按旧路径缺失；离线期评分改动
    db.0
        .execute(
            "UPDATE assets SET missing = 1, rating = 5 WHERE id = ?1",
            [id],
        )
        .unwrap();
    let renamed = temp.path().join("lr-b");
    fs::rename(&source, &renamed).unwrap();

    let (_, second) = common::run_engine(&renamed, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    // IMG_0001 重绑归位；其余两张在线同哈希照旧 skip
    assert_eq!(second.done_files, 1);
    assert_eq!(second.skipped_duplicates, (originals.len() - 1) as u64);
    assert_eq!(
        common::count_assets(&db),
        originals.len() as i64,
        "重绑不产生第二行"
    );

    let (path, missing, rating): (String, i64, i64) = db
        .0
        .query_row(
            "SELECT path, missing, rating FROM assets WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(missing, 0, "重绑清 missing");
    assert_eq!(rating, 5, "逻辑元数据（评分）保留");
    assert!(
        path.starts_with(renamed.to_str().unwrap()),
        "路径重绑到新源目录：{path}"
    );
    // 边车补写新位置（§八-1）：DB 真值（5 星）投影到新位置边车
    let sidecar = renamed.join("DCIM/100CANON/IMG_0001.xmp");
    let text = fs::read_to_string(&sidecar).unwrap();
    assert_eq!(metadata::xmp::sidecar_rating(&text), Some(5));
    // 关键字不被重绑清掉（asset_metadata 与资产行无关，原样保留）
    assert_eq!(asset_keywords(&db, id).len(), 2);
}

/// asset_metadata.value JSON 的 keywords 数组（无记录/损坏 → 空）。
fn asset_keywords(db: &db::Db, asset_id: i64) -> Vec<String> {
    #[derive(serde::Deserialize)]
    struct KeywordsOnly {
        #[serde(default)]
        keywords: Vec<String>,
    }
    db.0
        .query_row(
            "SELECT value FROM asset_metadata WHERE asset_id = ?1",
            [asset_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|text| serde_json::from_str::<KeywordsOnly>(&text).ok())
        .map(|parsed| parsed.keywords)
        .unwrap_or_default()
}

/// §八-1 同路径恢复：外部盘拔插后重新 reference 同一文件夹——路径全中、
/// 行被惰性检测（访问时通道，§五）标 missing、内容一致 → 恢复清 missing，
/// 不产生第二行。reference 资产路径在库 root 之外，库扫描永不踏足，此处
/// 是唯一恢复通道——skip 会把照片永久锁在画廊缺失态。
#[test]
fn reference_reimport_same_path_recovers_missing_rows() {
    let temp = tempfile::tempdir().unwrap();
    let database = temp.path().join("database");
    let photos = temp.path().join("photos");
    fs::create_dir_all(&database).unwrap();
    fs::create_dir_all(&photos).unwrap();
    let source = temp.path().join("source");
    let originals = common::build_source(&source);

    let (_, first) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(first.done_files, originals.len() as u64);

    // 盘拔掉期间画廊被浏览 → 惰性检测标 missing（§五 第三通道）
    let db = open_db(&database);
    db.0
        .execute("UPDATE assets SET missing = 1", [])
        .unwrap();

    // 盘重插（文件原样回来），重新 reference 同一文件夹
    let (_, second) = common::run_engine(&source, &database, &photos, |plan| {
        plan.mode = ImportMode::Reference;
    });
    assert_eq!(second.done_files, 0, "同路径不重复登记");
    assert_eq!(second.skipped_duplicates, originals.len() as u64);
    let still_missing: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE missing = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(still_missing, 0, "内容一致 → 全部恢复清 missing");
    assert_eq!(common::count_assets(&db), originals.len() as i64);
}
