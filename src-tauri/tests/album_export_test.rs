//! M6 相册/子组「导出为文件夹」（2026-10-09 §六，Photographer → LR 互操作）：
//! 同卷硬链接（秒级零拷贝）+ 全量 XMP 边车（星级/颜色/关键字）、源 missing
//! 成员跳过（§五，total-done 呈现）、目标在照片库内不禁止（DTO 携带提示
//! 文案）、取消软信号终态 cancelled。
//!
//! 跨卷拷贝分支说明：测试临时目录恒在同一卷，无法稳定构造异卷目标——
//! 该分支 = `same_filesystem` 判否/硬链接失败回退 `std::fs::copy`（标准库
//! 调用），本文件以「硬链接可用性探针」按环境真值断言 linked 计数（FAT
//! 临时卷上自动退化为拷贝语义断言）。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, geo, import, index, ipc, metadata, settings, tasks, thumbs,
};

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use db::AssetRow;
use events::{AppEvent, AssetKind};

/// 进程级单活跃槽是全局状态——本文件测试串行（并行线程会互斥拒单）。
static SERIAL: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// 脚手架
// ---------------------------------------------------------------------------

struct Fixture {
    _serial: std::sync::MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    state: ipc::AppState,
    db: db::Db,
    photo_root: PathBuf,
}

fn setup() -> Fixture {
    // 单活跃槽守卫先落锁，再建库（防并行线程先占到槽）
    let _serial = SERIAL.lock().unwrap();
    let (dir, state, database) = common::library_fixture();
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&photo_root).unwrap();
    Fixture {
        _serial,
        _dir: dir,
        state,
        db: database,
        photo_root,
    }
}

/// 落一张照片资产（磁盘文件 + 资产行）。返回资产 id。
fn ins_photo(db: &db::Db, dir: &Path, name: &str, bytes: &[u8]) -> i64 {
    let path = dir.join(name);
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(&path, bytes).unwrap();
    db.insert_asset(&AssetRow {
        path: path.to_string_lossy().into_owned(),
        filename: name.to_string(),
        size: bytes.len() as u64,
        mtime: "2026-09-01T00:00:00.000Z".to_string(),
        xxhash: bytes.len() as u64,
        kind: AssetKind::Photo,
        captured_at: Some("2026-06-01T10:00:00.000Z".to_string()),
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
        library_id: None,
        missing: 0,
        xmp_dirty: 0,
        volume_serial: None,
        file_id: None,
    })
    .unwrap();
    db.0
        .query_row("SELECT id FROM assets WHERE path = ?1", [&path.to_string_lossy()], |r| {
            r.get(0)
        })
        .unwrap()
}

/// 硬链接可用性探针（FAT/exFAT 临时卷上 false——导出应全程拷贝、linked=0）。
fn hardlink_supported(dir: &Path) -> bool {
    let a = dir.join("__probe_a");
    let b = dir.join("__probe_b");
    std::fs::write(&a, b"probe").unwrap();
    let ok = std::fs::hard_link(&a, &b).is_ok();
    let _ = std::fs::remove_file(&a);
    let _ = std::fs::remove_file(&b);
    ok
}

/// 轮询任务到达终态（fetch 走 supervisor 线程），并等活跃槽释放
///（收尾事件/落库先于槽摘除——不留窗口给下一个测试的拒单竞态）。
fn wait_terminal(db: &db::Db, job_id: i64) -> db::AlbumExportJobRow {
    let deadline = Instant::now() + Duration::from_secs(10);
    let row = loop {
        let row = db
            .album_export_job_latest()
            .unwrap()
            .expect("任务行存在");
        assert_eq!(row.id, job_id, "latest 应是刚建的任务");
        if matches!(row.status.as_str(), "done" | "cancelled" | "error") {
            break row;
        }
        assert!(Instant::now() < deadline, "导出任务超时未收尾: {row:?}");
        std::thread::sleep(Duration::from_millis(10));
    };
    while ipc::album_export::active_job_id().is_some_and(|id| id == job_id) {
        assert!(Instant::now() < deadline, "活跃槽超时未释放");
        std::thread::sleep(Duration::from_millis(5));
    }
    row
}

// ---------------------------------------------------------------------------
// 硬链接 + 全量边车
// ---------------------------------------------------------------------------

#[test]
fn hardlink_export_with_full_sidecar() {
    let fixture = setup();
    let out = fixture._dir.path().join("export");
    let album = fixture.db.album_create("交付册").unwrap();
    let id1 = ins_photo(&fixture.db, &fixture.photo_root, "DSC_0001.jpg", b"jpeg-1");
    let id2 = ins_photo(&fixture.db, &fixture.photo_root, "DSC_0002.NEF", b"nef-2");
    fixture
        .db
        .album_add_assets(album.id, &[id1, id2], Some("成片"))
        .unwrap();
    // 库内真值：id1 = 4 星 + red + 关键字；id2 = 拒绝（投影 -1）。
    fixture
        .db
        .0
        .execute(
            "UPDATE assets SET rating = 4, color_label = 'red' WHERE id = ?1",
            [id1],
        )
        .unwrap();
    fixture
        .db
        .0
        .execute(
            "UPDATE assets SET rejected = 1 WHERE id = ?1",
            [id2],
        )
        .unwrap();
    fixture
        .db
        .0
        .execute(
            "INSERT INTO asset_metadata (asset_id, value) VALUES (?1, ?2)",
            rusqlite::params![id1, r#"{"title":"t","keywords":["wedding","keep"]}"#],
        )
        .unwrap();

    // 事件订阅（收尾事件载荷）
    let mut rx = fixture.state.bus.subscribe();
    let task = ipc::album_export::fetch_album_export_run(
        &fixture.state,
        album.id,
        Some("成片"),
        &out.to_string_lossy(),
    )
    .unwrap();
    assert_eq!(task.status, "queued");
    assert_eq!(task.total, 2);
    assert!(task.warning.is_none(), "库外目标无提示");

    let row = wait_terminal(&fixture.db, task.id);
    assert_eq!(row.status, "done");
    assert_eq!(row.total, 2);
    assert_eq!(row.done, 2);
    assert!(row.error.is_none());
    // linked 计数按硬链接可用性真值断言（同卷 + 支持硬链接 → 全链接）
    let expected_linked = if hardlink_supported(&out) { 2 } else { 0 };
    assert_eq!(row.linked, expected_linked);

    // 收尾事件（ok + exported/linked 计数）
    let mut finished = None;
    let deadline = Instant::now() + Duration::from_secs(5);
    while finished.is_none() && Instant::now() < deadline {
        if let Ok(AppEvent::AlbumExportFinished {
            task_id,
            ok,
            exported,
            linked,
            ..
        }) = rx.try_recv()
        {
            assert_eq!(task_id, task.id);
            finished = Some((ok, exported, linked));
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    let (ok, exported, linked) = finished.expect("应收到 AlbumExportFinished");
    assert!(ok);
    assert_eq!((exported, linked), (2, expected_linked));

    // 本体：内容一致；硬链接两侧同 file id（卷支持登记指纹时）
    let dst1 = out.join("DSC_0001.jpg");
    let dst2 = out.join("DSC_0002.NEF");
    assert_eq!(std::fs::read(&dst1).unwrap(), b"jpeg-1");
    assert_eq!(std::fs::read(&dst2).unwrap(), b"nef-2");
    if expected_linked == 2 {
        if let (Ok(a), Ok(b)) = (
            platform::file_registration_id(&fixture.photo_root.join("DSC_0001.jpg")),
            platform::file_registration_id(&dst1),
        ) {
            assert_eq!(a, b, "硬链接两侧登记指纹（卷序列号+file id）相同");
        }
    }

    // 全量边车：星级/颜色/关键字（id1）
    let sidecar1 = std::fs::read_to_string(out.join("DSC_0001.xmp")).unwrap();
    assert!(sidecar1.contains(r#"xmp:Rating="4""#));
    assert!(sidecar1.contains(r#"xmp:Label="Red""#), "小写 token 升为 LR 标准色名");
    assert!(sidecar1.contains("<rdf:li>wedding</rdf:li>"));
    assert!(sidecar1.contains("<rdf:li>keep</rdf:li>"));
    assert!(sidecar1.contains("<dc:subject>"), "关键字走 dc:subject 容器");
    // 拒绝投影（id2）：-1 + 无颜色字段
    let sidecar2 = std::fs::read_to_string(out.join("DSC_0002.xmp")).unwrap();
    assert!(sidecar2.contains(r#"xmp:Rating="-1""#));
    assert!(!sidecar2.contains("xmp:Label"));
}

/// 源旁已有边车做基底：LR 开发设置字节保留，三字段覆写为库内真值。
#[test]
fn sidecar_base_from_source_preserves_lr_fields() {
    let fixture = setup();
    let out = fixture._dir.path().join("export");
    let album = fixture.db.album_create("交付册").unwrap();
    let id = ins_photo(&fixture.db, &fixture.photo_root, "DSC_0003.jpg", b"jpeg-3");
    fixture
        .db
        .album_add_assets(album.id, &[id], None)
        .unwrap();
    // 源旁 LR 边车（旧评分 + 开发设置）
    std::fs::write(
        fixture.photo_root.join("DSC_0003.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmp:Rating="1" crs:Sharpness="25"/></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    fixture
        .db
        .0
        .execute("UPDATE assets SET rating = 5 WHERE id = ?1", [id])
        .unwrap();

    let task = ipc::album_export::fetch_album_export_run(
        &fixture.state,
        album.id,
        None,
        &out.to_string_lossy(),
    )
    .unwrap();
    let row = wait_terminal(&fixture.db, task.id);
    assert_eq!((row.status.as_str(), row.done), ("done", 1));

    let sidecar = std::fs::read_to_string(out.join("DSC_0003.xmp")).unwrap();
    assert!(sidecar.contains(r#"xmp:Rating="5""#), "覆写为库内真值");
    assert!(!sidecar.contains(r#"xmp:Rating="1""#));
    assert!(sidecar.contains(r#"crs:Sharpness="25""#), "LR 开发设置字节保留");
}

// ---------------------------------------------------------------------------
// 容错：源 missing 跳过
// ---------------------------------------------------------------------------

#[test]
fn missing_members_skipped_and_reported_by_total_minus_done() {
    let fixture = setup();
    let out = fixture._dir.path().join("export");
    let album = fixture.db.album_create("交付册").unwrap();
    // 三个成员：在盘 / 文件被删（未标记）/ DB missing 标记
    let ok_id = ins_photo(&fixture.db, &fixture.photo_root, "OK.jpg", b"ok");
    let gone_path = fixture.photo_root.join("gone.jpg");
    std::fs::write(&gone_path, b"g").unwrap();
    let gone_id = ins_photo(&fixture.db, &fixture.photo_root, "gone.jpg", b"g");
    std::fs::remove_file(&gone_path).unwrap();
    let flagged_id = ins_photo(&fixture.db, &fixture.photo_root, "flagged.jpg", b"f");
    std::fs::remove_file(fixture.photo_root.join("flagged.jpg")).unwrap();
    fixture
        .db
        .0
        .execute("UPDATE assets SET missing = 1 WHERE id = ?1", [flagged_id])
        .unwrap();
    fixture
        .db
        .album_add_assets(album.id, &[ok_id, gone_id, flagged_id], None)
        .unwrap();

    let task = ipc::album_export::fetch_album_export_run(
        &fixture.state,
        album.id,
        None,
        &out.to_string_lossy(),
    )
    .unwrap();
    assert_eq!(task.total, 3);
    let row = wait_terminal(&fixture.db, task.id);
    assert_eq!(row.status, "done");
    assert_eq!(row.done, 1, "缺失成员跳过不计 done");
    assert!(row.error.is_none(), "跳过不是失败（§五 定案）");
    assert!(out.join("OK.jpg").is_file());
    assert!(!out.join("gone.jpg").exists());
    assert!(!out.join("flagged.jpg").exists());
    // 边车照写（在盘成员）
    assert!(out.join("OK.xmp").is_file());
}

// ---------------------------------------------------------------------------
// 目标校验：库内目标不禁止 + 提示文案
// ---------------------------------------------------------------------------

#[test]
fn inside_library_target_warns_but_not_forbidden() {
    let fixture = setup();
    let lib_root = fixture._dir.path().join("libroot");
    std::fs::create_dir_all(lib_root.join("2026").join("06")).unwrap();
    common::ensure_photo_library(&fixture.db, &fixture.state.config_dir, &lib_root);
    let out = lib_root.join("lr-export"); // 库内目标

    let album = fixture.db.album_create("交付册").unwrap();
    let id = ins_photo(
        &fixture.db,
        &lib_root.join("2026").join("06"),
        "DSC_0004.jpg",
        b"jpeg-4",
    );
    fixture
        .db
        .album_add_assets(album.id, &[id], None)
        .unwrap();

    let task = ipc::album_export::fetch_album_export_run(
        &fixture.state,
        album.id,
        None,
        &out.to_string_lossy(),
    )
    .unwrap();
    let warning = task.warning.expect("库内目标携带提示文案");
    assert!(warning.contains("扫描忽略"), "提示文案：{warning}");
    assert!(warning.contains("测试照片库"), "点名所在库：{warning}");

    // 不禁止：任务照常执行完成
    let row = wait_terminal(&fixture.db, task.id);
    assert_eq!((row.status.as_str(), row.done), ("done", 1));
    assert!(out.join("DSC_0004.jpg").is_file());

    // status 查询核同样携带提示（库登记真值重算）
    let status = ipc::album_export::fetch_album_export_status(&fixture.state)
        .unwrap()
        .expect("最近任务可见");
    assert_eq!(status.id, task.id);
    assert!(status.warning.is_some());
}

// ---------------------------------------------------------------------------
// 取消：软信号 → cancelled 终态（直调 worker，同步确定性）
// ---------------------------------------------------------------------------

#[test]
fn cancel_before_run_marks_cancelled() {
    let fixture = setup();
    let out = fixture._dir.path().join("export");
    std::fs::create_dir_all(&out).unwrap();
    let album = fixture.db.album_create("交付册").unwrap();
    let id = ins_photo(&fixture.db, &fixture.photo_root, "DSC_0005.jpg", b"jpeg-5");
    fixture
        .db
        .album_add_assets(album.id, &[id], None)
        .unwrap();
    let members = fixture.db.album_export_members(album.id, None).unwrap();
    let job_id = fixture
        .db
        .album_export_job_create(album.id, None, &out.to_string_lossy(), members.len() as u64)
        .unwrap();
    let cancel = Arc::new(AtomicBool::new(true)); // 启动前已取消
    let worker_db = common::open_db(&fixture.state.config_dir);
    ipc::album_export::run_album_export_job(
        worker_db,
        &events::EventBus::new(),
        ipc::album_export::AlbumExportJobRequest {
            job_id,
            output_dir: out.to_string_lossy().into_owned(),
            members,
            cancel,
        },
    );
    let row = fixture.db.album_export_job_latest().unwrap().unwrap();
    assert_eq!(row.status, "cancelled");
    assert_eq!(row.done, 0);
    assert!(row.error.is_none());
    assert!(!out.join("DSC_0005.jpg").exists(), "取消后不落文件");
}
