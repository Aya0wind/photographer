//! Db / journal 仓储行为测试（M1 Task 2，tempdir 真实文件库）。
//!
//! 覆盖：迁移幂等与 user_version 稳定、job 生命周期、job_files upsert 覆盖、
//! pending 过滤、按状态分组计数、jobs/logs keyset 分页边界、(size, xxhash)
//! 查重、路径存在性、WAL 两连接并发读写、FileState/AssetKind 存储格式 roundtrip。
//!
//! lib.rs 的 `mod db;` 暂为私有（T9 IPC 接线时再导出），集成测试通过
//! `#[path]` 把 `src/db` 及其依赖的 `src/events` 直接编译进测试 crate，
//! 保证被测代码就是生产源码本体（events 自带的单测会随本目标重复执行一次）。

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

#[path = "../src/db/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod db;

use chrono::DateTime;
use rusqlite::params;

use db::{AssetRow, Db, JobFileRow, JobRow};
use events::{AssetKind, FileState};

fn temp_db() -> (tempfile::TempDir, Db) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let db = Db::open(&dir.path().join("library.db")).expect("failed to open db");
    db.migrate().expect("failed to migrate");
    (dir, db)
}

fn user_version(db: &Db) -> i64 {
    db.0.query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("failed to read user_version")
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
        orientation: None,
        flash: None,
        metering_mode: None,
        white_balance: None,
        exposure_program: None,
        software: None,
        artist: None,
        gps_lat: None,
        gps_lon: None,
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
fn migration_is_idempotent_and_version_stable() {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("library.db");

    {
        let db = Db::open(&path).expect("open");
        db.migrate().expect("first migrate");
        assert_eq!(user_version(&db), 13);
        db.migrate().expect("second migrate");
        assert_eq!(user_version(&db), 13, "重复迁移不得推进 user_version");
    }

    // 重开已迁移的库：仍是 no-op，且每张表/索引只存在一份
    let db = Db::open(&path).expect("reopen");
    db.migrate().expect("migrate on reopen");
    assert_eq!(user_version(&db), 13);
    let tables: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name IN \
             ('assets', 'jobs', 'job_files', 'logs')",
            [],
            |row| row.get(0),
        )
        .expect("count tables");
    assert_eq!(tables, 4, "重复迁移不得重复建表");
    let indexes: i64 =
        db.0.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
            [],
            |row| row.get(0),
        )
        .expect("count indexes");
    assert_eq!(
        indexes, 10,
        "assets 4（含 size+filename 宽松查重索引）+ job_files 1 + logs 1 + index_tasks 2          + faces 2（asset/cluster，0007）+ burst 1（0012）"
    );
}

#[test]
fn migration_0007_deduplicates_index_tasks_and_keeps_best_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("library.db");
    {
        let db = Db::open(&path).unwrap();
        db.migrate().unwrap();
        db.insert_asset(&asset("X:/dup.jpg", 10, 7, AssetKind::Photo))
            .unwrap();
        let asset_id = asset_row_id(&db, "X:/dup.jpg");
        // 模拟升级前的重复脏数据：去掉 0007 唯一索引，给同一资产再插一条
        // done；迁移应优先保留 done 而不是旧 pending。
        db.0.execute("DROP INDEX idx_index_tasks_kind_asset", [])
            .unwrap();
        db.0.execute(
            "INSERT INTO index_tasks (kind, asset_id, state, attempts, created_at, updated_at) \
             VALUES ('thumb', ?1, 'done', 0, '2026', '2026')",
            [asset_id],
        )
        .unwrap();
        // 0008/0009 的 ALTER ADD COLUMN 不可重放：回卷版本前先摘掉这些列
        //（迁移会原样补回，语义不变）；0010 的表同理（先 DROP 再重建）；
        // 0011 会再删一遍 sha256 列/索引——先按 0001 形态补回
        db.0.execute("DROP TABLE view_history", []).unwrap();
        db.0.execute(
            "ALTER TABLE assets ADD COLUMN sha256 BLOB NOT NULL DEFAULT x'00'",
            [],
        )
        .unwrap();
        db.0.execute("CREATE INDEX idx_assets_sha256 ON assets (sha256)", [])
            .unwrap();
        db.0.execute("ALTER TABLE job_files ADD COLUMN sha256 BLOB", [])
            .unwrap();
        // 0012 同理：phash/burst_id/bursts 均不可重放，回卷前摘除
        db.0.execute("DROP INDEX idx_assets_burst", []).unwrap();
        db.0.execute("ALTER TABLE assets DROP COLUMN burst_id", [])
            .unwrap();
        db.0.execute("ALTER TABLE assets DROP COLUMN phash", [])
            .unwrap();
        db.0.execute("DROP TABLE bursts", []).unwrap();
        // 0013：桶表 CREATE 不可重放
        db.0.execute("DROP TABLE similar_bucket", []).unwrap();
        for col in [
            "orientation",
            "flash",
            "metering_mode",
            "white_balance",
            "exposure_program",
            "software",
            "artist",
            "gps_lat",
            "gps_lon",
            "rating",
            "flagged",
        ] {
            db.0.execute(&format!("ALTER TABLE assets DROP COLUMN {col}"), [])
                .unwrap();
        }
        db.0.pragma_update(None, "user_version", 6).unwrap();
    }

    let db = Db::open(&path).unwrap();
    db.migrate().unwrap();
    assert_eq!(user_version(&db), 13);
    let rows: Vec<(String, String)> =
        db.0.prepare("SELECT kind, state FROM index_tasks")
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
    assert_eq!(rows, vec![("thumb".into(), "done".into())]);
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
fn find_asset_by_size_xxh_hit_and_miss() {
    let (_dir, db) = temp_db();
    db.insert_asset(&asset(
        "I:/SmartPhoto/2026/a.jpg",
        123,
        77,
        AssetKind::Photo,
    ))
    .expect("insert a");
    db.insert_asset(&asset("I:/SmartPhoto/2026/b.jpg", 123, 88, AssetKind::Raw))
        .expect("insert b");
    db.insert_asset(&asset(
        "I:/SmartPhoto/2026/c.jpg",
        456,
        77,
        AssetKind::Video,
    ))
    .expect("insert c");

    // 命中：(size, xxhash) 联合定位到 a，不误中同 size 或同 xxhash 的其他行
    let expected = asset_row_id(&db, "I:/SmartPhoto/2026/a.jpg");
    assert_eq!(
        db.find_asset_by_size_xxh(123, 77).expect("query"),
        Some(expected)
    );
    assert_eq!(db.find_asset_by_size_xxh(123, 999).expect("query"), None);
    assert_eq!(db.find_asset_by_size_xxh(999, 77).expect("query"), None);
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
    writer.migrate().expect("migrate");
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
