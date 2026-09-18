//! M1 T7 导入引擎测试：tempdir 源目录 + 真实 SQLite（WAL 双连接断言）。
//! 经 `#[path]` 纳入源码模块树（与 devices_test 同法）。

#[path = "../src/db/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod db;
#[path = "../src/devices/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod devices;
#[path = "../src/events/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod events;
#[path = "../src/import/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod import;
#[path = "../src/metadata/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod metadata;
#[path = "../src/settings/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod settings;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use db::Db;
use devices::volume::VolumeSource;
use devices::{DeviceResult, DeviceSource, FileEntry, SourceKind};
use events::{AppEvent, EventBus, FileState};
use import::engine::{Engine, ImportPlan};
use settings::DuplicatePolicy;

// ---------------------------------------------------------------------------
// 测试脚手架
// ---------------------------------------------------------------------------

/// 造源树：返回 (rel_path, content) 列表（扩展名与魔数一致，classify 通过）。
fn build_source(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fs::create_dir_all(dir.join("DCIM/100CANON")).unwrap();
    let jpg = shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096);
    let raw = shrink(b"II*\0\x00\x00\x00\x08\x00\x00".to_vec(), 8192);
    let mp4 = shrink(b"\0\0\0\x18ftypisom\x00\x00".to_vec(), 2048);
    let files = vec![
        ("DCIM/100CANON/IMG_0001.jpg".to_string(), jpg),
        ("DCIM/100CANON/IMG_0002.CR3".to_string(), raw),
        ("DCIM/100CANON/MVI_0003.MP4".to_string(), mp4),
    ];
    for (rel, content) in &files {
        fs::write(dir.join(rel.replace('/', "\\")), content).unwrap();
    }
    files
}

/// 用 pattern 填充到至少 size 字节（保持魔数前缀）。
fn shrink(mut head: Vec<u8>, size: usize) -> Vec<u8> {
    while head.len() < size {
        head.push(b'x');
    }
    head
}

fn open_db(dir: &Path) -> Db {
    let db = Db::open(&dir.join("library.db")).unwrap();
    db.migrate().unwrap();
    db
}

fn plan_for(target: &Path) -> ImportPlan {
    ImportPlan {
        source_id: "test-src".into(),
        target_root: target.to_path_buf(),
        dir_template: "{YYYY}/{MM-DD}".into(),
        name_template: "{原文件名}".into(),
        duplicate: DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
    }
}

fn run_engine(
    source_dir: &Path,
    db_dir: &Path,
    target_dir: &Path,
    plan_override: impl FnOnce(&mut ImportPlan),
) -> (i64, events::JobStats) {
    let db = open_db(db_dir);
    let bus = EventBus::new();
    let source = Box::new(VolumeSource::new(source_dir));
    let mut plan = plan_for(target_dir);
    plan_override(&mut plan);
    let mut engine = Engine::new(db, bus, source, plan);
    let job_id = engine.begin().unwrap();
    let stats = engine.run();
    (job_id, stats)
}

/// 源文件 mtime 推导的期望目标目录（模板 {YYYY}/{MM-DD}；无 EXIF 回退 mtime）。
fn expected_subdir(source_dir: &Path, rel: &str) -> PathBuf {
    let mtime = fs::metadata(source_dir.join(rel.replace('/', "\\")))
        .unwrap()
        .modified()
        .unwrap();
    let t: chrono::DateTime<chrono::Utc> = mtime.into();
    let sub = t.format("%Y/%m-%d").to_string();
    PathBuf::from(sub)
}

fn sha256_of(data: &[u8]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(data);
    sha.finalize().into()
}

fn count_assets(db: &Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap()
}

fn journal_states(db: &Db, job_id: i64) -> Vec<FileState> {
    db.all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|row| row.state)
        .collect()
}

/// 阻塞等待下一个 ImportFileCompleted（跳过节流进度/里程碑事件）。
fn wait_completed(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) {
    loop {
        match rx.blocking_recv() {
            Ok(AppEvent::ImportFileCompleted { .. }) => return,
            Ok(_) | Err(_) => continue,
        }
    }
}

// ---------------------------------------------------------------------------
// T7 用例
// ---------------------------------------------------------------------------

#[test]
fn copies_files_with_byte_and_hash_integrity() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert_eq!(stats.total_files, 3);
    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.skipped_duplicates, 0);
    assert_eq!(stats.failed_files, 0);
    assert_eq!(
        stats.total_bytes,
        files.iter().map(|(_, c)| c.len() as u64).sum::<u64>()
    );

    // 字节级比对 + 目标路径按模板落位
    let db = open_db(db_dir.path());
    for (rel, content) in &files {
        let dst = target
            .path()
            .join(expected_subdir(src.path(), rel))
            .join(rel.rsplit('/').next().unwrap());
        assert_eq!(fs::read(&dst).unwrap(), *content, "字节不一致: {rel}");
        // journal + assets 哈希一致
        let row = db
            .all_job_files(job_id)
            .unwrap()
            .into_iter()
            .find(|r| &r.src == rel)
            .expect("journal row");
        assert_eq!(row.state, FileState::Verified);
        let expected_sha = sha256_of(content);
        assert_eq!(
            row.sha256.as_ref().map(|s| s.as_slice()),
            Some(expected_sha.as_slice())
        );
        assert_eq!(
            row.dst.replace('\\', "/"),
            dst.to_string_lossy().replace('\\', "/")
        );
    }
    assert_eq!(count_assets(&db), 3);
    assert_eq!(
        db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "done"
    );
}

#[test]
fn milestones_fire_in_order() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());
    // 追加第 4 个等大文件 → 每文件 25%
    let extra = shrink(b"\0\0\0\x18ftypisom\x00\x00".to_vec(), 4096);
    fs::write(src.path().join("DCIM/100CANON/MVI_0004.MOV"), &extra).unwrap();

    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let db = open_db(db_dir.path());
    let engine = Engine::new(
        db,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
    );
    let stats = engine.run();

    assert_eq!(stats.done_files, 4);
    let mut milestones = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let AppEvent::ImportMilestoneReached { percent, .. } = ev {
            milestones.push(percent);
        }
    }
    assert_eq!(milestones, vec![25, 50, 75, 100]);
}

#[test]
fn second_import_all_skipped() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());

    let (_, first) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(first.done_files, 3);
    let (_, second) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert_eq!(second.done_files, 0);
    assert_eq!(second.skipped_duplicates, 3);
    assert_eq!(second.failed_files, 0);
    let db = open_db(db_dir.path());
    assert_eq!(count_assets(&db), 3, "二次导入不得新增资产");
}

#[test]
fn rename_policy_generates_suffix() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    // 预占一个目标路径（内容不同，跳过内容查重干扰：skip_imported=false）
    let jpg = &files[0];
    let occupied = target
        .path()
        .join(expected_subdir(src.path(), &jpg.0))
        .join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate = DuplicatePolicy::Rename;
        plan.skip_imported = false;
    });

    assert_eq!(stats.done_files, 3);
    assert_eq!(stats.skipped_duplicates, 0);
    assert_eq!(fs::read(&occupied).unwrap(), b"occupied");
    let renamed = occupied.with_file_name("IMG_0001_1.jpg");
    assert_eq!(
        fs::read(&renamed).unwrap(),
        jpg.1,
        "Rename 策略应生成 _1 副本"
    );
}

#[test]
fn skip_policy_path_conflict_marks_skipped() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let jpg = &files[0];
    let occupied = target
        .path()
        .join(expected_subdir(src.path(), &jpg.0))
        .join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate = DuplicatePolicy::Skip;
        plan.skip_imported = false;
    });

    assert_eq!(stats.done_files, 2);
    assert_eq!(stats.skipped_duplicates, 1);
    let states = journal_states(&open_db(db_dir.path()), job_id);
    assert_eq!(
        states.iter().filter(|s| **s == FileState::Skipped).count(),
        1
    );
    // 原占用文件不被覆盖
    assert_eq!(fs::read(&occupied).unwrap(), b"occupied");
}

/// 慢速源：每个 stream 前睡眠，保证取消/暂停信号有窗口落入文件之间。
struct SlowSource {
    inner: VolumeSource,
    delay: Duration,
}

impl DeviceSource for SlowSource {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Volume
    }
    fn name(&self) -> String {
        self.inner.name()
    }
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        self.inner.list()
    }
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        self.inner.open_head(id, max)
    }
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        std::thread::sleep(self.delay);
        self.inner.stream(id)
    }
}

fn build_many(dir: &Path, n: usize) {
    fs::create_dir_all(dir.join("DCIM")).unwrap();
    for i in 0..n {
        let mut content = shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 2048);
        // 内容唯一化：同 (size, xxh) 的相同字节会被精确查重合法跳过
        content[10..14].copy_from_slice(&(i as u32).to_be_bytes());
        fs::write(dir.join(format!("DCIM/IMG_{i:04}.jpg")), &content).unwrap();
    }
}

#[test]
fn soft_cancel_returns_partial_and_job_cancelled() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 12);

    let db = open_db(db_dir.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(15),
        }),
        plan_for(target.path()),
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());

    // 首个文件完成后软取消
    wait_completed(&mut rx);
    controls.cancel();
    let stats = handle.join().unwrap();

    assert!(stats.done_files < 12, "软取消应停在部分完成: {stats:?}");
    assert_eq!(
        stats.done_files + stats.failed_files,
        stats.total_files - pending_count(&open_db(db_dir.path()), job_id)
    );
    let status = job_status(&open_db(db_dir.path()), job_id);
    assert_eq!(status, "cancelled");
}

#[test]
fn pause_resume_completes_without_loss() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 10);

    let db = open_db(db_dir.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus,
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(10),
        }),
        plan_for(target.path()),
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());

    // 首个文件完成后暂停 → 验证停工 → 恢复 → 完成
    wait_completed(&mut rx);
    controls.pause();
    std::thread::sleep(Duration::from_millis(120));
    assert!(!controls.is_done(), "暂停期间 run 不得结束");
    controls.resume();
    let stats = handle.join().unwrap();

    assert_eq!(stats.done_files, 10);
    assert_eq!(job_status(&open_db(db_dir.path()), job_id), "done");
}

#[test]
fn resume_after_interruption_redoes_pending_only() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 12);

    // 会话一：取消中断
    let db = open_db(db_dir.path());
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = Engine::new(
        db,
        bus.clone(),
        Box::new(SlowSource {
            inner: VolumeSource::new(src.path()),
            delay: Duration::from_millis(12),
        }),
        plan_for(target.path()),
    );
    let job_id = engine.begin().unwrap();
    let controls = engine.controls();
    let handle = std::thread::spawn(move || engine.run());
    wait_completed(&mut rx);
    controls.cancel();
    let partial = handle.join().unwrap();
    assert!(partial.done_files < 12);

    // 模拟崩溃残留：暂存目录里塞一个孤儿 .part
    let part_dir = target.path().join(".smartphoto-part");
    fs::create_dir_all(&part_dir).unwrap();
    fs::write(part_dir.join("999.part"), b"half-written").unwrap();

    // 会话二：resume 重建（verified 跳过，pending 重做）
    let db2 = open_db(db_dir.path());
    let engine = Engine::resume(
        db2,
        bus.clone(),
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
        job_id,
    )
    .unwrap();
    let stats = engine.run();

    assert_eq!(stats.done_files, 12, "续传后无遗漏");
    assert_eq!(stats.failed_files, 0);
    assert_eq!(count_assets(&open_db(db_dir.path())), 12, "无重复入库");

    // .part 无残留：暂存目录已清除
    assert!(!part_dir.exists(), "resume 后暂存目录应清除");
    assert!(find_part_files(target.path()).is_empty());

    // journal 全 verified，job 终态 done
    assert!(journal_states(&open_db(db_dir.path()), job_id)
        .into_iter()
        .all(|s| s == FileState::Verified));
    assert_eq!(job_status(&open_db(db_dir.path()), job_id), "done");
}

#[test]
fn no_part_residue_after_success() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path());

    run_engine(src.path(), db_dir.path(), target.path(), |_| {});

    assert!(!target.path().join(".smartphoto-part").exists());
    assert!(find_part_files(target.path()).is_empty());
}

#[test]
fn progress_events_are_throttled() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_many(src.path(), 40);

    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let db = open_db(db_dir.path());
    let engine = Engine::new(
        db,
        bus,
        Box::new(VolumeSource::new(src.path())),
        plan_for(target.path()),
    );
    let stats = engine.run();
    assert_eq!(stats.done_files, 40);

    let mut progress_count = 0;
    while let Ok(ev) = rx.try_recv() {
        if matches!(ev, AppEvent::ImportFileProgress { .. }) {
            progress_count += 1;
        }
    }
    assert!(progress_count >= 1, "首个进度必须立即发出");
    assert!(
        progress_count < 40,
        "40 个小文件不应产生 40 次进度（节流失效）: {progress_count}"
    );
}

#[test]
fn camera_template_falls_back_for_exifless_files() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    // 无 EXIF 相机字段的 NEF（TIFF 头但空 IFD）
    fs::create_dir_all(src.path().join("DCIM")).unwrap();
    let nef = shrink(b"II*\0\x00\x00\x00\x08\x00\x00".to_vec(), 1024);
    fs::write(src.path().join("DCIM/DSC_0001.NEF"), &nef).unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.dir_template = "{YYYY}/{相机}/{MM-DD}".into();
    });

    assert_eq!(stats.done_files, 1);
    // 期望落在 .../<年>/未知相机/<月-日>/DSC_0001.NEF
    let mtime: chrono::DateTime<chrono::Utc> = fs::metadata(src.path().join("DCIM/DSC_0001.NEF"))
        .unwrap()
        .modified()
        .unwrap()
        .into();
    let expected = target
        .path()
        .join(mtime.format("%Y").to_string())
        .join("未知相机")
        .join(mtime.format("%m-%d").to_string())
        .join("DSC_0001.NEF");
    assert!(
        expected.exists(),
        "应降级到 未知相机 目录: {}",
        expected.display()
    );
    assert_eq!(fs::read(&expected).unwrap(), nef);
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn job_status(db: &Db, job_id: i64) -> String {
    db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
        r.get(0)
    })
    .unwrap()
}

fn pending_count(db: &Db, job_id: i64) -> u64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM job_files WHERE job_id = ?1 AND state = 'pending'",
        [job_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap() as u64
}

fn find_part_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "part")
        })
        .map(|entry| entry.path().to_path_buf())
        .collect()
}
