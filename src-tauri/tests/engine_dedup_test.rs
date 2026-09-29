//! 引擎查重分层：宽松键预判、(size, xxhash) 精确复核、目标路径冲突策略
//! （Rename 生成 `_1` / Skip 判 skipped）、二次导入全跳过。

mod common;

pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, migrate, settings, tasks, thumbs,
};

use std::fs;

use common::{
    build_source, count_assets, expected_ungrouped_dir, journal_states, open_db, run_engine,
};
use events::FileState;
use settings::DuplicatePolicy;

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
    let occupied = expected_ungrouped_dir(db_dir.path(), target.path()).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (_, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate_policy = DuplicatePolicy::Rename;
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
    build_source(src.path()); // 建源树（期望路径不再依赖逐文件 mtime）

    let occupied = expected_ungrouped_dir(db_dir.path(), target.path()).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let (job_id, stats) = run_engine(src.path(), db_dir.path(), target.path(), |plan| {
        plan.duplicate_policy = DuplicatePolicy::Skip;
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

// --- 免下载预跳（2026-09-29 实测重建库全量重导：重复照片被完整拉回再丢弃） ----------

/// 计数源桩：包装 VolumeSource，只覆写 stream 统计实际读出的字节数。
struct CountingSource {
    inner: devices::volume::VolumeSource,
    read_bytes: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl devices::DeviceSource for CountingSource {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn kind(&self) -> devices::SourceKind {
        self.inner.kind()
    }
    fn name(&self) -> String {
        self.inner.name()
    }
    fn list(&self) -> devices::DeviceResult<Vec<devices::FileEntry>> {
        self.inner.list()
    }
    fn open_head(&self, id: &str, max: u64) -> devices::DeviceResult<Vec<u8>> {
        self.inner.open_head(id, max)
    }
    fn stream(&self, id: &str) -> devices::DeviceResult<Box<dyn std::io::Read + Send>> {
        let inner = self.inner.stream(id)?;
        Ok(Box::new(CountingReader {
            inner,
            read: std::sync::Arc::clone(&self.read_bytes),
        }))
    }
}

struct CountingReader {
    inner: Box<dyn std::io::Read + Send>,
    read: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl std::io::Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
        Ok(n)
    }
}

#[test]
fn skip_policy_existing_dst_skips_without_full_download() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();

    // 5MB JPEG（大于 1MB 头段上限）：旧路径整读 5MB 写 .part 再丢弃
    let mut content = vec![0u8; 5 * 1024 * 1024];
    content[..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    fs::write(src.path().join("IMG_0001.jpg"), &content).unwrap();

    // 预占目标路径（db 空 + skip_imported=false → 只剩路径判重这一层）
    let occupied = expected_ungrouped_dir(db_dir.path(), target.path()).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let read_bytes = std::sync::Arc::new(AtomicU64::new(0));
    let db = open_db(db_dir.path());
    let bus = events::EventBus::new();
    let source = CountingSource {
        inner: devices::volume::VolumeSource::new(src.path()),
        read_bytes: std::sync::Arc::clone(&read_bytes),
    };
    let mut plan = common::plan_for(target.path());
    plan.duplicate_policy = DuplicatePolicy::Skip;
    plan.skip_imported = false;
    plan.album_id = Some(db.ensure_default_album().unwrap());
    let mut engine = import::engine::Engine::new(db, bus, Box::new(source), plan);
    let job_id = engine.begin().unwrap();
    let stats = engine.run();

    assert_eq!(stats.skipped_duplicates, 1);
    assert_eq!(stats.done_files, 0);
    assert_eq!(stats.failed_files, 0);
    assert_eq!(fs::read(&occupied).unwrap(), b"occupied", "占用文件不得覆盖");
    let states = journal_states(&open_db(db_dir.path()), job_id);
    assert_eq!(states.iter().filter(|s| **s == FileState::Skipped).count(), 1);

    // 零设备 IO 预判（默认模板 {原文件名} + 相册级目录公式）：不碰源
    let read = read_bytes.load(Ordering::Relaxed);
    assert_eq!(
        read, 0,
        "默认模板下目标已存在应零 IO 预跳，实际读了 {read} 字节"
    );
}

#[test]
fn exif_dependent_template_falls_back_to_head_preflight() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();

    // 命名含 {YYYY}（拍摄日期令牌）：预测不可用，回退头段预跳（≤1MB 头）
    let mut content = vec![0u8; 5 * 1024 * 1024];
    content[..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    fs::write(src.path().join("IMG_0002.jpg"), &content).unwrap();

    // 无 EXIF → captured 回退清单 mtime；占用目录按该年份动态构造
    let year: String = {
        let mtime: chrono::DateTime<chrono::Utc> =
            fs::metadata(src.path().join("IMG_0002.jpg")).unwrap().modified().unwrap().into();
        mtime.format("%Y").to_string()
    };
    let occupied = expected_ungrouped_dir(db_dir.path(), target.path())
        .join(&year)
        .join("IMG_0002.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let read_bytes = std::sync::Arc::new(AtomicU64::new(0));
    let db = open_db(db_dir.path());
    let bus = events::EventBus::new();
    let source = CountingSource {
        inner: devices::volume::VolumeSource::new(src.path()),
        read_bytes: std::sync::Arc::clone(&read_bytes),
    };
    let mut plan = common::plan_for(target.path());
    plan.duplicate_policy = DuplicatePolicy::Skip;
    plan.skip_imported = false;
    plan.name_template = "{YYYY}/{原文件名}".into();
    plan.album_id = Some(db.ensure_default_album().unwrap());
    let mut engine = import::engine::Engine::new(db, bus, Box::new(source), plan);
    engine.begin().unwrap();
    let stats = engine.run();

    assert_eq!(stats.skipped_duplicates, 1);
    let read = read_bytes.load(Ordering::Relaxed);
    assert!(
        read > 0 && read <= 2 * 1024 * 1024,
        "EXIF 模板应走头段预跳（>0 且 ≤2MB），实际 {read} 字节"
    );
}
