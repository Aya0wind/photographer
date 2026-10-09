//! 共享测试脚手架（tests/ 各文件 `mod common;` 复用）。
//!
//! - `#[path]` 引入的源码模块树在此统一声明；测试文件在 crate 根
//!   `pub use common::{db, devices, ...}` 再导出——源码内的 `crate::db`
//!   等绝对路径经由根部再导出解析（与旧版“每文件重复声明”等价）。
//! - 跨文件复用的 fixture：tempdir 源树构造、慢速/删源失败源桩、
//!   等待助手、AppState 构造等。本文件不放任何 #[test]。

#![allow(dead_code)] // 各测试文件按需取用 fixture 子集

#[path = "../../src/ai/mod.rs"]
pub mod ai;
#[path = "../../src/bursts/mod.rs"]
pub mod bursts;
#[path = "../../src/db/mod.rs"]
pub mod db;
#[path = "../../src/devices/mod.rs"]
pub mod devices;
#[path = "../../src/events/mod.rs"]
pub mod events;
#[path = "../../src/geo/mod.rs"]
pub mod geo;
#[path = "../../src/import/mod.rs"]
pub mod import;
#[path = "../../src/index/mod.rs"]
pub mod index;
#[path = "../../src/ipc/mod.rs"]
pub mod ipc;
#[path = "../../src/metadata/mod.rs"]
pub mod metadata;
#[path = "../../src/platform/mod.rs"]
pub mod platform;
#[path = "../../src/scan/mod.rs"]
pub mod scan;
#[path = "../../src/settings/mod.rs"]
pub mod settings;
#[path = "../../src/tasks/mod.rs"]
pub mod tasks;
#[path = "../../src/tethering/mod.rs"]
pub mod tethering;
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
use settings::{DuplicatePolicy, Settings};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// 通用源树构造
// ---------------------------------------------------------------------------

/// 引擎/清卡用的 3 张图片源树。
/// 返回可导入的 (rel_path, content) 列表。
pub fn build_source(dir: &Path) -> Vec<(String, Vec<u8>)> {
    fs::create_dir_all(dir.join("DCIM/100CANON")).unwrap();
    let jpg = shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096);
    let raw = shrink(b"II*\0\x00\x00\x00\x08\x00\x00".to_vec(), 8192);
    let mut jpg2 = shrink(vec![0xFF, 0xD8, 0xFF, 0xE0], 4096);
    jpg2[10] = 3;
    let files = vec![
        ("DCIM/100CANON/IMG_0001.jpg".to_string(), jpg),
        ("DCIM/100CANON/IMG_0002.CR3".to_string(), raw),
        ("DCIM/100CANON/IMG_0003.jpg".to_string(), jpg2),
    ];
    for (rel, content) in &files {
        fs::write(dir.join(rel), content).unwrap();
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
    // 单版本 schema(2026-10-09):打开即幂等建表,无 migrate 步骤。
    Db::open(&dir.join("library.db")).unwrap()
}

/// 幂等登记目标照片库（root 已登记 → 复用同一条）。root 互斥校验的
/// 数据库目录基准给一个恒不与测试 root 重叠的锚目录（应用侧传真实数据
/// 库目录；引擎级测试不验证互斥本身）。
pub fn ensure_photo_library(db: &Db, database_dir: &Path, root: &Path) -> String {
    let root_str = root.to_string_lossy().into_owned();
    if let Ok(rows) = db.photos_library_list() {
        if let Some(row) = rows
            .into_iter()
            .find(|r| r.root_path.eq_ignore_ascii_case(&root_str))
        {
            return row.id;
        }
    }
    db.photos_library_register("测试照片库", &root_str, database_dir)
        .unwrap()
        .id
}

/// 引擎级导入计划：目标照片库 = target root 登记条目（2026-10-09 §三
/// 照片库基准）；布局固定纯时间（无模板字段）。album_id 缺省 None（相册
/// 为纯逻辑引用，可选）。
pub fn plan_for(db: &Db, db_dir: &Path, target: &Path) -> ImportPlan {
    ImportPlan {
        source_id: "test-src".into(),
        target_library_id: ensure_photo_library(db, db_dir, target),
        duplicate_policy: DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
        mode: ImportMode::Copy,
        second_target: None,
        include: None,
        album_id: None,
        album_subgroup: None,
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
    let mut plan = plan_for(&db, db_dir, target_dir);
    plan_override(&mut plan);
    let mut engine = Engine::new(db, bus, source, plan);
    let job_id = engine.begin().unwrap();
    let stats = engine.run();
    (job_id, stats)
}

/// 纯时间布局期望目录（§三 唯一公式）：`target/{拍摄年}/{拍摄月}`。
/// captured 传 EXIF 时间；无 EXIF 文件用 [`expected_mtime_dir`]（mtime
/// 回退口径与引擎一致）。
pub fn expected_time_dir(target: &Path, captured: DateTime<Utc>) -> PathBuf {
    target.join(captured.format("%Y/%m").to_string())
}

/// 无 EXIF 源文件的期望目录：以源文件 mtime 推断（引擎 resolve_captured
/// 读同一 metadata，两次读取对未变动文件恒同值）。
pub fn expected_mtime_dir(target: &Path, source_file: &Path) -> PathBuf {
    let mtime: DateTime<Utc> = fs::metadata(source_file)
        .unwrap()
        .modified()
        .unwrap()
        .into();
    expected_time_dir(target, mtime)
}

/// plan_for + ensure_default_album（挂「未分组」引用的直接引擎测试用）。
pub fn plan_with_album(db: &Db, db_dir: &Path, target: &Path) -> ImportPlan {
    let mut plan = plan_for(db, db_dir, target);
    plan.album_id = Some(db.ensure_default_album().unwrap());
    plan
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

/// 构造带慢速卷源注册表的 AppState（IPC 编排测试用）。应用唯一数据库 =
/// config_dir（= db_dir，tests 的 open_db(db_dir) 与 AppState 解析同库）；
/// settings 只含应用级配置（2026-10-09：库注册表/activeLibrary 退役）。
pub fn state_with_library(db_dir: &Path, source_dir: &Path, delay: Duration) -> AppState {
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
    devices_map.insert(source.id(), DeviceEntry::ready(source, snapshot));
    let supervisor = tasks::TaskSupervisor::new(EventBus::new());
    AppState {
        settings: Mutex::new(Settings::default()),
        config_dir: db_dir.to_path_buf(),
        bus: EventBus::new(),
        devices: Mutex::new(devices_map),
        active_import: Mutex::new(None),
        import_running: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        register_gate: std::sync::Arc::new(std::sync::Mutex::new(())),
        library_scan_kick: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ai: ai::ModelManager::new(db_dir.join("models"), EventBus::new(), supervisor.clone()),
        supervisor,
        thumb_queue: ipc::thumb::ThumbQueue::new(),
    }
}

/// 独立临时库（含慢速卷源），调用方持有 TempDir 保证整个测试的生命周期。
pub fn library_fixture() -> (tempfile::TempDir, AppState, Db) {
    let dir = tempfile::TempDir::new().unwrap();
    let db_dir = dir.path().join("db");
    let photo_root = dir.path().join("photos");
    std::fs::create_dir_all(&db_dir).unwrap();
    std::fs::create_dir_all(&photo_root).unwrap();
    let state = state_with_library(&db_dir, &photo_root, Duration::from_millis(1));
    let db = open_db(&db_dir);
    (dir, state, db)
}

/// IPC 导入计划：source_id 取注册表首个设备；目标照片库按 target root
/// 在应用唯一数据库（state.config_dir）登记（root 互斥校验走真实基准——
/// target 与数据库目录重叠会在登记处报错）。
pub fn ipc_plan(state: &AppState, target: &Path) -> ImportPlan {
    let device_id = state
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();
    let db = open_db(&state.config_dir);
    let root_str = target.to_string_lossy().into_owned();
    let existing = db
        .photos_library_list()
        .unwrap()
        .into_iter()
        .find(|r| r.root_path.eq_ignore_ascii_case(&root_str))
        .map(|r| r.id);
    let target_library_id = match existing {
        Some(id) => id,
        None => {
            db.photos_library_register("测试照片库", &root_str, &state.config_dir)
                .unwrap()
                .id
        }
    };
    ImportPlan {
        source_id: device_id,
        target_library_id,
        duplicate_policy: DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
        mode: ImportMode::Copy,
        second_target: None,
        include: None,
        album_id: None,
        album_subgroup: None,
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

// ---------------------------------------------------------------------------
// EXIF JPEG fixture（导入入库拍摄参数测试；II* 小端 + APP1 + SOF0 + EOI）
// ---------------------------------------------------------------------------

/// 一张带完整拍摄参数的 JPEG（Sony A7R5 / FE 85mm / f2.8 / 1-250s / ISO1600
/// / 6048x8064 / 2026-06-28 15:30:00）。宽高经 SOF0 帧段给出。
pub fn build_exif_jpeg() -> Vec<u8> {
    fn ascii(s: &str) -> Vec<u8> {
        let mut v = s.as_bytes().to_vec();
        v.push(0);
        v
    }
    fn rational(n: u32, d: u32) -> Vec<u8> {
        let mut v = n.to_le_bytes().to_vec();
        v.extend_from_slice(&d.to_le_bytes());
        v
    }
    // (tag, type, payload)
    let ifd0_entries: Vec<(u16, u16, Vec<u8>)> =
        vec![(0x010F, 2, ascii("Sony")), (0x0110, 2, ascii("ILCE-7RM5"))];
    let exif_entries: Vec<(u16, u16, Vec<u8>)> = vec![
        (0x829A, 5, rational(1, 250)),               // ExposureTime
        (0x829D, 5, rational(28, 10)),               // FNumber
        (0x8827, 3, 1600u16.to_le_bytes().to_vec()), // ISO
        (0x9003, 2, ascii("2026:06:28 15:30:00")),   // DateTimeOriginal
        (0x920A, 5, rational(85, 1)),                // FocalLength
        (0xA434, 2, ascii("FE 85mm F1.8")),          // LensModel
    ];

    let ifd0_size = 2 + 12 * (ifd0_entries.len() + 1) + 4; // + ExifIFD 指针
    let exif_off = 8 + ifd0_size;
    let exif_size = 2 + 12 * exif_entries.len() + 4;
    let ifd0_data_off = exif_off + exif_size;
    let mut ifd0_data = Vec::new();
    let mut exif_data = Vec::new();

    let encode = |entries: &[(u16, u16, Vec<u8>)],
                  data_off: usize,
                  data: &mut Vec<u8>,
                  child: Option<(u16, u32)>|
     -> Vec<u8> {
        let mut buf = Vec::new();
        buf.extend_from_slice(&((entries.len() + child.is_some() as usize) as u16).to_le_bytes());
        let mut list = entries.to_vec();
        if let Some((tag, value)) = child {
            list.push((tag, 4, value.to_le_bytes().to_vec()));
        }
        list.sort_by_key(|e| e.0);
        for (tag, typ, payload) in list {
            let count = payload.len().max(1) / if typ == 2 { 1 } else { typ_len(typ) };
            buf.extend_from_slice(&tag.to_le_bytes());
            buf.extend_from_slice(&typ.to_le_bytes());
            buf.extend_from_slice(&(count as u32).to_le_bytes());
            if payload.len() <= 4 {
                let mut inline = [0u8; 4];
                inline[..payload.len()].copy_from_slice(&payload);
                buf.extend_from_slice(&inline);
            } else {
                buf.extend_from_slice(&((data_off + data.len()) as u32).to_le_bytes());
                data.extend_from_slice(&payload);
            }
        }
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf
    };
    fn typ_len(typ: u16) -> usize {
        match typ {
            3 => 2,
            4 => 4,
            5 => 8,
            _ => 1,
        }
    }

    let ifd0_bytes = encode(
        &ifd0_entries,
        ifd0_data_off,
        &mut ifd0_data,
        Some((0x8769, exif_off as u32)),
    );
    let exif_bytes = encode(
        &exif_entries,
        ifd0_data_off + ifd0_data.len(),
        &mut exif_data,
        None,
    );

    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II* ");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&ifd0_bytes);
    tiff.extend_from_slice(&exif_bytes);
    tiff.extend_from_slice(&ifd0_data);
    tiff.extend_from_slice(&exif_data);

    let mut out = Vec::new();
    out.extend_from_slice(&[0xFF, 0xD8]); // SOI
    out.extend_from_slice(&[0xFF, 0xE1]); // APP1
    out.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    out.extend_from_slice(b"Exif  ");
    out.extend_from_slice(&tiff);
    // SOF0：6048x8064
    out.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08]);
    out.extend_from_slice(&8064u16.to_be_bytes());
    out.extend_from_slice(&6048u16.to_be_bytes());
    out.push(3);
    out.extend_from_slice(&[0; 9]);
    out.extend_from_slice(&[0xFF, 0xD9]); // EOI
    out
}
