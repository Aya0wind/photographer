//! 选片补全（0016，阶段 B1 + 2026-10-09 §五 M2c）：颜色标签（批量 + 非法
//! 拒绝 + XMP 边车写回 + xmp:Label 清除）、接受/拒绝状态（筛选 + 默认查询
//! 不排除）、应用内回收站全链路（移入 → 常规查询不可见 → 列表/还原 →
//! 清空新语义：在线库真删本体 / 离线库原样保留 + 总结 / 缺失项仅删记录）、
//! 与 XMP 边车同步（离线/缺失走 xmp_dirty 闭环）。

mod common;

use common::library_fixture as setup;

pub use common::{
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, platform, scan,
    settings, tasks, thumbs,
};

use std::time::Duration;

use common::open_db;
use db::{AssetFilters, AssetRow};
use events::AssetKind;
use ipc::rating::fetch_asset_rating_set;
use ipc::selection::{
    fetch_asset_label_set, fetch_asset_reject_set, fetch_asset_trash_move, fetch_trash_list,
    fetch_trash_purge, fetch_trash_restore,
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
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();
    db.asset_id_by_path(path).unwrap().unwrap()
}

fn names(dtos: &[ipc::assets::AssetDto]) -> Vec<String> {
    dtos.iter().map(|d| d.name.clone()).collect()
}

fn page(state: &ipc::AppState, filters: AssetFilters) -> Vec<String> {
    names(&ipc::assets::fetch_assets_page(state, 0, 50, filters).unwrap())
        .as_slice()
        .to_vec()
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

// ---------------------------------------------------------------------------
// 颜色标签
// ---------------------------------------------------------------------------

#[test]
fn color_label_batch_set_filter_and_invalid_rejected() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    let c = ins(&db, "X:/p/c.jpg", None, AssetKind::Photo);

    // 批量打标 + 返回更新行数
    let n = fetch_asset_label_set(&state, &[a, b], Some("red")).unwrap();
    assert_eq!(n, 2);
    // 空列表 no-op
    assert_eq!(fetch_asset_label_set(&state, &[], Some("blue")).unwrap(), 0);

    // 筛选 colorLabel；默认查询不排除任何标签（NULL captured_at → id DESC 序）
    assert_eq!(
        page(
            &state,
            AssetFilters {
                color_label: Some("red".into()),
                ..Default::default()
            }
        ),
        vec!["b.jpg".to_string(), "a.jpg".to_string()]
    );
    assert_eq!(page(&state, AssetFilters::default()).len(), 3);

    // 详情载荷带 colorLabel（camelCase）
    let detail = ipc::assets::fetch_asset_detail(&state, a).unwrap().unwrap();
    let json = serde_json::to_value(&detail).unwrap();
    assert_eq!(json["colorLabel"], "red");

    // 清除（None）→ 筛选命中空集
    fetch_asset_label_set(&state, &[a], None).unwrap();
    assert_eq!(
        page(
            &state,
            AssetFilters {
                color_label: Some("red".into()),
                ..Default::default()
            }
        ),
        vec!["b.jpg".to_string()]
    );
    let cleared: Option<String> =
        db.0.query_row("SELECT color_label FROM assets WHERE id = ?1", [a], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(cleared, None);
    // 未触碰的 c 仍无标签
    let label_c: Option<String> =
        db.0.query_row("SELECT color_label FROM assets WHERE id = ?1", [c], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(label_c, None);

    // 非法色名拒绝（大小写归一只认 LR 标准五色）
    assert!(fetch_asset_label_set(&state, &[a], Some("magenta")).is_err());
    assert!(fetch_asset_label_set(&state, &[a], Some("")).is_ok_and(|_| true)); // 空 = 清除，合法
    assert!(fetch_asset_label_set(&state, &[a], Some("BLUE")).is_ok());
}

#[test]
fn color_label_syncs_xmp_sidecar_and_clear_removes_attribute() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let photo = photo_root.join("DSC_0001.NEF");
    std::fs::write(&photo, b"fake-nef").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), None, AssetKind::Raw);

    // 预置一份 LR 风格边车（含调色参数 + 旧标签），验证外科手术保留
    let sidecar = xmp::sidecar_path(&photo);
    std::fs::write(
        &sidecar,
        r#"<?xpacket begin=""?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="2" xmp:Label="Blue" crs:Sharpness="25">
   <crs:Look><rdf:Bag><rdf:li>Punch</rdf:li></rdf:Bag></crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
    )
    .unwrap();

    // 打 red 标签 → 边车异步更新（supervisor 线程；轮询等待）
    fetch_asset_label_set(&state, &[id], Some("red")).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            xmp::read_label(&std::fs::read_to_string(&sidecar).unwrap()).as_deref() == Some("Red")
        }),
        "边车 xmp:Label 应异步更新为 Red"
    );
    let text = std::fs::read_to_string(&sidecar).unwrap();
    // 未触字段逐字节语义保留
    assert!(text.contains(r#"xmp:Rating="2""#));
    assert!(text.contains(r#"crs:Sharpness="25""#));
    assert!(text.contains("Punch"));
    assert!(!text.contains("Blue"));
    assert_eq!(xmp::sidecar_rating(&text), Some(2));

    // 清除标签 → xmp:Label 整属性摘除，其余保留
    fetch_asset_label_set(&state, &[id], None).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            let t = std::fs::read_to_string(&sidecar).unwrap();
            xmp::read_label(&t).is_none() && !t.contains("xmp:Label")
        }),
        "清除后边车应无 xmp:Label"
    );
    let text = std::fs::read_to_string(&sidecar).unwrap();
    assert!(text.contains(r#"xmp:Rating="2""#));
    assert!(text.contains(r#"crs:Sharpness="25""#));
    assert!(text.contains("Punch"));

    // 无档新建：打标签 → 最小模板边车
    let fresh = photo_root.join("DSC_0002.CR2");
    std::fs::write(&fresh, b"fake-raw").unwrap();
    let id2 = ins(&db, &fresh.to_string_lossy(), None, AssetKind::Raw);
    fetch_asset_label_set(&state, &[id2], Some("purple")).unwrap();
    let fresh_sidecar = xmp::sidecar_path(&fresh);
    assert!(
        wait_until(Duration::from_secs(10), || {
            fresh_sidecar.is_file()
                && xmp::read_label(&std::fs::read_to_string(&fresh_sidecar).unwrap()).as_deref()
                    == Some("Purple")
        }),
        "无边车时应新建最小模板并携带 Label"
    );

    // 用户主动修改外部引用照片的标签、拒绝状态也写原文件旁的边车。
    let ext_path = dir.path().join("elsewhere").join("DSC_0003.jpg");
    std::fs::create_dir_all(dir.path().join("elsewhere")).unwrap();
    std::fs::write(&ext_path, b"ext").unwrap();
    let ext_id = {
        let mut row = AssetRow {
            path: ext_path.to_string_lossy().into_owned(),
            filename: "DSC_0003.jpg".into(),
            size: 100,
            mtime: "2026-09-01T00:00:00.000Z".into(),
            xxhash: 77,
            kind: AssetKind::Photo,
            captured_at: None,
            camera: None,
            source: "indexed".into(),
            created_at: "2026-09-01T00:00:00.000Z".into(),
            origin: "external".into(),
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
            library_id: None,
            missing: 0,
            xmp_dirty: 0,
            volume_serial: None,
            file_id: None,
        };
        row.origin = "external".into();
        db.insert_asset(&row).unwrap();
        db.asset_id_by_path(&ext_path.to_string_lossy())
            .unwrap()
            .unwrap()
    };
    fetch_asset_label_set(&state, &[ext_id], Some("green")).unwrap();
    let label: Option<String> =
        db.0.query_row(
            "SELECT color_label FROM assets WHERE id = ?1",
            [ext_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(label.as_deref(), Some("green"));
    let ext_sidecar = xmp::sidecar_path(&ext_path);
    assert!(
        wait_until(Duration::from_secs(10), || {
            ext_sidecar.is_file()
                && xmp::read_label(&std::fs::read_to_string(&ext_sidecar).unwrap()).as_deref()
                    == Some("Green")
        }),
        "外部引用照片的主动标签应写入边车"
    );
    fetch_asset_reject_set(&state, &[ext_id], true).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            ext_sidecar.is_file()
                && std::fs::read_to_string(&ext_sidecar)
                    .unwrap()
                    .contains(r#"xmp:Rating="-1""#)
        }),
        "外部引用照片的主动拒绝状态应写入边车"
    );
}

// ---------------------------------------------------------------------------
// 接受/拒绝状态
// ---------------------------------------------------------------------------

#[test]
fn reject_status_filter_and_default_inclusive() {
    let (_dir, state, db) = setup();
    let a = ins(&db, "X:/p/a.jpg", None, AssetKind::Photo);
    let b = ins(&db, "X:/p/b.jpg", None, AssetKind::Photo);
    ins(&db, "X:/p/c.jpg", None, AssetKind::Photo);

    fetch_asset_reject_set(&state, &[a], true).unwrap();

    // rejected=true/false 筛选
    assert_eq!(
        page(
            &state,
            AssetFilters {
                rejected: Some(true),
                ..Default::default()
            }
        ),
        vec!["a.jpg".to_string()]
    );
    assert_eq!(
        page(
            &state,
            AssetFilters {
                rejected: Some(false),
                ..Default::default()
            }
        )
        .len(),
        2
    );
    // 默认查询不排除已拒绝（区别于回收站）
    assert_eq!(page(&state, AssetFilters::default()).len(), 3);

    // 撤销拒绝
    fetch_asset_reject_set(&state, &[a, b], false).unwrap();
    assert_eq!(
        page(
            &state,
            AssetFilters {
                rejected: Some(true),
                ..Default::default()
            }
        )
        .len(),
        0
    );
    // AssetDto 契约：rejected 随行
    let dtos = ipc::assets::fetch_assets_page(&state, 0, 10, AssetFilters::default()).unwrap();
    assert!(dtos.iter().all(|d| !d.rejected));
}

// ---------------------------------------------------------------------------
// 回收站全链路
// ---------------------------------------------------------------------------

#[test]
fn trash_full_cycle_move_hidden_list_restore() {
    let (_dir, state, db) = setup();
    let a = ins(
        &db,
        "X:/p/a.jpg",
        Some("2025-06-01T10:00:00.000Z"),
        AssetKind::Photo,
    );
    let b = ins(
        &db,
        "X:/p/b.jpg",
        Some("2024-06-01T10:00:00.000Z"),
        AssetKind::Photo,
    );
    let c = ins(
        &db,
        "X:/p/c.jpg",
        Some("2023-06-01T10:00:00.000Z"),
        AssetKind::Photo,
    );

    // 常规链路预热：日期分组/侧栏计数基线
    assert_eq!(db.asset_group_dates().unwrap().len(), 3);
    assert_eq!(db.sidebar_assets_count().unwrap(), 3);

    // 移入回收站（a、b）
    let moved = fetch_asset_trash_move(&state, &[a, b]).unwrap();
    assert_eq!(moved, 2);
    // 幂等重试：已在站的行不重复计数、trashed_at 不刷新
    assert_eq!(fetch_asset_trash_move(&state, &[a]).unwrap(), 0);

    // 常规查询全链路不可见：画廊分页 / 计数 / 日期分组 / 那年今天 / 最近添加 /
    // 最近浏览 / 相机聚合 / 侧栏计数 / 去重扫描
    assert_eq!(
        page(&state, AssetFilters::default()),
        vec!["c.jpg".to_string()]
    );
    assert_eq!(
        ipc::assets::fetch_assets_count(&state, AssetFilters::default()).unwrap(),
        1
    );
    assert_eq!(db.asset_group_dates().unwrap().len(), 1);
    // 那年今天：站内只剩 c（2023-06-01），a/b（06-01 历年）已入站不再出现
    assert_eq!(db.assets_on_this_day("06-01").unwrap().len(), 1);
    assert_eq!(
        ipc::rating::fetch_recent_assets(&state, 0, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.recently_viewed(10).unwrap().len(), 0);
    let cams = db.camera_list().unwrap();
    assert_eq!(cams.iter().map(|c| c.count).sum::<u64>(), 1);
    assert_eq!(db.sidebar_assets_count().unwrap(), 1);
    assert!(db.exact_duplicate_keys().unwrap().is_empty());

    // 语义检索 join：回收站资产不可检索
    assert!(!db.asset_searchable(a).unwrap());
    assert!(db.asset_searchable(c).unwrap());

    // 回收站列表：trashed_at DESC（后移入的 b 在前）、复用 AssetDto
    let list = fetch_trash_list(&state, 0, 10).unwrap();
    assert_eq!(names(&list), vec!["b.jpg".to_string(), "a.jpg".to_string()]);
    assert!(list.iter().all(|d| d.path.contains(".jpg")));

    // keyset：after = a 的 id → 只剩第一页后的内容（a 之后为空）
    let after_a = list.iter().find(|d| d.id == a).unwrap().id;
    assert!(fetch_trash_list(&state, after_a, 10).unwrap().is_empty());

    // 还原 b → 回到常规查询，回收站剩 a
    assert_eq!(fetch_trash_restore(&state, &[b]).unwrap(), 1);
    assert_eq!(page(&state, AssetFilters::default()).len(), 2);
    assert_eq!(
        names(&fetch_trash_list(&state, 0, 10).unwrap()),
        vec!["a.jpg".to_string()]
    );
    assert_eq!(fetch_trash_restore(&state, &[b]).unwrap(), 0); // 幂等
}

#[test]
fn trash_purge_deletes_rows_files_and_cascades_references() {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    // a：真实文件 + 相册引用 + 人脸 + 浏览历史
    let photo = photo_root.join("DSC_0001.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let a = ins(&db, &photo.to_string_lossy(), None, AssetKind::Photo);
    let album = db.album_create("相册一").unwrap();
    assert_eq!(db.album_add_assets(album.id, &[a], None).unwrap(), 1);
    db.insert_face(a, 1.0, 2.0, 10.0, 10.0, &[0.5f32; 512], None)
        .unwrap();
    db.mark_asset_viewed(a).unwrap();

    // 外部库资产（文件不在库内）：purge 只删库行绝不物理删
    let ext_dir = dir.path().join("elsewhere");
    std::fs::create_dir_all(&ext_dir).unwrap();
    let ext_file = ext_dir.join("DSC_0002.jpg");
    std::fs::write(&ext_file, b"ext").unwrap();
    let ext = {
        let mut row = AssetRow {
            path: ext_file.to_string_lossy().into_owned(),
            filename: "DSC_0002.jpg".into(),
            size: 100,
            mtime: "2026-09-01T00:00:00.000Z".into(),
            xxhash: 42,
            kind: AssetKind::Photo,
            captured_at: None,
            camera: None,
            source: "indexed".into(),
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
            library_id: None,
            missing: 0,
            xmp_dirty: 0,
            volume_serial: None,
            file_id: None,
        };
        row.origin = "external".into();
        db.insert_asset(&row).unwrap();
        db.asset_id_by_path(&ext_file.to_string_lossy())
            .unwrap()
            .unwrap()
    };

    // 移入回收站
    fetch_asset_trash_move(&state, &[a, ext]).unwrap();

    // purge（delete_files=false）：库行 + 级联消失，物理文件保留
    let result = fetch_trash_purge(&state, &[a, ext], false).unwrap();
    assert_eq!(result.deleted_records, 2);
    assert_eq!(result.deleted_files, 0, "delete_files=false 不动物理文件");
    assert_eq!(result.offline_kept, 0);
    assert!(photo.is_file());
    assert!(ext_file.is_file());
    assert!(db.asset_albums(a).unwrap().is_empty(), "相册引用级联清");
    let faces: i64 =
        db.0.query_row("SELECT COUNT(*) FROM faces WHERE asset_id = ?1", [a], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(faces, 0, "人脸级联清");
    let history: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM view_history WHERE asset_id = ?1",
            [a],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(history, 0, "浏览历史级联清");

    // 再走一遍带 delete_files=true 的完整流程（library_id=NULL 视同在线库）
    let p2 = photo_root.join("DSC_0003.jpg");
    std::fs::write(&p2, b"jpeg2").unwrap();
    let c = ins(&db, &p2.to_string_lossy(), None, AssetKind::Photo);
    fetch_asset_trash_move(&state, &[c]).unwrap();
    let result = fetch_trash_purge(&state, &[c], true).unwrap();
    assert_eq!(result.deleted_records, 1);
    assert_eq!(result.deleted_files, 1, "在线资产真删本体");
    assert!(!p2.exists(), "delete_files=true 应物理删除文件");
    // 幂等：再 purge 无剩余行
    assert_eq!(fetch_trash_purge(&state, &[c], true).unwrap().deleted_records, 0);
    // 常规查询回到干净状态
    assert_eq!(db.sidebar_assets_count().unwrap(), 0);
}

// ---------------------------------------------------------------------------
// 清空回收站新语义（§五 M2c）：离线库原样保留 + 缺失项仅删记录
// ---------------------------------------------------------------------------

/// 双库 fixture：A 在线（root 在盘）、B 标记离线（root 仍在盘——status 是
/// purge 唯一 consulted 的真相，不信任离线卷上的任何文件操作）。
fn two_libraries(
    state: &ipc::AppState,
) -> (tempfile::TempDir, String, std::path::PathBuf, String, std::path::PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let root_a = dir.path().join("lib-a");
    let root_b = dir.path().join("lib-b");
    std::fs::create_dir_all(&root_a).unwrap();
    std::fs::create_dir_all(&root_b).unwrap();
    let db = open_db(&state.config_dir);
    let a = db
        .photos_library_register("在线库", &root_a.to_string_lossy(), &state.config_dir)
        .unwrap();
    let b = db
        .photos_library_register("离线库", &root_b.to_string_lossy(), &state.config_dir)
        .unwrap();
    db.photos_library_set_status(&b.id, "offline").unwrap();
    (dir, a.id, root_a, b.id, root_b)
}

/// 插入库内资产（真实文件可选；missing 可选），返回 (id, 文件路径)。
#[allow(clippy::too_many_arguments)]
fn ins_in_library(
    db: &db::Db,
    library_id: &str,
    root: &std::path::Path,
    name: &str,
    with_file: bool,
    missing: bool,
) -> (i64, std::path::PathBuf) {
    let path = root.join("2026").join("06").join(name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    if with_file {
        std::fs::write(&path, b"jpeg").unwrap();
    }
    db.insert_asset(&AssetRow {
        path: path.to_string_lossy().into_owned(),
        filename: name.to_string(),
        size: 4,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: name.len() as u64,
        kind: AssetKind::Photo,
        captured_at: None,
        camera: None,
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
        library_id: Some(library_id.to_string()),
        missing: i64::from(missing),
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();
    let id = db.asset_id_by_path(&path.to_string_lossy()).unwrap().unwrap();
    (id, path)
}

#[test]
fn trash_purge_keeps_offline_library_items_and_reports_summary() {
    let (_dir, state, db) = setup();
    let (dir, lib_a, root_a, lib_b, root_b) = two_libraries(&state);
    let db = open_db(&state.config_dir);
    let (a, file_a) = ins_in_library(&db, &lib_a, &root_a, "online.jpg", true, false);
    let (b, file_b) = ins_in_library(&db, &lib_b, &root_b, "offline.jpg", true, false);
    let (m, file_m) = ins_in_library(&db, &lib_a, &root_a, "missing.jpg", false, true);

    fetch_asset_trash_move(&state, &[a, b, m]).unwrap();
    let result = fetch_trash_purge(&state, &[a, b, m], true).unwrap();

    // 在线库真删本体；缺失项仅删记录；离线库原样保留 + 总结提示。
    assert_eq!(result.deleted_files, 1, "只有在线库的本体被真删");
    assert!(!file_a.exists(), "在线库本体已删");
    assert!(file_b.is_file(), "离线库回收站项原样保留（文件不动）");
    assert_eq!(result.offline_kept, 1);
    assert_eq!(result.offline_libraries, vec!["离线库".to_string()]);
    assert_eq!(result.missing_records_only, 1, "缺失项仅删记录");
    assert_eq!(result.deleted_records, 2, "删除 a + m 两行；离线项不计");
    let _ = file_m;

    // 离线库资产行仍在回收站（后续可恢复）；恢复在线后再清空即真删。
    let trash = names(&fetch_trash_list(&state, 0, 10).unwrap());
    assert_eq!(trash, vec!["offline.jpg".to_string()], "离线项仍在回收站");
    db.photos_library_set_status(&lib_b, "online").unwrap();
    let result = fetch_trash_purge(&state, &[b], true).unwrap();
    assert_eq!(result.deleted_files, 1);
    assert!(!file_b.exists(), "库恢复在线后真删本体");
    assert_eq!(result.offline_kept, 0);
    let _ = dir;
}

#[test]
fn trash_purge_offline_library_never_record_deletes_even_without_files() {
    let (_dir, state, _db) = setup();
    let (_dir2, _lib_a, _root_a, lib_b, root_b) = two_libraries(&state);
    let db = open_db(&state.config_dir);
    let (b, file_b) = ins_in_library(&db, &lib_b, &root_b, "keep.jpg", true, false);

    fetch_asset_trash_move(&state, &[b]).unwrap();
    // delete_files=false 也不动离线库记录（§五 定案：不做「强制仅删记录」）。
    let result = fetch_trash_purge(&state, &[b], false).unwrap();
    assert_eq!(result.offline_kept, 1, "离线库项原样保留");
    assert_eq!(result.deleted_records, 0);
    assert!(file_b.is_file());
    let kept: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM assets WHERE id = ?1 AND in_trash = 1",
            [b],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(kept, 1, "回收站行保留");
}

// ---------------------------------------------------------------------------
// xmp_dirty 写方向闭环（§五 M2c）：缺失/离线期间的改动进库置脏，不派边车
// ---------------------------------------------------------------------------

#[test]
fn label_and_reject_on_offline_library_enter_db_dirty_without_sidecar() {
    let (_dir, state, _db) = setup();
    let (dir, _lib_a, _root_a, lib_b, root_b) = two_libraries(&state);
    let db = open_db(&state.config_dir);
    let (b, file_b) = ins_in_library(&db, &lib_b, &root_b, "dirty.nef", true, false);

    // 离线库上打标签 + 拒绝：入库 + 置脏；边车绝不写（库不在盘语义）。
    assert_eq!(fetch_asset_label_set(&state, &[b], Some("red")).unwrap(), 1);
    assert_eq!(fetch_asset_reject_set(&state, &[b], true).unwrap(), 1);
    let (label, rejected, dirty): (Option<String>, i64, i64) = db
        .0
        .query_row(
            "SELECT color_label, rejected, xmp_dirty FROM assets WHERE id = ?1",
            [b],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(label.as_deref(), Some("red"));
    assert_eq!(rejected, 1);
    assert_eq!(dirty, 1, "离线期间改动置脏");
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !xmp::sidecar_path(&file_b).exists(),
        "离线库不派边车任务"
    );

    // 库恢复在线 → 扫一轮 → 补写边车、清标志（与 rating_watch 读入方向对称）。
    db.photos_library_set_status(&lib_b, "online").unwrap();
    let library = db.photos_library_get(&lib_b).unwrap().unwrap();
    let report = scan::scan_library_once(
        &db,
        &dir.path().join("db"),
        &library,
        &scan::ScanOptions {
            cooldown: Duration::ZERO,
            now: None,
        },
        None,
        None,
    )
    .unwrap();
    assert_eq!(report.xmp_flushed, 1, "脏资产补写冲刷");
    let sidecar = xmp::sidecar_path(&file_b);
    let text = std::fs::read_to_string(&sidecar).unwrap();
    assert_eq!(xmp::read_label(&text).as_deref(), Some("Red"));
    assert!(text.contains(r#"xmp:Rating="-1""#), "拒绝投影 -1");
    let dirty: i64 = db
        .0
        .query_row("SELECT xmp_dirty FROM assets WHERE id = ?1", [b], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(dirty, 0, "补写后清标志");
}

// ---------------------------------------------------------------------------
// XMP 即时投影（用户定案）：边车评分 = rejected ? -1 : rating(0-5)
// ---------------------------------------------------------------------------

#[test]
fn reject_projects_minus_one_and_restore_projects_rating() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let photo = photo_root.join("DSC_0010.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), None, AssetKind::Photo);
    let sidecar = xmp::sidecar_path(&photo);

    // 拒绝 → 边车新建且 Rating = -1（DB 评分仍 0）
    fetch_asset_reject_set(&state, &[id], true).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            sidecar.is_file()
                && std::fs::read_to_string(&sidecar)
                    .unwrap()
                    .contains(r#"xmp:Rating="-1""#)
        }),
        "拒绝应投影 xmp:Rating=-1（无边车时新建）"
    );
    let db_rating: i64 =
        db.0.query_row("SELECT rating FROM assets WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(db_rating, 0, "评分真值保留在 DB");

    // 取消拒绝 → 恢复投影星级（0）
    fetch_asset_reject_set(&state, &[id], false).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            std::fs::read_to_string(&sidecar)
                .unwrap()
                .contains(r#"xmp:Rating="0""#)
        }),
        "取消拒绝应恢复投影星级"
    );
}

#[test]
fn rejected_state_rating_change_still_projects_minus_one() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let photo = photo_root.join("DSC_0011.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), None, AssetKind::Photo);
    let sidecar = xmp::sidecar_path(&photo);

    // 拒绝态下改星级：DB 保留 4，边车仍投影 -1
    fetch_asset_reject_set(&state, &[id], true).unwrap();
    fetch_asset_rating_set(&state, id, 4).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            std::fs::read_to_string(&sidecar)
                .unwrap()
                .contains(r#"xmp:Rating="-1""#)
        }),
        "拒绝态改星级仍应投影 -1"
    );
    let db_rating: i64 =
        db.0.query_row("SELECT rating FROM assets WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(db_rating, 4, "星级真值保留在 DB");

    // 取消拒绝 → 恢复投影新星级 4
    fetch_asset_reject_set(&state, &[id], false).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            std::fs::read_to_string(&sidecar)
                .unwrap()
                .contains(r#"xmp:Rating="4""#)
        }),
        "取消拒绝应投影 DB 星级"
    );
}

#[test]
fn label_and_rating_coexist_and_reject_keeps_label() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let photo = photo_root.join("DSC_0012.jpg");
    std::fs::write(&photo, b"jpeg").unwrap();
    let id = ins(&db, &photo.to_string_lossy(), None, AssetKind::Photo);
    let sidecar = xmp::sidecar_path(&photo);

    // 星级 → 颜色：两个独立外科手术共存
    fetch_asset_rating_set(&state, id, 3).unwrap();
    fetch_asset_label_set(&state, &[id], Some("red")).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            let t = std::fs::read_to_string(&sidecar).unwrap_or_default();
            t.contains(r#"xmp:Rating="3""#) && t.contains(r#"xmp:Label="Red""#)
        }),
        "星级与颜色应共存于同一边车"
    );

    // 拒绝：Rating 投影 -1，Label 不受扰
    fetch_asset_reject_set(&state, &[id], true).unwrap();
    assert!(
        wait_until(Duration::from_secs(10), || {
            let t = std::fs::read_to_string(&sidecar).unwrap_or_default();
            t.contains(r#"xmp:Rating="-1""#) && t.contains(r#"xmp:Label="Red""#)
        }),
        "拒绝投影不得扰动颜色标签"
    );
}

#[test]
fn reject_batch_tolerates_per_file_failures() {
    let dir = tempfile::tempdir().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = common::state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);

    let p1 = photo_root.join("DSC_0020.jpg");
    std::fs::write(&p1, b"j1").unwrap();
    let id1 = ins(&db, &p1.to_string_lossy(), None, AssetKind::Photo);
    let p2 = photo_root.join("DSC_0021.jpg");
    std::fs::write(&p2, b"j2").unwrap();
    let id2 = ins(&db, &p2.to_string_lossy(), None, AssetKind::Photo);

    // 人为制造 id2 的边车写失败：把边车路径占成目录（rename 落盘必败）
    std::fs::create_dir_all(xmp::sidecar_path(&p2)).unwrap();

    // 批量拒绝：单文件失败不中断，正常文件照常投影
    let n = fetch_asset_reject_set(&state, &[id1, id2], true).unwrap();
    assert_eq!(n, 2, "DB 更新两行");
    let sidecar1 = xmp::sidecar_path(&p1);
    assert!(
        wait_until(Duration::from_secs(10), || {
            sidecar1.is_file()
                && std::fs::read_to_string(&sidecar1)
                    .unwrap()
                    .contains(r#"xmp:Rating="-1""#)
        }),
        "失败不得中断批量：正常文件仍投影"
    );
    assert!(
        xmp::sidecar_path(&p2).is_dir(),
        "失败文件保持目录占位（边车未写坏）"
    );
    // 清理目录占位
    std::fs::remove_dir(xmp::sidecar_path(&p2)).unwrap();
}
