//! 阶段 D 基础编辑与导出（0022）：配方存取/校验夹取、folder 导出（长边
//! 只缩不放 + JPEG 可解码 + GPS 剥离 + EXIF 拍摄时间保留 + 目标存在报错）、
//! 旋转后裁剪坐标、album 导出落位（{root}/{创建YYYY}/{创建MM}/{dir_name}/
//! [{子组}/]——与导入/claim/移组同公式 album_item_home_rel；0022 子组物理化
//! + subgroup + 重名后缀）、
//! 文字/笔迹渲染冒烟、IPC 后台任务链路。

mod common;

pub use common::{
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

#[path = "../src/edit/mod.rs"]
pub mod edit;

use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use db::AssetRow;
use events::{AppEvent, AssetKind, EventBus};
use exif::experimental::Writer as ExifWriter;
use exif::{Field, In, Tag, Value};
use img_parts::jpeg::{Jpeg, JpegSegment};
use img_parts::Bytes;

// ---------------------------------------------------------------------------
// 脚手架
// ---------------------------------------------------------------------------

struct Fixture {
    _dir: tempfile::TempDir,
    state: ipc::AppState,
    db: db::Db,
    photo_root: PathBuf,
}

fn setup() -> Fixture {
    let (dir, state, database) = common::library_fixture();
    let photo_root = dir.path().join("photos");
    Fixture {
        _dir: dir,
        state,
        db: database,
        photo_root,
    }
}

/// EXIF TIFF 流（kamadak-exif Writer）：拍摄时间 + 相机 + GPS（北纬 31 度）。
fn build_exif_tiff() -> Vec<u8> {
    let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
    let fields = vec![
        Field {
            tag: Tag::Make,
            ifd_num: In::PRIMARY,
            value: ascii("Sony"),
        },
        Field {
            tag: Tag::Model,
            ifd_num: In::PRIMARY,
            value: ascii("ILCE-7RM5"),
        },
        Field {
            tag: Tag::DateTimeOriginal,
            ifd_num: In::PRIMARY,
            value: ascii("2026:06:28 15:30:00"),
        },
        Field {
            tag: Tag::GPSLatitudeRef,
            ifd_num: In::PRIMARY,
            value: ascii("N"),
        },
        Field {
            tag: Tag::GPSLatitude,
            ifd_num: In::PRIMARY,
            value: Value::Rational(vec![exif::Rational { num: 31, denom: 1 }]),
        },
    ];
    let mut writer = ExifWriter::new();
    for field in &fields {
        writer.push_field(field);
    }
    let mut tiff = Vec::new();
    writer.write(&mut Cursor::new(&mut tiff), true).unwrap();
    tiff
}

/// 源 JPEG：image crate 编码的渐变图 + EXIF APP1（拍摄时间/相机/GPS）。
fn build_source_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x * 255 / width.max(1)) as u8, (y * 255 / height.max(1)) as u8, 128])
    });
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95);
    img.write_with_encoder(encoder).unwrap();
    let mut parsed = Jpeg::from_bytes(Bytes::from(jpeg)).unwrap();
    let mut contents = Vec::new();
    contents.extend_from_slice(b"Exif\0\0");
    contents.extend_from_slice(&build_exif_tiff());
    parsed
        .segments_mut()
        .insert(1, JpegSegment::new_with_contents(0xE1, Bytes::from(contents)));
    let mut out = Vec::new();
    parsed.encoder().write_to(&mut Cursor::new(&mut out)).unwrap();
    out
}

/// 双色源 JPEG（左半红 / 右半蓝）：旋转+裁剪的像素级断言用。
fn build_bicolor_jpeg(width: u32, height: u32) -> Vec<u8> {
    let img = image::RgbImage::from_fn(width, height, |x, _y| {
        if x < width / 2 {
            image::Rgb([200, 10, 10])
        } else {
            image::Rgb([10, 10, 200])
        }
    });
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 95);
    img.write_with_encoder(encoder).unwrap();
    jpeg
}

/// 建源资产（落盘 + 入库），返回资产 id。
fn ins_photo(db: &db::Db, dir: &Path, name: &str, bytes: &[u8], captured: Option<&str>) -> i64 {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    db.insert_asset(&AssetRow {
        path: path.to_string_lossy().into_owned(),
        filename: name.to_string(),
        size: bytes.len() as u64,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: bytes.len() as u64,
        kind: AssetKind::Photo,
        captured_at: captured.map(str::to_string),
        camera: Some("Sony ILCE-7RM5".to_string()),
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
    db.asset_id_by_path(&path.to_string_lossy()).unwrap().unwrap()
}

fn folder_options(dir: &Path, name: &str) -> edit::export::ExportOptions {
    edit::export::ExportOptions {
        mode: edit::export::ExportMode::Folder,
        folder: Some(edit::export::ExportFolderTarget {
            output_dir: dir.to_string_lossy().into_owned(),
            file_name: name.to_string(),
        }),
        album: None,
        long_edge: None,
        quality: None,
        remove_gps: false,
        copyright: None,
        author: None,
        keywords: Vec::new(),
    }
}

fn album_options(album_id: i64, subgroup: Option<&str>) -> edit::export::ExportOptions {
    edit::export::ExportOptions {
        mode: edit::export::ExportMode::Album,
        folder: None,
        album: Some(edit::export::ExportAlbumTarget {
            album_id: album_id.to_string(),
            subgroup: subgroup.map(str::to_string),
        }),
        long_edge: None,
        quality: None,
        remove_gps: false,
        copyright: None,
        author: None,
        keywords: Vec::new(),
    }
}

/// 同步跑一个导出任务（绕过 supervisor，直调执行核）。
fn run_job(
    db_dir: &Path,
    asset_id: i64,
    recipe: &edit::recipe::EditRecipe,
    options: &edit::export::ExportOptions,
    photo_root: &Path,
) -> db::ExportJobRow {
    let database = common::open_db(db_dir);
    let validated = edit::export::validate_options(&database, options).unwrap();
    let job_id = database
        .export_job_create(asset_id, validated.mode.as_str())
        .unwrap();
    let request = edit::export::ExportJobRequest {
        job_id,
        asset_id,
        asset: database.asset_by_id(asset_id).unwrap().unwrap(),
        recipe: recipe.clone(),
        options: validated,
        photo_root: photo_root.to_path_buf(),
    };
    let bus = EventBus::new();
    edit::export::run_export_job(database, &bus, request);
    let row = common::open_db(db_dir).export_job_get(job_id).unwrap().unwrap();
    assert_eq!(row.status, "done", "导出应成功: {:?}", row.error);
    row
}

fn plain_recipe() -> edit::recipe::EditRecipe {
    edit::recipe::parse_recipe(&serde_json::json!({ "version": 1 })).unwrap()
}

fn read_exif(bytes: &[u8]) -> exif::Exif {
    exif::Reader::new()
        .read_from_container(&mut Cursor::new(bytes))
        .unwrap()
}

// ---------------------------------------------------------------------------
// 配方存取 / 校验夹取
// ---------------------------------------------------------------------------

#[test]
fn recipe_save_get_delete_with_clamping() {
    let fixture = setup();
    let src = build_source_jpeg(64, 48);
    let id = ins_photo(&fixture.db, &fixture.photo_root, "DSC_0001.jpg", &src, None);

    // 无配方 → null/null
    let empty = edit::ipc::fetch_edit_recipe(&fixture.db, id).unwrap();
    assert!(empty.recipe.is_none() && empty.updated_at.is_none());

    // 保存（越界值被夹取，回显归一化配方）
    let saved = edit::ipc::fetch_edit_recipe_save(
        &fixture.db,
        id,
        &serde_json::json!({
            "version": 1,
            "rotateQuarter": 3,
            "crop": { "x": -0.2, "y": 0.1, "w": 1.5, "h": 0.4 },
            "textLayers": [{ "id": "t1", "x": 1.2, "y": 0.2, "text": "a\nb",
                             "sizeRel": 0.05, "color": "#FFFF00" }],
            "brushStrokes": [{ "id": "b1", "color": "#FF3333", "widthRel": 0.008,
                               "points": [{ "x": 0.1, "y": 0.2 }, { "x": 0.3, "y": 2.0 }] }],
            "output": { "longEdge": 2560, "quality": 95 }
        }),
    )
    .unwrap();
    assert!(saved.updated_at.is_some());
    let recipe = saved.recipe.unwrap();
    assert_eq!(recipe["crop"]["x"], 0.0);
    assert_eq!(recipe["crop"]["w"], 1.0);
    assert_eq!(recipe["textLayers"][0]["x"], 1.0);
    assert_eq!(recipe["brushStrokes"][0]["points"][1]["y"], 1.0);

    // 读回一致（JSON 文本落库 → 同一归一化形状）
    let got = edit::ipc::fetch_edit_recipe(&fixture.db, id).unwrap();
    assert_eq!(got.recipe.unwrap(), recipe);
    assert!(got.updated_at.is_some());

    // 非法配方与非法资产
    assert!(edit::ipc::fetch_edit_recipe_save(
        &fixture.db,
        id,
        &serde_json::json!({ "version": 2 })
    )
    .is_err());
    assert!(edit::ipc::fetch_edit_recipe_save(
        &fixture.db,
        id,
        &serde_json::json!({ "version": 1, "rotateQuarter": 9 })
    )
    .is_err());
    assert!(
        edit::ipc::fetch_edit_recipe_save(&fixture.db, 999, &serde_json::json!({ "version": 1 }))
            .is_err()
    );

    // 删除 → 幂等回到空态
    edit::ipc::fetch_edit_recipe_delete(&fixture.db, id).unwrap();
    edit::ipc::fetch_edit_recipe_delete(&fixture.db, id).unwrap();
    let after = edit::ipc::fetch_edit_recipe(&fixture.db, id).unwrap();
    assert!(after.recipe.is_none() && after.updated_at.is_none());
}

// ---------------------------------------------------------------------------
// folder 模式：尺寸 / 元数据
// ---------------------------------------------------------------------------

#[test]
fn folder_export_dimensions_and_metadata() {
    let fixture = setup();
    let out_dir = fixture._dir.path().join("exports");
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0001.jpg",
        &build_source_jpeg(200, 120),
        Some("2026-06-01T10:00:00.000Z"),
    );

    // 长边缩放（200→100 长边）+ 只缩不放（请求 1000 > 源 200 → 原尺寸）
    let mut options = folder_options(&out_dir, "a.jpg");
    options.long_edge = Some(100);
    let row = run_job(&fixture._dir.path().join("db"), id, &plain_recipe(), &options, &fixture.photo_root);
    let bytes = std::fs::read(&row.output_path.clone().unwrap()).unwrap();
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (100, 60));
    assert_eq!(row.width.unwrap(), 100);
    assert_eq!(row.height.unwrap(), 60);
    assert_eq!(row.bytes.unwrap(), bytes.len() as u64);

    let mut options = folder_options(&out_dir, "b.jpg");
    options.long_edge = Some(1000);
    let row = run_job(&fixture._dir.path().join("db"), id, &plain_recipe(), &options, &fixture.photo_root);
    let bytes = std::fs::read(&row.output_path.unwrap()).unwrap();
    let img = image::load_from_memory(&bytes).unwrap();
    assert_eq!((img.width(), img.height()), (200, 120), "只缩不放");

    // GPS 保留（removeGps=false）
    let options = folder_options(&out_dir, "keep.jpg");
    let row = run_job(&fixture._dir.path().join("db"), id, &plain_recipe(), &options, &fixture.photo_root);
    let bytes = std::fs::read(&row.output_path.unwrap()).unwrap();
    let exif = read_exif(&bytes);
    assert!(
        exif.get_field(Tag::GPSLatitude, In::PRIMARY).is_some(),
        "未剥离时应保留 GPS"
    );
    let dt = exif
        .get_field(Tag::DateTimeOriginal, In::PRIMARY)
        .expect("拍摄时间必须保留");
    assert!(dt.display_value().to_string().contains("2026-06-28 15:30:00"));
    assert!(
        exif.get_field(Tag::Model, In::PRIMARY)
            .unwrap()
            .display_value()
            .to_string()
            .contains("ILCE-7RM5")
    );

    // GPS 剥离（removeGps=true）+ 版权/作者/关键词
    let mut options = folder_options(&out_dir, "stripped.jpg");
    options.remove_gps = true;
    options.copyright = Some("© 2026 Studio".into());
    options.author = Some("张三".into());
    options.keywords = vec!["婚礼".into(), "成片".into()];
    let row = run_job(&fixture._dir.path().join("db"), id, &plain_recipe(), &options, &fixture.photo_root);
    let bytes = std::fs::read(&row.output_path.unwrap()).unwrap();
    let exif = read_exif(&bytes);
    assert!(
        exif.get_field(Tag::GPSLatitude, In::PRIMARY).is_none(),
        "removeGps 后不应再有 GPS"
    );
    assert!(
        exif.get_field(Tag::DateTimeOriginal, In::PRIMARY).is_some(),
        "剥离 GPS 不应丢拍摄时间"
    );
    assert!(bytes.windows(9).any(|w| w == b"dc:rights"), "XMP dc:rights");
    assert!(bytes.windows(10).any(|w| w == b"dc:creator"), "XMP dc:creator");
    assert!(bytes.windows(10).any(|w| w == b"dc:subject"), "XMP dc:subject");
    assert!(bytes.windows(6).any(|w| w == "婚礼".as_bytes()), "XMP 关键词");
    let copyright = "© 2026 Studio";
    assert!(
        bytes.windows(copyright.len()).any(|w| w == copyright.as_bytes()),
        "XMP dc:rights 内容"
    );
}

#[test]
fn folder_export_target_exists_errors_without_overwrite() {
    let fixture = setup();
    let out_dir = fixture._dir.path().join("exports");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(out_dir.join("taken.jpg"), b"marker").unwrap();
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0002.jpg",
        &build_source_jpeg(64, 48),
        None,
    );

    let db_dir = fixture._dir.path().join("db");
    let database = common::open_db(&db_dir);
    let validated = edit::export::validate_options(&database, &folder_options(&out_dir, "taken.jpg")).unwrap();
    let job_id = database.export_job_create(id, "folder").unwrap();
    let request = edit::export::ExportJobRequest {
        job_id,
        asset_id: id,
        asset: database.asset_by_id(id).unwrap().unwrap(),
        recipe: plain_recipe(),
        options: validated,
        photo_root: fixture.photo_root.clone(),
    };
    edit::export::run_export_job(database, &EventBus::new(), request);
    let row = common::open_db(&db_dir).export_job_get(job_id).unwrap().unwrap();
    assert_eq!(row.status, "error");
    assert!(row.error.unwrap().contains("已存在"));
    // 既有文件不被覆盖
    assert_eq!(std::fs::read(out_dir.join("taken.jpg")).unwrap(), b"marker");
}

// ---------------------------------------------------------------------------
// 旋转 → 裁剪（坐标正确性）
// ---------------------------------------------------------------------------

#[test]
fn rotate_quarter_then_crop_maps_source_region() {
    let fixture = setup();
    let out_dir = fixture._dir.path().join("exports");
    // 左半红右半蓝的 100x60 源
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0003.jpg",
        &build_bicolor_jpeg(100, 60),
        None,
    );

    // 顺时针 90°：100x60 → 60x100（红半成为顶部），再裁 x∈[0.25,0.75], h=1
    // → 30x100；顶行红、底行蓝（源左半 x<50 经 rotate90cw 落在输出行 y'<50）
    let recipe = edit::recipe::parse_recipe(&serde_json::json!({
        "version": 1,
        "rotateQuarter": 1,
        "crop": { "x": 0.25, "y": 0.0, "w": 0.5, "h": 1.0 }
    }))
    .unwrap();
    let row = run_job(
        &fixture._dir.path().join("db"),
        id,
        &recipe,
        &folder_options(&out_dir, "rot.jpg"),
        &fixture.photo_root,
    );
    let bytes = std::fs::read(&row.output_path.unwrap()).unwrap();
    let img = image::load_from_memory(&bytes).unwrap().to_rgb8();
    assert_eq!((img.width(), img.height()), (30, 100), "旋转后裁剪比例");
    assert!(img.get_pixel(5, 2)[0] > 150 && img.get_pixel(5, 2)[2] < 80, "顶部应为红半");
    assert!(img.get_pixel(5, 97)[2] > 150 && img.get_pixel(5, 97)[0] < 80, "底部应为蓝半");
}

// ---------------------------------------------------------------------------
// album 模式：落位 / 登记 / 子分组 / 重名
// ---------------------------------------------------------------------------

#[test]
fn album_export_placement_subgroup_and_conflict_suffix() {
    let fixture = setup();
    let album = fixture.db.album_create("交付册").unwrap();
    // 固定创建时间 2026-04-12 → 主目录 2026/04/交付册（与拍摄日 06-01 无关）
    fixture
        .db
        .0
        .execute(
            "UPDATE album SET created_at = '2026-04-12T03:00:00.000Z' WHERE id = ?1",
            [album.id],
        )
        .unwrap();
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0004.jpg",
        &build_source_jpeg(80, 60),
        Some("2026-06-01T10:00:00.000Z"),
    );

    let db_dir = fixture._dir.path().join("db");
    let options = album_options(album.id, Some("成片"));
    let row = run_job(&db_dir, id, &plain_recipe(), &options, &fixture.photo_root);

    // 落位：{root}/{创建YYYY}/{创建MM}/{dir_name}/{子组}/{stem}_edit.jpg
    // （0022 子组物理化：相册内平铺的唯一例外 = 子组段——与导入/claim/移组
    // 同公式 album_item_home_rel；外层=相册创建年月 UTC）
    let dst = PathBuf::from(row.output_path.clone().unwrap());
    let expected = fixture
        .photo_root
        .join("2026")
        .join("04")
        .join(album.dir_name.clone())
        .join("成片")
        .join("DSC_0004_edit.jpg");
    assert_eq!(dst, expected, "album 落位路径（含子组段）");
    assert!(dst.is_file());

    // 登记：新资产 + album_item 子分组 + 拍摄时间/相机沿用
    let new_id = row.new_asset_id.unwrap();
    let registered: Option<(String, String)> = fixture
        .db
        .0
        .query_row(
            "SELECT path, kind FROM assets WHERE id = ?1",
            [new_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let (reg_path, kind) = registered.unwrap();
    assert_eq!(reg_path, dst.to_string_lossy());
    assert_eq!(kind, "photo");
    let subgroup: Option<String> = fixture
        .db
        .0
        .query_row(
            "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![album.id, new_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(subgroup.as_deref(), Some("成片"));
    let captured: Option<String> = fixture
        .db
        .0
        .query_row("SELECT captured_at FROM assets WHERE id = ?1", [new_id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(captured.as_deref(), Some("2026-06-01T10:00:00.000Z"));
    let camera: Option<String> = fixture
        .db
        .0
        .query_row("SELECT camera FROM assets WHERE id = ?1", [new_id], |r| r.get(0))
        .unwrap();
    assert_eq!(camera.as_deref(), Some("Sony ILCE-7RM5"));
    // 导入引擎同款登记：索引任务（thumb/eyes/blur）就位
    let tasks: i64 = fixture
        .db
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE asset_id = ?1 AND kind IN ('thumb','eyes','blur')",
            [new_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(tasks, 3);

    // 重名冲突：预占 _edit.jpg 后再导出 → _edit_1.jpg（导入引擎 _N 规则）
    let options = album_options(album.id, Some("成片"));
    let row2 = run_job(&db_dir, id, &plain_recipe(), &options, &fixture.photo_root);
    let dst2 = PathBuf::from(row2.output_path.unwrap());
    assert!(
        dst2.file_name().unwrap().to_string_lossy().ends_with("_edit_1.jpg"),
        "冲突追加 _1 后缀: {}",
        dst2.display()
    );
    assert!(dst2.is_file());
    // 相册根（subgroup=None）→ 平铺主目录（无子组段；根目录首件无后缀）
    let options = album_options(album.id, None);
    let row3 = run_job(&db_dir, id, &plain_recipe(), &options, &fixture.photo_root);
    let dst3 = PathBuf::from(row3.output_path.clone().unwrap());
    let expected3 = fixture
        .photo_root
        .join("2026")
        .join("04")
        .join(&album.dir_name)
        .join("DSC_0004_edit.jpg");
    assert_eq!(dst3, expected3, "subgroup 可空 = 相册根平铺");
    assert!(dst3.is_file());
    let subgroup3: Option<String> = fixture
        .db
        .0
        .query_row(
            "SELECT subgroup FROM album_item WHERE album_id = ?1 AND asset_id = ?2",
            rusqlite::params![album.id, row3.new_asset_id.unwrap()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(subgroup3, None, "subgroup 可空 = 相册根");
}

// ---------------------------------------------------------------------------
// 文字 / 笔迹渲染冒烟
// ---------------------------------------------------------------------------

#[test]
fn text_and_brush_layers_change_pixels() {
    let fixture = setup();
    let out_dir = fixture._dir.path().join("exports");
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0005.jpg",
        &build_source_jpeg(120, 90),
        None,
    );

    let row_plain = run_job(
        &fixture._dir.path().join("db"),
        id,
        &plain_recipe(),
        &folder_options(&out_dir, "plain.jpg"),
        &fixture.photo_root,
    );
    let annotated = edit::recipe::parse_recipe(&serde_json::json!({
        "version": 1,
        "textLayers": [
            { "id": "t1", "x": 0.05, "y": 0.05, "text": "多行\n支持",
              "sizeRel": 0.08, "color": "#FFFFFF" },
            { "id": "t2", "x": 0.5, "y": 0.1, "text": "OK", "sizeRel": 0.05, "color": "#00FF00" }
        ],
        "brushStrokes": [
            { "id": "b1", "color": "#FF3333", "widthRel": 0.02,
              "points": [{ "x": 0.1, "y": 0.7 }, { "x": 0.5, "y": 0.8 }, { "x": 0.9, "y": 0.7 }] }
        ]
    }))
    .unwrap();
    let row_marked = run_job(
        &fixture._dir.path().join("db"),
        id,
        &annotated,
        &folder_options(&out_dir, "marked.jpg"),
        &fixture.photo_root,
    );

    let plain = image::load_from_memory(&std::fs::read(row_plain.output_path.unwrap()).unwrap())
        .unwrap()
        .to_rgb8();
    let marked = image::load_from_memory(&std::fs::read(row_marked.output_path.unwrap()).unwrap())
        .unwrap()
        .to_rgb8();
    assert_eq!((plain.width(), plain.height()), (120, 90));
    assert_eq!(
        (marked.width(), marked.height()),
        (120, 90),
        "标注不改尺寸"
    );
    let diff = plain
        .pixels()
        .zip(marked.pixels())
        .filter(|(a, b)| a != b)
        .count();
    assert!(diff > 200, "文字/笔迹应产生可见像素差异（diff={diff}）");
    // 笔迹红色确实落进画布（0.1..0.9 横带中心行采样）
    let red_hit = marked
        .enumerate_pixels()
        .any(|(_, _, p)| p[0] > 180 && p[1] < 120 && p[2] < 120);
    assert!(red_hit, "笔迹红色像素应存在");
}

// ---------------------------------------------------------------------------
// IPC 后台任务链路（supervisor + 事件 + 持久化）
// ---------------------------------------------------------------------------

#[test]
fn export_run_ipc_background_job_and_events() {
    let fixture = setup();
    let out_dir = fixture._dir.path().join("exports");
    let id = ins_photo(
        &fixture.db,
        &fixture.photo_root,
        "DSC_0006.jpg",
        &build_source_jpeg(64, 48),
        Some("2026-06-01T10:00:00.000Z"),
    );

    let mut rx = fixture.state.bus.subscribe();
    let task = edit::ipc::fetch_export_run(
        &fixture.state,
        id,
        &serde_json::json!({ "version": 1 }),
        &folder_options(&out_dir, "bg.jpg"),
    )
    .unwrap();
    assert_eq!(task.status, "queued");
    assert_eq!(task.mode, "folder");
    assert!(task.result.is_none());

    // 后台线程跑完（worker 独立连接落库，轮询同一库文件）
    let db_dir = fixture._dir.path().join("db");
    let poller = common::open_db(&db_dir);
    assert!(
        edit::export::wait_terminal(&poller, task.id, Duration::from_secs(30)),
        "导出任务超时未收尾"
    );

    // 事件链：进度 + 收尾（bus → 前端 app://event 的既有通道）
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (mut saw_progress, mut finished) = (false, false);
    while std::time::Instant::now() < deadline && !(saw_progress && finished) {
        match rx.try_recv() {
            Ok(AppEvent::ExportTaskProgress { job_id, .. }) => {
                saw_progress = saw_progress || job_id == task.id;
            }
            Ok(AppEvent::ExportTaskFinished { ok, output_path, .. }) => {
                assert!(ok);
                assert!(output_path.unwrap().ends_with("bg.jpg"));
                finished = true;
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::TryRecvError::Closed) => break,
        }
    }
    assert!(saw_progress, "应收到 exportTaskProgress");
    assert!(finished, "应收到 exportTaskFinished");

    // 持久化任务铁律：新连接读回终态 + 完整 DTO 形状
    let row = common::open_db(&db_dir).export_job_get(task.id).unwrap().unwrap();
    assert_eq!(row.status, "done");
    let dto = edit::export::ExportTaskDto::from(row);
    assert_eq!(dto.status, "done");
    let result = dto.result.unwrap();
    assert_eq!(result.width, 64);
    assert_eq!(result.height, 48);
    assert_eq!(result.asset_id, None);

    // 孤儿收尸：手工塞一条 running 行（不在活跃集合）→ 下次 export_run 收尸。
    // 用显式高位 id 避开并行测试的活跃集合碰撞（生产单库 id 唯一无此问题）。
    fixture
        .db
        .0
        .execute(
            "INSERT INTO export_job (id, asset_id, mode, status, created_at) \
             VALUES (987654, ?1, 'folder', 'running', '2026-01-01T00:00:00.000Z')",
            [id],
        )
        .unwrap();
    let task2 = edit::ipc::fetch_export_run(
        &fixture.state,
        id,
        &serde_json::json!({ "version": 1 }),
        &folder_options(&out_dir, "bg2.jpg"),
    )
    .unwrap();
    let poller = common::open_db(&db_dir);
    assert!(edit::export::wait_terminal(&poller, task2.id, Duration::from_secs(30)));
    let stale: Vec<String> = fixture
        .db
        .0
        .prepare("SELECT status FROM export_job WHERE status = 'error'")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(stale.len(), 1, "孤儿任务被收尸为 error");
}


#[test]
fn metadata_edits_are_per_asset_survive_reindex_and_leave_original_unchanged() {
    let fixture = setup();
    let bytes = build_source_jpeg(24, 16);
    let id = ins_photo(&fixture.db, &fixture.photo_root, "edited-meta.jpg", &bytes, Some("2026-06-28T15:30:00"));
    let other = ins_photo(&fixture.db, &fixture.photo_root, "other-meta.jpg", &bytes, None);
    let mut metadata = edit::metadata::get(&fixture.db, id).unwrap();
    metadata.title = "  雨后  ".into();
    metadata.author = "摄影师".into();
    metadata.lens = "".into();
    metadata.captured_at = Some("2026-09-28T09:00:00+08:00".into());
    metadata.keywords = vec!["树叶".into(), "树叶".into(), " ".into()];
    let saved = edit::metadata::save(&fixture.db, id, metadata).unwrap();
    assert_eq!(saved.title, "雨后");
    assert_eq!(saved.keywords, vec!["树叶"]);
    fixture.db.update_asset_deep_exif(id, &metadata::exif_lite::parse(&bytes)).unwrap();
    let asset = fixture.db.asset_by_id(id).unwrap().unwrap();
    assert_eq!(asset.artist.as_deref(), Some("摄影师"));
    assert_eq!(asset.lens, None);
    assert_eq!(asset.gps_lat, None);
    assert_eq!(asset.captured_at.as_deref(), Some("2026-09-28T09:00:00+08:00"));
    assert_eq!(edit::metadata::get(&fixture.db, id).unwrap(), saved);
    assert!(edit::metadata::get(&fixture.db, other).unwrap().title.is_empty());
    assert_eq!(std::fs::read(fixture.photo_root.join("edited-meta.jpg")).unwrap(), bytes);
    let mut invalid = saved.clone(); invalid.gps_lat = Some(200.0); invalid.gps_lon = Some(10.0);
    assert!(edit::metadata::save(&fixture.db, id, invalid).is_err());
    assert_eq!(edit::metadata::get(&fixture.db, id).unwrap(), saved);
}

#[test]
fn adjustments_render_black_and_white_and_clamp_invalid_ranges() {
    let recipe = edit::recipe::parse_recipe(&serde_json::json!({"version":1,"adjustments":{"brightness":200,"contrast":-200,"saturation":0}})).unwrap();
    assert_eq!(recipe.adjustments.unwrap().brightness, 100.0);
    assert_eq!(recipe.adjustments.unwrap().contrast, -100.0);
    let mut image = image::RgbImage::from_pixel(1, 1, image::Rgb([255, 0, 0]));
    edit::render::apply_adjustments(&mut image, edit::recipe::Adjustments { saturation: -100.0, ..Default::default() });
    assert_eq!(image.get_pixel(0, 0).0, [54, 54, 54]);
    let mut image = image::RgbImage::from_pixel(1, 1, image::Rgb([40, 80, 120]));
    edit::render::apply_adjustments(&mut image, edit::recipe::Adjustments { brightness: 100.0, ..Default::default() });
    assert_eq!(image.get_pixel(0, 0).0, [80, 160, 240]);
}
