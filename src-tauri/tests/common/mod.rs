//! 共享测试脚手架（tests/ 各文件 `mod common;` 复用）。
//!
//! - `#[path]` 引入的源码模块树在此统一声明；测试文件在 crate 根
//!   `pub use common::{db, devices, ...}` 再导出——源码内的 `crate::db`
//!   等绝对路径经由根部再导出解析（与旧版“每文件重复声明”等价）。
//! - 跨文件复用的 fixture：tempdir 源树构造、慢速/删源失败源桩、
//!   等待助手、AppState 构造等。本文件不放任何 #[test]。

#![allow(dead_code)] // 各测试文件按需取用 fixture 子集

#[path = "../../src/db/mod.rs"]
pub mod db;
#[path = "../../src/devices/mod.rs"]
pub mod devices;
#[path = "../../src/events/mod.rs"]
pub mod events;
#[path = "../../src/import/mod.rs"]
pub mod import;
#[path = "../../src/ipc/mod.rs"]
pub mod ipc;
#[path = "../../src/metadata/mod.rs"]
pub mod metadata;
#[path = "../../src/settings/mod.rs"]
pub mod settings;
#[path = "../../src/tasks/mod.rs"]
pub mod tasks;
#[path = "../../src/thumbs/mod.rs"]
pub mod thumbs;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, NaiveDate, Utc};
use db::Db;
use devices::volume::VolumeSource;
use devices::{DeviceResult, DeviceSource, FileEntry, SourceKind};
use events::{AppEvent, EventBus, FileState, JobStats};
use import::engine::{Engine, ImportMode, ImportPlan};
use ipc::{AppState, DeviceEntry};
use settings::{DuplicatePolicy, Library, Settings};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// 通用源树构造
// ---------------------------------------------------------------------------

/// 引擎/清卡用的 3 文件源树（jpg/raw/mp4，魔数与扩展名一致，内容 padded）。
/// 返回 (rel_path, content) 列表。
pub fn build_source(dir: &Path) -> Vec<(String, Vec<u8>)> {
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
pub fn shrink(mut head: Vec<u8>, size: usize) -> Vec<u8> {
    while head.len() < size {
        head.push(b'x');
    }
    head
}

/// n 个唯一内容的扁平 jpg 树（DCIM/IMG_{i:04}.jpg，取消/暂停/IPC 状态机用）。
pub fn build_many(dir: &Path, n: usize) {
    fs::create_dir_all(dir.join("DCIM")).unwrap();
    for i in 0..n {
        let mut content = shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 2048);
        // 内容唯一化：同 (size, xxh) 的相同字节会被精确查重合法跳过
        content[10..14].copy_from_slice(&(i as u32).to_be_bytes());
        fs::write(dir.join(format!("DCIM/IMG_{i:04}.jpg")), &content).unwrap();
    }
}

/// 设备源测试树：3 个媒体文件（混合大小写扩展名）+ 各类应忽略项。
pub fn build_tree(root: &Path) {
    let dcim = root.join("DCIM").join("100CANON");
    fs::create_dir_all(&dcim).unwrap();
    fs::write(dcim.join("IMG_0001.CR3"), vec![b'C'; 100]).unwrap();
    fs::write(dcim.join("IMG_0002.jpg"), b"jpeg-bytes").unwrap();
    fs::write(dcim.join("MVI_0003.MP4"), b"mp4-bytes!!").unwrap();

    // 非媒体扩展名
    fs::write(root.join("notes.txt"), b"ignore me").unwrap();

    // 系统目录（应整体跳过）
    let sys = root.join("System Volume Information");
    fs::create_dir_all(&sys).unwrap();
    fs::write(sys.join("WPQR0001.jpg"), b"x").unwrap();
    let bin = root.join("$RECYCLE.BIN");
    fs::create_dir_all(&bin).unwrap();
    fs::write(bin.join("trashed.jpg"), b"x").unwrap();

    // 隐藏目录（`.` 开头）
    let hidden = root.join(".Trashes");
    fs::create_dir_all(&hidden).unwrap();
    fs::write(hidden.join("hidden.jpg"), b"x").unwrap();
}

pub fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, s)
        .unwrap()
        .and_utc()
}

// ---------------------------------------------------------------------------
// DB / 引擎脚手架
// ---------------------------------------------------------------------------

pub fn open_db(dir: &Path) -> Db {
    let db = Db::open(&dir.join("library.db")).unwrap();
    db.migrate().unwrap();
    db
}

pub fn plan_for(target: &Path) -> ImportPlan {
    ImportPlan {
        source_id: "test-src".into(),
        target_root: target.to_path_buf(),
        dir_template: "{YYYY}/{MM-DD}".into(),
        name_template: "{原文件名}".into(),
        duplicate_policy: DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
        mode: ImportMode::Copy,
        second_target: None,
        include: None,
    }
}

/// 跑一个引擎会话（Volume 源），返回 (job_id, stats)。
pub fn run_engine(
    source_dir: &Path,
    db_dir: &Path,
    target_dir: &Path,
    plan_override: impl FnOnce(&mut ImportPlan),
) -> (i64, JobStats) {
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
pub fn expected_subdir(source_dir: &Path, rel: &str) -> PathBuf {
    let mtime = fs::metadata(source_dir.join(rel.replace('/', "\\")))
        .unwrap()
        .modified()
        .unwrap();
    let t: DateTime<Utc> = mtime.into();
    PathBuf::from(t.format("%Y/%m-%d").to_string())
}

pub fn sha256_of(data: &[u8]) -> [u8; 32] {
    let mut sha = Sha256::new();
    sha.update(data);
    sha.finalize().into()
}

pub fn count_assets(db: &Db) -> i64 {
    db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
        .unwrap()
}

pub fn journal_states(db: &Db, job_id: i64) -> Vec<FileState> {
    db.all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|row| row.state)
        .collect()
}

pub fn job_status(db: &Db, job_id: i64) -> String {
    db.0.query_row("SELECT status FROM jobs WHERE id = ?1", [job_id], |r| {
        r.get(0)
    })
    .unwrap()
}

pub fn pending_count(db: &Db, job_id: i64) -> u64 {
    db.0.query_row(
        "SELECT COUNT(*) FROM job_files WHERE job_id = ?1 AND state = 'pending'",
        [job_id],
        |r| r.get::<_, i64>(0),
    )
    .unwrap() as u64
}

pub fn find_part_files(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file() && entry.path().extension().is_some_and(|e| e == "part")
        })
        .map(|entry| entry.path().to_path_buf())
        .collect()
}

/// 阻塞等待下一个 ImportFileCompleted（跳过节流进度/里程碑事件）。
pub fn wait_completed(rx: &mut tokio::sync::broadcast::Receiver<AppEvent>) {
    loop {
        match rx.blocking_recv() {
            Ok(AppEvent::ImportFileCompleted { .. }) => return,
            Ok(_) | Err(_) => continue,
        }
    }
}

// ---------------------------------------------------------------------------
// 源桩
// ---------------------------------------------------------------------------

/// 慢速卷源：每个 stream 前睡眠，保证取消/暂停信号有窗口落入文件之间。
pub struct SlowSource {
    pub inner: VolumeSource,
    pub delay: Duration,
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

/// 删源必败的源（其余委托卷源）：move 模式删源失败降级路径用。
pub struct DeleteFailSource {
    pub inner: VolumeSource,
}

impl DeviceSource for DeleteFailSource {
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
        self.inner.stream(id)
    }
    fn delete(&self, _id: &str) -> DeviceResult<()> {
        Err(devices::DeviceError::Other("删源被拒绝（测试桩）".into()))
    }
}

// ---------------------------------------------------------------------------
// IPC / AppState 脚手架
// ---------------------------------------------------------------------------

/// 构造带激活库 + 慢速卷源注册表的 AppState（IPC 编排测试用）。
pub fn state_with_library(db_dir: &Path, source_dir: &Path, delay: Duration) -> AppState {
    let settings = Settings {
        libraries: vec![Library {
            id: "lib-1".into(),
            name: "主库".into(),
            db_dir: db_dir.to_string_lossy().into_owned(),
            photo_root: source_dir.to_string_lossy().into_owned(),
            ..Library::default()
        }],
        active_library_id: Some("lib-1".into()),
        ..Settings::default()
    };

    let source: Arc<dyn DeviceSource> = Arc::new(SlowSource {
        inner: VolumeSource::new(source_dir),
        delay,
    });
    let snapshot = devices::orchestrator::DeviceSnapshot {
        id: source.id(),
        name: "测试卡".into(),
        kind: SourceKind::Volume,
        files_by_kind: Default::default(),
        bytes_total: 0,
        new_files: 0,
    };
    let mut devices_map = HashMap::new();
    devices_map.insert(source.id(), DeviceEntry { source, snapshot });
    AppState {
        settings: Mutex::new(settings),
        config_dir: db_dir.join("config"),
        bus: EventBus::new(),
        devices: Mutex::new(devices_map),
        active_import: Mutex::new(None),
        supervisor: tasks::TaskSupervisor::new(EventBus::new()),
    }
}

/// IPC 导入计划：source_id 取注册表首个设备。
pub fn ipc_plan(state: &AppState, target: &Path) -> ImportPlan {
    let device_id = state
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();
    ImportPlan {
        source_id: device_id,
        target_root: target.to_path_buf(),
        dir_template: "{YYYY}/{MM-DD}".into(),
        name_template: "{原文件名}".into(),
        duplicate_policy: DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
        mode: ImportMode::Copy,
        second_target: None,
        include: None,
    }
}

/// 轮询等待活跃导入收尾（controls.is_done）。
pub fn wait_done(state: &AppState, timeout: Duration) -> bool {
    let started = Instant::now();
    loop {
        let done = state
            .active_import
            .lock()
            .unwrap()
            .as_ref()
            .is_none_or(|job| job.controls.is_done());
        if done {
            return true;
        }
        if started.elapsed() > timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
