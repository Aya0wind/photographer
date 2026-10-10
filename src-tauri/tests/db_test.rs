//! Db / journal 仓储行为测试（M1 Task 2，tempdir 真实文件库）。
//!
//! 覆盖：单版本 schema 幂等、photos_libraries 登记表与 root 互斥校验、assets
//! 多照片库新列、job 生命周期、job_files upsert 覆盖、
//! pending 过滤、按状态分组计数、jobs/logs keyset 分页边界、(size, xxhash)
//! 查重、路径存在性、WAL 两连接并发读写、FileState/AssetKind 存储格式 roundtrip。
//!
//! lib.rs 的 `mod db;` 暂为私有（T9 IPC 接线时再导出），集成测试通过
//! `#[path]` 把 `src/db` 及其依赖的 `src/events` 直接编译进测试 crate，
//! 保证被测代码就是生产源码本体（events 自带的单测会随本目标重复执行一次）。

#[path = "../src/platform/mod.rs"]
#[allow(dead_code)]
mod platform;
#[path = "../src/db/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod db;
#[path = "../src/devices/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树（events::DeviceScanned 引用）
mod devices;
#[path = "../src/events/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod events;
#[path = "../src/metadata/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树（db::update_asset_deep_exif 引用）
mod metadata;
#[path = "../src/thumbs/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树（metadata::phash 引用）
mod thumbs;

use chrono::DateTime;
use rusqlite::params;

use db::{AssetRow, Db, JobFileRow, JobRow};
use events::{AssetKind, FileState};

fn temp_db() -> (tempfile::TempDir, Db) {
    // 单版本 schema（2026-10-09）：打开即幂等建表。
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db = Db::open(&dir.path().join("library.db")).expect("failed to open db");
    (dir, db)
}

fn ids(rows: &[JobRow]) -> Vec<i64> {
    rows.iter().map(|job| job.id).collect()
}

fn log_ids(rows: &[db::LogRow]) -> Vec<i64> {
    rows.iter().map(|log| log.id).collect()
}

fn job_file(job_id: i64, src: &str, state: FileState) -> JobFileRow {
    JobFileRow {
        job_id,
        src: src.to_string(),
        dst: format!("I:/SmartPhoto/{src}"),
        size: 2048,
        state,
        error: None,
        xxhash: None,
        dst2: String::new(),
    }
}

fn asset(path: &str, size: u64, xxhash: u64, kind: AssetKind) -> AssetRow {
    AssetRow {
        path: path.to_string(),
        filename: "IMG_0001.JPG".to_string(),
        size,
        mtime: "2026-09-18T00:00:00.000Z".to_string(),
        xxhash,
        kind,
        captured_at: Some("2026-09-01T10:20:30.000Z".to_string()),
        camera: Some("Sony A7M4".to_string()),
        source: "volume:E:".to_string(),
        created_at: "2026-09-18T08:00:00.000Z".to_string(),
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
    }
}

fn asset_row_id(db: &Db, path: &str) -> i64 {
    db.0.query_row(
        "SELECT id FROM assets WHERE path = ?1",
        params![path],
        |row| row.get(0),
    )
    .expect("failed to read asset id")
}

#[test]
fn schema_is_single_version_and_idempotent_across_reopens() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("library.db");

    // 打开即建表：全部对象一份就位，不依赖 user_version/migrate
    {
        let db = Db::open(&path).expect("open");
        let tables: i64 = db
            .0
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN \
                 ('photos_libraries', 'assets', 'jobs', 'job_files', 'logs')",
                [],
                |row| row.get(0),
            )
            .expect("count tables");
        assert_eq!(tables, 5, "photos_libraries 随首开建立");
        let indexes: i64 =
            db.0.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
                [],
                |row| row.get(0),
            )
            .expect("count indexes");
        assert_eq!(
            indexes, 27,
            "assets 7（captured_at/xxhash/size_filename/burst/trash + library/volume_file \
             两新索引）+ job_files 1 + logs 1 + index_tasks 2 + faces 2 + album_dir_name 1 \
             + album_item 3 + group_asset 1 + export_job 2 + album_export_job 1 + culling 3 \
             + regions 2 + asset_regions 1"
        );
    }

    // 重开幂等：每张表/索引仍只存在一份
    let db = Db::open(&path).expect("reopen");
    let tables: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN \
             ('photos_libraries', 'assets', 'jobs', 'job_files', 'logs')",
            [],
            |row| row.get(0),
        )
        .expect("count tables");
    assert_eq!(tables, 5, "重复打开不得重复建表");
    let indexes: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
            [],
            |row| row.get(0),
        )
        .expect("count indexes");
    assert_eq!(indexes, 27, "重复打开不得重复建索引");
}

/// assets 三新列 + 登记指纹列随 schema 就位且默认值正确（NOT NULL 列无
/// NULL 态；可空列 NULL）——INSERT 未显式给值时走列默认。
#[test]
fn assets_multi_library_columns_default_values() {
    let (_dir, db) = temp_db();
    db.insert_asset(&asset("I:/p/a.jpg", 10, 1, AssetKind::Photo))
        .expect("insert");
    let (library_id, missing, xmp_dirty, volume_serial, file_id): (
        Option<String>,
        i64,
        i64,
        Option<i64>,
        Option<String>,
    ) = db
        .0
        .query_row(
            "SELECT library_id, missing, xmp_dirty, volume_serial, file_id \
             FROM assets WHERE path = 'I:/p/a.jpg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .expect("select new columns");
    assert_eq!(library_id, None);
    assert_eq!(missing, 0);
    assert_eq!(xmp_dirty, 0);
    assert_eq!(volume_serial, None);
    assert_eq!(file_id, None);
}

/// 新列经 AssetRow 写入/读回 roundtrip（insert_asset → asset_by_id）。
#[test]
fn assets_multi_library_fields_roundtrip() {
    let (_dir, db) = temp_db();
    let mut row = asset("I:/p/b.jpg", 20, 2, AssetKind::Photo);
    row.library_id = Some("lib-uuid-1".into());
    row.missing = 1;
    row.xmp_dirty = 1;
    row.volume_serial = Some(0x1234_5678);
    row.file_id = Some("00ff...".into());
    db.insert_asset(&row).expect("insert");
    let id = db.asset_id_by_path("I:/p/b.jpg").unwrap().unwrap();
    let read = db.asset_by_id(id).unwrap().unwrap();
    assert_eq!(read.library_id.as_deref(), Some("lib-uuid-1"));
    assert_eq!(read.missing, 1);
    assert_eq!(read.xmp_dirty, 1);
    assert_eq!(read.volume_serial, Some(0x1234_5678));
    assert_eq!(read.file_id.as_deref(), Some("00ff..."));
}

/// 登记指纹查重（§三 两级识别第一级）：同卷同 file id 命中既有资产，
/// 不同 file id / 不同卷不命中。
#[test]
fn find_asset_by_volume_file_id_hit_and_miss() {
    let (_dir, db) = temp_db();
    let mut a = asset("I:/p/hard.jpg", 30, 3, AssetKind::Photo);
    a.volume_serial = Some(77);
    a.file_id = Some("abcd".into());
    db.insert_asset(&a).expect("insert");
    let expected = asset_row_id(&db, "I:/p/hard.jpg");

    assert_eq!(
        db.find_asset_by_volume_file_id(77, "abcd").expect("query"),
        Some((expected, "I:/p/hard.jpg".to_string())),
        "同卷同 file id = 同一物理文件（硬链接），命中"
    );
    assert_eq!(
        db.find_asset_by_volume_file_id(77, "zzzz").expect("query"),
        None,
        "同卷不同 file id 不命中"
    );
    assert_eq!(
        db.find_asset_by_volume_file_id(88, "abcd").expect("query"),
        None,
        "不同卷不命中"
    );
}

// ---------------------------------------------------------------------------
// photos_libraries 登记表（单数据库多照片库 §二 / §八-6）
// ---------------------------------------------------------------------------

fn roots_dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("failed to create temp dir")
}

/// 登记 → 列表 → 统计缓存刷新 → 状态翻转 → 移除的全链路。
#[test]
fn photos_library_register_list_stats_and_remove() {
    let dir = roots_dir();
    let (_dir, db) = temp_db();
    let database_dir = dir.path().join("appdata");
    std::fs::create_dir_all(&database_dir).unwrap();
    let root1 = dir.path().join("photos-2026");
    std::fs::create_dir_all(&root1).unwrap();

    let row = db
        .photos_library_register("2026 主库", &root1.to_string_lossy(), &database_dir)
        .expect("register");
    assert!(!row.id.is_empty(), "登记时生成 uuid");
    assert_eq!(row.status, "online");
    assert_eq!(row.asset_count, 0);
    assert_eq!(row.size_bytes, 0);

    // 第二库登记在不同分支
    let root2 = dir.path().join("photos-archive");
    std::fs::create_dir_all(&root2).unwrap();
    let row2 = db
        .photos_library_register("归档库", &root2.to_string_lossy(), &database_dir)
        .expect("register 2");
    let list = db.photos_library_list().expect("list");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, row.id, "登记序（created_at, id）");

    // 库内登记两资产 + 一回收站资产 + 一视频 → 统计口径只算 photo/raw 且不在回收站
    let mut a = asset("I:/photos-2026/a.jpg", 100, 11, AssetKind::Photo);
    a.library_id = Some(row.id.clone());
    db.insert_asset(&a).unwrap();
    let mut b = asset("I:/photos-2026/b.cr3", 50, 12, AssetKind::Raw);
    b.library_id = Some(row.id.clone());
    db.insert_asset(&b).unwrap();
    let mut trashed = asset("I:/photos-2026/c.jpg", 70, 13, AssetKind::Photo);
    trashed.library_id = Some(row.id.clone());
    db.insert_asset(&trashed).unwrap();
    let tid = asset_row_id(&db, "I:/photos-2026/c.jpg");
    db.0
        .execute("UPDATE assets SET in_trash = 1 WHERE id = ?1", [tid])
        .unwrap();
    let mut video = asset("I:/photos-2026/d.mp4", 500, 14, AssetKind::Video);
    video.library_id = Some(row.id.clone());
    db.insert_asset(&video).unwrap();
    db.photos_library_refresh_stats(&row.id).expect("refresh");
    let refreshed = db.photos_library_get(&row.id).unwrap().unwrap();
    assert_eq!(refreshed.asset_count, 2, "photo/raw 各一，回收站与视频不计");
    assert_eq!(refreshed.size_bytes, 150);

    // 状态翻转（整库离线标记）
    db.photos_library_set_status(&row.id, "offline").unwrap();
    assert_eq!(db.photos_library_get(&row.id).unwrap().unwrap().status, "offline");

    // 移除登记不连记录：资产行保留（归属 id 悬空，应用层决定语义）
    let deleted = db
        .photos_library_remove(&row.id, false)
        .expect("remove keep records");
    assert_eq!(deleted, 0);
    assert!(db.photos_library_get(&row.id).unwrap().is_none());
    let remaining: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets WHERE library_id = ?1", [&row.id], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(remaining, 4, "不连记录删时资产行原样保留");

    // 移除登记连记录删：库内资产行消失，他库不受影响
    let deleted = db
        .photos_library_remove(&row2.id, true)
        .expect("remove with records");
    assert_eq!(deleted, 0, "库 2 无资产");
    assert!(db.photos_library_list().unwrap().is_empty());
    assert!(
        db.photos_library_remove("no-such-id", true).is_err(),
        "移除不存在的库必须报错"
    );
}

/// 移除登记连记录删的级联：库内资产行删除带走索引任务等关联行。
#[test]
fn photos_library_remove_with_records_cascades_asset_rows() {
    let (_dir, db) = temp_db();
    let dir = roots_dir();
    let database_dir = dir.path().join("appdata");
    std::fs::create_dir_all(&database_dir).unwrap();
    let root = dir.path().join("lib-photos");
    std::fs::create_dir_all(&root).unwrap();
    let row = db
        .photos_library_register("库", &root.to_string_lossy(), &database_dir)
        .unwrap();
    let mut a = asset("I:/lib-photos/x.jpg", 10, 21, AssetKind::Photo);
    a.library_id = Some(row.id.clone());
    db.insert_asset(&a).unwrap();
    let id = asset_row_id(&db, "I:/lib-photos/x.jpg");
    let tasks: i64 = db
        .0
        .query_row(
            "SELECT COUNT(*) FROM index_tasks WHERE asset_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(tasks > 0, "photo 入库自带 thumb/eyes/blur 任务");

    let deleted = db.photos_library_remove(&row.id, true).unwrap();
    assert_eq!(deleted, 1);
    let assets: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
    assert_eq!(assets, 0);
    let tasks: i64 = db
        .0
        .query_row("SELECT COUNT(*) FROM index_tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(tasks, 0, "资产行删除级联清索引任务");
}

/// root 互斥校验（§八-6）：库间互斥 + 与数据库目录互斥 + 盘根拒绝 +
/// 大小写/斜杠不敏感 + 反向包含（父目录吞并既有库）。
#[test]
fn photos_library_root_validation_rejects_overlap() {
    let dir = roots_dir();
    let database_dir = dir.path().join("appdata");
    std::fs::create_dir_all(&database_dir).unwrap();
    // 既有库 root 独立分支（其父目录不含数据库目录，反向包含用例才能
    // 命中「库间重叠」而非「数据库目录重叠」）
    let photos_home = dir.path().join("photos-home");
    let root = photos_home.join("main");
    std::fs::create_dir_all(&root).unwrap();
    let root_str = root.to_string_lossy().into_owned();

    // 与数据库目录相同 / 互相包含
    for bad in [
        database_dir.clone(),
        database_dir.join("thumbs"),
        dir.path().to_path_buf(), // 数据库目录的父（包含数据库目录）
    ] {
        let err = db::libraries::validate_photos_library_root(&[], &database_dir, &bad)
            .expect_err("与数据库目录重叠必须拒绝");
        assert!(err.contains("数据库目录"), "{bad:?} 应拒：{err}");
    }

    // 与已有库 root 相同 / 互相包含（大小写与斜杠形态不敏感）
    let existing = vec![("主库".to_string(), root_str.replace('\\', "/"))];
    for bad in [
        std::path::PathBuf::from(root_str.clone()),
        std::path::PathBuf::from(root_str.to_lowercase()),
        root.join("sub"),
    ] {
        let err = db::libraries::validate_photos_library_root(&existing, &database_dir, &bad)
            .expect_err("库间重叠必须拒绝");
        assert!(err.contains("主库"), "{bad:?} 应拒：{err}");
    }
    // 反向包含（新 root 是已有库 root 的父目录，吞并既有库）同样拒绝
    let err = db::libraries::validate_photos_library_root(
        &existing,
        &database_dir,
        &photos_home,
    )
    .expect_err("新 root 包含已有库 root 必须拒绝");
    assert!(err.contains("主库"), "{err}");

    // 盘根拒绝（临时目录所在盘的根：ancestors 最后一个就是卷根）
    {
        let volume_root = root.ancestors().last().unwrap().to_path_buf();
        let err = db::libraries::validate_photos_library_root(&[], &database_dir, &volume_root)
            .expect_err("盘根必须拒绝");
        assert!(
            err.contains("根目录"),
            "{} 应拒：{err}",
            volume_root.display()
        );
    }

    // 合法：不同分支、非数据库目录、非盘根
    db::libraries::validate_photos_library_root(
        &existing,
        &database_dir,
        &dir.path().join("other-photos"),
    )
    .expect("互不重叠的 root 应通过");
}

/// 登记闸门内置校验：register 拒绝重叠 root。
#[test]
fn photos_library_register_validates_root() {
    let dir = roots_dir();
    let (_dir, db) = temp_db();
    let database_dir = dir.path().join("appdata");
    std::fs::create_dir_all(&database_dir).unwrap();
    let root = dir.path().join("photos");
    std::fs::create_dir_all(&root).unwrap();

    db
        .photos_library_register("主库", &root.to_string_lossy(), &database_dir)
        .unwrap();
    // 同 root 再登记（他库）被拒
    let err = db
        .photos_library_register("重复库", &root.to_string_lossy(), &database_dir)
        .expect_err("同 root 重复登记必须拒绝");
    assert!(err.contains("主库"), "{err}");
    // 数据库目录内登记被拒（防把 thumbs/ 登记进库）
    let err = db
        .photos_library_register(
            "数据库内库",
            &database_dir.join("inside").to_string_lossy(),
            &database_dir,
        )
        .expect_err("数据库目录内登记必须拒绝");
    assert!(err.contains("数据库目录"), "{err}");


}

#[test]
fn job_lifecycle_running_then_done() {
    let (_dir, db) = temp_db();

    let job_id = db
        .create_job("import", "E:", "SanDisk 64G", 10, 1024)
        .expect("create job");
    assert!(job_id > 0);

    let rows = db.jobs_page(0, 10).expect("jobs page");
    assert_eq!(rows.len(), 1);
    let job = &rows[0];
    assert_eq!(job.id, job_id);
    assert_eq!(job.kind, "import");
    assert_eq!(job.device_id, "E:");
    assert_eq!(job.device_name, "SanDisk 64G");
    assert_eq!(job.status, "running");
    assert_eq!(job.total_files, 10);
    assert_eq!(job.total_bytes, 1024);
    assert_eq!(job.stats_json, None);
    assert_eq!(job.finished_at, None);
    DateTime::parse_from_rfc3339(&job.started_at).expect("started_at 是 RFC3339");

    db.finish_job(job_id, "done", r#"{"doneFiles":10}"#)
        .expect("finish job");
    let rows = db.jobs_page(0, 10).expect("jobs page");
    let job = &rows[0];
    assert_eq!(job.status, "done");
    assert_eq!(job.stats_json.as_deref(), Some(r#"{"doneFiles":10}"#));
    let finished = job.finished_at.as_deref().expect("finished_at 已写入");
    DateTime::parse_from_rfc3339(finished).expect("finished_at 是 RFC3339");
}

#[test]
fn finish_job_rejects_status_outside_check() {
    let (_dir, db) = temp_db();
    let job_id = db.create_job("import", "E:", "dev", 1, 1).expect("create");
    assert!(
        db.finish_job(job_id, "exploded", "{}").is_err(),
        "CHECK 白名单之外的状态必须被 SQLite 拒绝"
    );
}

#[test]
fn upsert_job_file_overwrites_same_pk() {
    let (_dir, db) = temp_db();
    let job_id = db.create_job("import", "E:", "dev", 1, 1).expect("create");

    db.upsert_job_file(&job_file(
        job_id,
        "DCIM/100EOSCK/IMG_0001.JPG",
        FileState::Pending,
    ))
    .expect("insert job file");

    let mut updated = job_file(job_id, "DCIM/100EOSCK/IMG_0001.JPG", FileState::Verified);
    updated.dst = "I:/SmartPhoto/2026/IMG_0001_1.JPG".to_string();
    updated.xxhash = Some(0xdead_beef);
    db.upsert_job_file(&updated).expect("upsert overwrite");

    // PK(job_id, src) 覆盖：仍只有一行，且读回的是新值
    let (dst, state, xxhash): (String, String, Option<i64>) =
        db.0.query_row(
            "SELECT dst, state, xxhash FROM job_files WHERE job_id = ?1",
            params![job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("select job file");
    assert_eq!(dst, "I:/SmartPhoto/2026/IMG_0001_1.JPG");
    assert_eq!(state, "verified");
    assert_eq!(xxhash, Some(0xdead_beef_i64));
}

#[test]
fn pending_job_files_returns_pending_and_failed_only() {
    let (_dir, db) = temp_db();
    let job_id = db.create_job("import", "E:", "dev", 4, 0).expect("create");

    let mut failed = job_file(job_id, "b.JPG", FileState::Failed);
    failed.error = Some("target disk offline".to_string());
    for row in [
        job_file(job_id, "a.JPG", FileState::Pending),
        failed,
        job_file(job_id, "c.JPG", FileState::Verified),
        job_file(job_id, "d.JPG", FileState::Skipped),
    ] {
        db.upsert_job_file(&row).expect("insert");
    }

    let pending = db.pending_job_files(job_id).expect("pending");
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].src, "a.JPG");
    assert_eq!(pending[0].state, FileState::Pending);
    assert_eq!(pending[1].src, "b.JPG");
    assert_eq!(pending[1].state, FileState::Failed);
    assert_eq!(pending[1].error.as_deref(), Some("target disk offline"));
    assert_eq!(pending[1].size, 2048);
}

#[test]
fn job_file_counts_group_by_state() {
    let (_dir, db) = temp_db();
    let job_id = db.create_job("import", "E:", "dev", 8, 0).expect("create");

    let states = [
        (FileState::Pending, 3),
        (FileState::Verified, 2),
        (FileState::Skipped, 1),
        (FileState::Failed, 2),
    ];
    for (i, (state, n)) in states.iter().enumerate() {
        for k in 0..*n {
            db.upsert_job_file(&job_file(job_id, &format!("s{i}_{k}"), *state))
                .expect("insert");
        }
    }

    let counts = db.job_file_counts(job_id).expect("counts");
    assert_eq!(counts.len(), 4, "只应有非零分组：{counts:?}");
    assert!(counts.contains(&(FileState::Pending, 3)));
    assert!(counts.contains(&(FileState::Verified, 2)));
    assert!(counts.contains(&(FileState::Skipped, 1)));
    assert!(counts.contains(&(FileState::Failed, 2)));
    assert!(!counts.iter().any(|(state, _)| *state == FileState::Copying));
}

#[test]
fn jobs_page_keyset_boundaries() {
    let (_dir, db) = temp_db();
    for i in 0..25u64 {
        db.create_job("import", "E:", &format!("device {i}"), i, i * 100)
            .expect("create");
    }

    let page1 = db.jobs_page(0, 10).expect("page1");
    assert_eq!(ids(&page1), (1..=10).collect::<Vec<_>>());
    let page2 = db
        .jobs_page(page1.last().expect("non-empty").id, 10)
        .expect("page2");
    assert_eq!(ids(&page2), (11..=20).collect::<Vec<_>>());
    let page3 = db
        .jobs_page(page2.last().expect("non-empty").id, 10)
        .expect("page3");
    assert_eq!(ids(&page3), (21..=25).collect::<Vec<_>>());
    let page4 = db
        .jobs_page(page3.last().expect("non-empty").id, 10)
        .expect("page4");
    assert!(page4.is_empty(), "游标到末尾必须返回空页");

    // 精确边界：after_id 指向最后一行时不再返回
    let tail = db.jobs_page(24, 10).expect("tail");
    assert_eq!(ids(&tail), vec![25]);
}

#[test]
fn find_asset_by_size_xxh_scoped_to_library() {
    let (dir, db) = temp_db();
    // 两个照片库同内容指纹（跨库重复合法，§四）：查重只在同库内命中
    //（root 必须在数据库目录之外——§八-6 互斥）
    let photos_a = tempfile::tempdir().unwrap();
    let photos_b = tempfile::tempdir().unwrap();
    let root_a = photos_a.path().to_path_buf();
    let root_b = photos_b.path().to_path_buf();
    let lib_a = db
        .photos_library_register("库A", &root_a.to_string_lossy(), dir.path())
        .unwrap();
    let lib_b = db
        .photos_library_register("库B", &root_b.to_string_lossy(), dir.path())
        .unwrap();
    let mut in_a = asset("I:/PhotosA/2026/a.jpg", 123, 77, AssetKind::Photo);
    in_a.library_id = Some(lib_a.id.clone());
    let mut in_b = asset("I:/PhotosB/2026/a.jpg", 123, 77, AssetKind::Photo);
    in_b.library_id = Some(lib_b.id.clone());
    let mut other_fingerprint = asset("I:/PhotosA/2026/b.jpg", 123, 88, AssetKind::Raw);
    other_fingerprint.library_id = Some(lib_a.id.clone());
    db.insert_asset(&in_a).expect("insert a");
    db.insert_asset(&in_b).expect("insert b");
    db.insert_asset(&other_fingerprint).expect("insert b2");

    // 库A 命中自身指纹；库B 同指纹也命中（各库独立）；错库/错指纹不命中
    let expected_a = asset_row_id(&db, "I:/PhotosA/2026/a.jpg");
    let expected_b = asset_row_id(&db, "I:/PhotosB/2026/a.jpg");
    assert_eq!(
        db.find_asset_by_size_xxh(&lib_a.id, 123, 77).expect("query"),
        Some(expected_a)
    );
    assert_eq!(
        db.find_asset_by_size_xxh(&lib_b.id, 123, 77).expect("query"),
        Some(expected_b)
    );
    assert_eq!(
        db.find_asset_by_size_xxh(&lib_a.id, 123, 999).expect("query"),
        None
    );
    assert_eq!(
        db.find_asset_by_size_xxh(&lib_a.id, 999, 77).expect("query"),
        None
    );
}

#[test]
fn asset_path_exists_and_replace_keeps_single_row() {
    let (_dir, db) = temp_db();
    assert!(!db.asset_path_exists("I:/photos/x.jpg").expect("exists"));

    db.insert_asset(&asset("I:/photos/x.jpg", 10, 1, AssetKind::Photo))
        .expect("insert");
    assert!(db.asset_path_exists("I:/photos/x.jpg").expect("exists"));

    // 同路径再导入（内容已变化）：INSERT OR REPLACE 覆盖，不产生第二行
    db.insert_asset(&asset("I:/photos/x.jpg", 42, 2, AssetKind::Photo))
        .expect("replace");
    let n: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |row| row.get(0))
            .expect("count assets");
    assert_eq!(n, 1);
    assert!(db.asset_path_exists("I:/photos/x.jpg").expect("exists"));
}

#[test]
fn asset_kind_stored_as_camel_case_string() {
    let (_dir, db) = temp_db();
    let kinds = [
        (AssetKind::Photo, "photo"),
        (AssetKind::Raw, "raw"),
        (AssetKind::Video, "video"),
        (AssetKind::Other, "other"),
    ];
    for (i, (kind, _)) in kinds.iter().enumerate() {
        db.insert_asset(&asset(
            &format!("I:/photos/k{i}.bin"),
            1,
            i as u64 + 1,
            *kind,
        ))
        .expect("insert");
    }
    for (i, (_, expected)) in kinds.iter().enumerate() {
        let stored: String =
            db.0.query_row(
                "SELECT kind FROM assets WHERE path = ?1",
                params![format!("I:/photos/k{i}.bin")],
                |row| row.get(0),
            )
            .expect("select kind");
        assert_eq!(stored, *expected);
    }
}

#[test]
fn logs_page_cursor_pagination_and_job_isolation() {
    let (_dir, db) = temp_db();
    let job1 = db.create_job("import", "E:", "dev1", 0, 0).expect("job1");
    let job2 = db.create_job("import", "F:", "dev2", 0, 0).expect("job2");

    for i in 0..25 {
        let level = if i % 2 == 0 { "info" } else { "warn" };
        db.append_log(level, Some(job1), &format!("message {i}"))
            .expect("append");
    }
    db.append_log("error", Some(job2), "other job")
        .expect("append");
    db.append_log("info", None, "global").expect("append");

    let page1 = db.logs_page(job1, 0, 10).expect("page1");
    assert_eq!(log_ids(&page1), (1..=10).collect::<Vec<_>>());
    assert!(page1.iter().all(|log| log.message.starts_with("message ")));
    assert_eq!(page1[0].level, "info");
    assert_eq!(page1[1].level, "warn");
    assert_eq!(page1[0].job_id, Some(job1));
    DateTime::parse_from_rfc3339(&page1[0].ts).expect("ts 是 RFC3339");

    let page2 = db
        .logs_page(job1, page1.last().expect("non-empty").id, 10)
        .expect("page2");
    assert_eq!(log_ids(&page2), (11..=20).collect::<Vec<_>>());
    let page3 = db
        .logs_page(job1, page2.last().expect("non-empty").id, 10)
        .expect("page3");
    assert_eq!(log_ids(&page3), (21..=25).collect::<Vec<_>>());
    assert!(db.logs_page(job1, 25, 10).expect("page4").is_empty());

    // 任务隔离：job1 的分页不包含 job2 / 全局日志
    let all_job1 = db.logs_page(job1, 0, 100).expect("all");
    assert_eq!(log_ids(&all_job1).len(), 25);
}

#[test]
fn wal_two_connections_concurrent_read_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("library.db");
    let writer = Db::open(&path).expect("open writer");
    let job_id = writer
        .create_job("import", "E:", "dev", 1, 1)
        .expect("create");

    // 写连接持有未提交事务
    writer.0.execute_batch("BEGIN IMMEDIATE").expect("begin");
    writer
        .0
        .execute(
            "INSERT INTO logs (ts, level, job_id, message) \
             VALUES ('2026-09-18T00:00:00.000Z', 'info', ?1, 'uncommitted')",
            params![job_id],
        )
        .expect("insert in txn");

    // 读连接在写事务进行期间打开并查询：WAL 下不阻塞、读到最近已提交快照
    let reader = Db::open(&path).expect("open reader");
    let mode: String = reader
        .0
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .expect("journal mode");
    assert_eq!(mode.to_lowercase(), "wal");

    let jobs = reader.jobs_page(0, 10).expect("read during write txn");
    assert_eq!(jobs.len(), 1, "已提交快照可见");
    let logs: i64 = reader
        .0
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .expect("count logs");
    assert_eq!(logs, 0, "未提交行不可见");

    // 写连接提交后，读连接无需重开即可看到
    writer.0.execute_batch("COMMIT").expect("commit");
    let logs: i64 = reader
        .0
        .query_row("SELECT COUNT(*) FROM logs", [], |row| row.get(0))
        .expect("count logs");
    assert_eq!(logs, 1);
}

#[test]
fn foreign_keys_are_enforced() {
    let (_dir, db) = temp_db();
    let orphan = job_file(999, "DCIM/orphan.JPG", FileState::Pending);
    assert!(
        db.upsert_job_file(&orphan).is_err(),
        "job_files.job_id 必须引用已存在的 jobs.id"
    );
}

#[test]
fn file_state_roundtrip_all_variants() {
    let (_dir, db) = temp_db();
    let job_id = db.create_job("import", "E:", "dev", 5, 0).expect("create");

    let variants = [
        (FileState::Pending, "pending"),
        (FileState::Copying, "copying"),
        (FileState::Verified, "verified"),
        (FileState::Skipped, "skipped"),
        (FileState::Failed, "failed"),
    ];
    for (i, (state, stored)) in variants.iter().enumerate() {
        db.upsert_job_file(&job_file(job_id, &format!("v{i}"), *state))
            .expect("insert");
        let text: String =
            db.0.query_row(
                "SELECT state FROM job_files WHERE job_id = ?1 AND src = ?2",
                params![job_id, format!("v{i}")],
                |row| row.get(0),
            )
            .expect("select state");
        assert_eq!(text, *stored, "列值必须与 serde camelCase 形式一致");
    }

    // counts 把列值还原为枚举：5 个状态各一行
    let counts = db.job_file_counts(job_id).expect("counts");
    assert_eq!(counts.len(), 5);
    for (state, _) in &variants {
        assert!(counts.contains(&(*state, 1)), "缺少 {state:?}：{counts:?}");
    }
}
