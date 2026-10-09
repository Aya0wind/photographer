//! 引擎查重分层：宽松键预判、(size, xxhash) 精确复核、目标路径冲突策略
//! （Rename 生成 `_1` / Skip 判 skipped）、二次导入全跳过、missing 同哈希
//! 重绑（§八-1：skip 仅当存在**在线**同哈希资产）。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, settings, tasks, thumbs,
};

use std::fs;

use common::{
    build_source, count_assets, expected_mtime_dir, journal_states, open_db, run_engine,
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

// --- §八-1 导入侧：missing 同哈希重绑（skip 仅当在线同哈希资产） ----------

/// 库内同哈希资产缺失时重新导入：文件照常落盘 + 既有资产行重绑到新落位
/// 路径（清 missing、逻辑元数据保留），不产生第二行——旧行为是宽松/精确
/// 两层都 skip：文件不落盘、missing 记录不恢复，照片在画廊永久丢失视图。
#[test]
fn reimport_after_missing_rebinds_instead_of_skip() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    let (_, first) = run_engine(src.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(first.done_files, 3);

    let db = open_db(db_dir.path());
    // IMG_0001 本体被移出库（缺失）：行标 missing + 带逻辑元数据（评分）
    let (asset_id, old_path): (i64, String) = db
        .0
        .query_row(
            "SELECT id, path FROM assets WHERE filename = 'IMG_0001.jpg'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    fs::remove_file(&old_path).unwrap();
    db.0
        .execute(
            "UPDATE assets SET missing = 1, rating = 5 WHERE id = ?1",
            [asset_id],
        )
        .unwrap();

    // 同内容换文件名重新导入（宽松键 size+filename+mtime 不命中，放行到
    // 精确层哈希判定 → 命中 missing 行重绑）
    let src2 = tempfile::tempdir().unwrap();
    fs::create_dir_all(src2.path().join("DCIM/100CANON")).unwrap();
    fs::write(src2.path().join("DCIM/100CANON/IMG_0009.jpg"), &files[0].1).unwrap();

    let (job_id, second) = run_engine(src2.path(), db_dir.path(), target.path(), |_| {});
    assert_eq!(second.done_files, 1, "missing 同哈希重绑按成功结算");
    assert_eq!(second.skipped_duplicates, 0);
    assert_eq!(second.failed_files, 0);
    assert_eq!(count_assets(&db), 3, "重绑不产生第二行");
    let states = journal_states(&db, job_id);
    assert_eq!(
        states.iter().filter(|s| **s == FileState::Verified).count(),
        1
    );

    let (path, missing, rating): (String, i64, i64) = db
        .0
        .query_row(
            "SELECT path, missing, rating FROM assets WHERE id = ?1",
            [asset_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert_eq!(missing, 0, "重绑清 missing");
    assert_eq!(rating, 5, "逻辑元数据（评分）保留");
    assert!(
        path.ends_with("IMG_0009.jpg"),
        "路径重绑到新落位：{path}"
    );
    assert_eq!(fs::read(&path).unwrap(), files[0].1, "文件已落盘");
}

#[test]
fn rename_policy_generates_suffix() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let files = build_source(src.path());

    // 预占一个目标路径（内容不同，跳过内容查重干扰：skip_imported=false）
    let jpg = &files[0];
    let occupied = expected_mtime_dir(target.path(), &src.path().join("DCIM/100CANON/IMG_0001.jpg")).join("IMG_0001.jpg");
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

    let occupied = expected_mtime_dir(target.path(), &src.path().join("DCIM/100CANON/IMG_0001.jpg")).join("IMG_0001.jpg");
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
    let occupied = expected_mtime_dir(target.path(), &src.path().join("IMG_0001.jpg")).join("IMG_0001.jpg");
    fs::create_dir_all(occupied.parent().unwrap()).unwrap();
    fs::write(&occupied, b"occupied").unwrap();

    let read_bytes = std::sync::Arc::new(AtomicU64::new(0));
    let db = open_db(db_dir.path());
    let bus = events::EventBus::new();
    let source = CountingSource {
        inner: devices::volume::VolumeSource::new(src.path()),
        read_bytes: std::sync::Arc::clone(&read_bytes),
    };
    let mut plan = common::plan_for(&db, db_dir.path(), target.path());
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

    // 纯时间布局依赖拍摄时间（EXIF/mtime）→ 预判必须先读头段（≤1MB），
    // 不整读 5MB 写 .part（旧「零设备 IO 预跳」随布局改版退役）
    let read = read_bytes.load(Ordering::Relaxed);
    assert!(
        read > 0 && read <= 1024 * 1024,
        "目标已存在应只读头段预跳（≤1MB），实际 {read} 字节"
    );
}

#[test]
fn exif_dependent_layout_falls_back_to_head_preflight() {
    use std::sync::atomic::{AtomicU64, Ordering};

    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();

    // 纯时间布局依赖拍摄时间（EXIF/mtime）→ 预测必须先读头段（≤1MB 头）
    let mut content = vec![0u8; 5 * 1024 * 1024];
    content[..4].copy_from_slice(&[0xFF, 0xD8, 0xFF, 0xE0]);
    fs::write(src.path().join("IMG_0002.jpg"), &content).unwrap();

    // 无 EXIF → captured 回退清单 mtime；期望目录按源文件 mtime 推断
    let occupied = expected_mtime_dir(target.path(), &src.path().join("IMG_0002.jpg"))
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
    let mut plan = common::plan_for(&db, db_dir.path(), target.path());
    plan.duplicate_policy = DuplicatePolicy::Skip;
    plan.skip_imported = false;
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
