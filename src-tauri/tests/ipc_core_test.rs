//! M1 T9 IPC 核心测试：编排状态机（Busy/无库/分页/暂停恢复取消/设备文件）。
//! tauri 命令层是薄包装，业务逻辑全在这里测（直接构造 AppState）。

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
#[path = "../src/ipc/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod ipc;
#[path = "../src/metadata/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod metadata;
#[path = "../src/settings/mod.rs"]
#[allow(dead_code)] // 测试按子集编译源码树
mod settings;

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use db::Db;
use devices::volume::VolumeSource;
use devices::{DeviceResult, DeviceSource, FileEntry, SourceKind};
use events::{EventBus, FileState};
use import::engine::{ImportMode, ImportPlan};
use ipc::{
    active_library_db, cancel_import, files_by_id, jobs_page, list_dirs, logs_page, retry_failed,
    scan_folder, set_import_paused, start_import, AppState, DeviceEntry,
};
use settings::{Library, Settings};

/// 慢速卷源：保证暂停/取消窗口落在会话中段。
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

fn build_source(dir: &Path, n: usize) {
    fs::create_dir_all(dir.join("DCIM")).unwrap();
    for i in 0..n {
        let mut content = vec![0xFF, 0xD8, 0xFF, 0xE0];
        content.extend(vec![b'x'; 512]);
        content.extend_from_slice(&(i as u32).to_be_bytes());
        fs::write(dir.join(format!("DCIM/IMG_{i:04}.jpg")), &content).unwrap();
    }
}

fn state_with_library(db_dir: &Path, source_dir: &Path, delay: Duration) -> AppState {
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
    }
}

fn plan(state: &AppState, target: &Path) -> ImportPlan {
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
        duplicate_policy: settings::DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
        mode: ImportMode::Copy,
    }
}

fn wait_done(state: &AppState, timeout: Duration) -> bool {
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
// M2：从文件夹导入（folder_scan 注册临时源）
// ---------------------------------------------------------------------------

/// 与 `\\?\` 前缀无关的 canonical 路径字符串（期望 id 用）。
fn plain_canonical(path: &Path) -> String {
    path.to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_string()
}

#[test]
fn folder_scan_registers_source_then_files_and_move_import_work() {
    let folder = tempfile::tempdir().unwrap();
    build_source(folder.path(), 3);
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let state = state_with_library(db_dir.path(), folder.path(), Duration::from_millis(1));
    // 只留 folder_scan 注册路径（清掉预注册的卷源）
    state.devices.lock().unwrap().clear();

    // 不存在路径 → Err
    assert!(scan_folder(&state, r"C:\definitely\not\here").is_err());

    // 扫描注册：id 为 FOLDER:<canonical>，快照统计正确
    let snapshot = scan_folder(&state, folder.path().to_str().unwrap()).unwrap();
    assert_eq!(
        snapshot.id,
        format!(
            "FOLDER:{}",
            plain_canonical(&fs::canonicalize(folder.path()).unwrap())
        )
    );
    assert_eq!(snapshot.kind, SourceKind::Folder);
    assert_eq!(snapshot.new_files, 3, "空库应全部为新文件");
    assert!(snapshot.bytes_total > 0);

    // 路径变体（尾部 /.）→ 同一 id（幂等注册，不重复建条目）
    let variant = folder.path().join(".");
    let again = scan_folder(&state, variant.to_str().unwrap()).unwrap();
    assert_eq!(again.id, snapshot.id);
    assert_eq!(state.devices.lock().unwrap().len(), 1);

    // 注册后 device_files 可用（前端向导走同一命令）
    let files = files_by_id(&state, &snapshot.id).unwrap();
    assert_eq!(files.len(), 3);
    assert!(files.iter().all(|f| f.rel_path.starts_with("DCIM/")));

    // 走现有 import_start 管线（move 模式）→ 源删除、目标入库
    let mut p = plan(&state, target.path());
    p.source_id = snapshot.id.clone();
    p.mode = ImportMode::Move;
    let job_id = start_import(&state, p).unwrap();
    assert!(
        wait_done(&state, Duration::from_secs(15)),
        "move 导入应完成"
    );
    let db = open_library_db(&state);
    let states: Vec<FileState> = db
        .all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|r| r.state)
        .collect();
    assert!(states.iter().all(|s| *s == FileState::Verified));
    let assets: i64 =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get(0))
            .unwrap();
    assert_eq!(assets, 3);
    // 源 jpg 全部删除（move 经 IPC 的 ArcSource 转发生效；空目录清理后
    // DCIM 可能整体移除，按全树扫描断言）
    let remaining = walkdir::WalkDir::new(folder.path())
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x == "jpg"))
        .count();
    assert_eq!(remaining, 0, "move 后源不应残留 jpg");
    assert!(folder.path().exists(), "源根保留");
}

// ---------------------------------------------------------------------------
// M2：文件系统目录树浏览（fs_list_dirs，导入向导源面板懒加载）
// ---------------------------------------------------------------------------

#[test]
fn fs_list_dirs_lists_child_dirs_one_level() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("photos").join("2024")).unwrap();
    fs::create_dir_all(root.path().join("empty_dir")).unwrap();
    fs::create_dir_all(root.path().join(".dotted")).unwrap();
    fs::create_dir_all(root.path().join("$RECYCLE.BIN")).unwrap();
    fs::create_dir_all(root.path().join("System Volume Information")).unwrap();
    fs::write(root.path().join("afile.jpg"), b"x").unwrap(); // 文件不得出现
                                                             // 隐藏属性目录（0x2）应被过滤
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::core::PCWSTR;
        use windows::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
        let hidden = root.path().join("hidden_attr");
        fs::create_dir_all(&hidden).unwrap();
        let wide: Vec<u16> = hidden
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe { SetFileAttributesW(PCWSTR(wide.as_ptr()), FILE_ATTRIBUTE_HIDDEN) }
            .expect("set hidden attr");
    }

    let entries = list_dirs(Some(root.path().to_str().unwrap()));
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(
        names,
        ["empty_dir", "photos"],
        "文件/点前缀/黑名单/隐藏属性不出现"
    );

    // hasSubdirs 浅探测 + 绝对路径 + camelCase 负载
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();
    assert!(photos.has_subdirs, "photos 下有 2024");
    assert!(Path::new(&photos.path).is_absolute(), "路径必须绝对化");
    let empty = entries.iter().find(|e| e.name == "empty_dir").unwrap();
    assert!(!empty.has_subdirs);
    let json = serde_json::to_value(photos).unwrap();
    assert!(json.get("hasSubdirs").is_some(), "camelCase: {json}");
}

#[test]
fn fs_list_dirs_invalid_parent_is_empty_and_roots_listed() {
    // 非法/不存在 parent → 空数组不报错
    assert!(list_dirs(Some(r"C:\definitely\not\here")).is_empty());

    // None → 盘符根：真实系统上 C:\ 必在（只断言存在，不遍历内容）
    let drives = list_dirs(None);
    assert!(
        drives.iter().any(|d| d.path == "C:\\"),
        "应包含 C:\\ : {drives:?}"
    );
    assert!(drives.iter().all(|d| d.name.ends_with(':')));
    assert!(drives.iter().all(|d| d.path.ends_with('\\')));
}

#[test]
fn fs_list_dirs_preserves_parent_path_form() {
    // 回归（映射盘 bug，2026-09-18）：条目 path 必须与父路径同形态原样拼接，
    // 不得 canonicalize——否则映射盘 Y:\DCIM 被解析成 \\?\UNC\... 再剥前缀，
    // 产出残缺的 "UNC\192.168.31.103\..."。用 verbatim 父路径本地复现
    // “形态转换”这一类破坏：canonicalize 会把 \\?\C:\... 变回 C:\...，
    // 修复后必须保留 verbatim 形态。
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("photos").join("2024")).unwrap();

    let verbatim_parent = format!(r"\\?\{}", root.path().display());
    let entries = list_dirs(Some(&verbatim_parent));
    let photos = entries
        .iter()
        .find(|e| e.name == "photos")
        .expect("verbatim 父路径应可枚举");
    assert_eq!(
        photos.path,
        format!(r"{}\photos", verbatim_parent),
        "条目 path 必须原样拼接父路径（不转换形态）: {:?}",
        entries
    );
    // 子树探测也用同形态路径（否则 hasSubdirs 恒 false —— Y:\照片 展开为空的根因）
    assert!(photos.has_subdirs, "探测子目录不得因形态转换失败");
    // 绝不出现剥坏前缀的 UNC\ 残缺形态
    assert!(!photos.path.starts_with(r"UNC\"));

    // 普通形态父路径（带尾分隔符）同样原样系（幂等展开）
    let plain_with_slash = format!(r"{}\", root.path().display());
    let entries = list_dirs(Some(&plain_with_slash));
    let photos = entries.iter().find(|e| e.name == "photos").unwrap();
    assert_eq!(photos.path, format!(r"{}\photos", root.path().display()));
    assert!(photos.has_subdirs);
}

// ---------------------------------------------------------------------------
// 内部工具
// ---------------------------------------------------------------------------

fn open_library_db(state: &AppState) -> Db {
    active_library_db(state).unwrap()
}

#[test]
fn start_pause_resume_cancel_state_machine() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path(), 8);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(25));

    let job_id = start_import(&state, plan(&state, target.path())).unwrap();
    assert!(job_id > 0);

    // Busy：进行中二次启动被拒
    let busy = start_import(&state, plan(&state, target.path())).unwrap_err();
    assert!(busy.contains("Busy"), "应返回 Busy 错误: {busy}");

    // 暂停 → 恢复 → 取消（job_id 不匹配报错）
    set_import_paused(&state, job_id, true).unwrap();
    let wrong = set_import_paused(&state, job_id + 999, false).unwrap_err();
    assert!(wrong.contains("不一致"));
    set_import_paused(&state, job_id, false).unwrap();
    cancel_import(&state, job_id).unwrap();
    let wrong_cancel = cancel_import(&state, job_id + 999).unwrap_err();
    assert!(wrong_cancel.contains("不一致"));

    assert!(wait_done(&state, Duration::from_secs(10)), "取消后应收尾");
    let rows = jobs_page(&state, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "cancelled");
    assert_eq!(rows[0].id, job_id);

    // 取消后可再启动（活跃项被回收）
    let second = start_import(&state, plan(&state, target.path()));
    match second {
        Ok(new_id) => {
            assert_ne!(new_id, job_id);
            cancel_import(&state, new_id).unwrap();
            assert!(wait_done(&state, Duration::from_secs(10)));
        }
        Err(err) => panic!("取消后应允许再次导入: {err}"),
    }
}

#[test]
fn no_active_library_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_source(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    state.settings.lock().unwrap().active_library_id = None;

    let err = match active_library_db(&state) {
        Err(err) => err,
        Ok(_) => panic!("无库时应报错"),
    };
    assert!(err.contains("尚未创建库"));
    let err = start_import(&state, plan(&state, db_dir.path())).unwrap_err();
    assert!(err.contains("尚未创建库"));
}

#[test]
fn offline_device_rejected() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_source(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(5));
    let mut bad = plan(&state, db_dir.path());
    bad.source_id = "Z:".into();
    let err = start_import(&state, bad).unwrap_err();
    assert!(err.contains("不在线"));
}

#[test]
fn jobs_and_logs_paging_after_completion() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path(), 3);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let job_id = start_import(&state, plan(&state, target.path())).unwrap();
    assert!(wait_done(&state, Duration::from_secs(10)));

    let rows = jobs_page(&state, 0, 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].status, "done");
    assert_eq!(rows[0].total_files, 3);
    // keyset 分页边界
    assert!(jobs_page(&state, job_id, 10).unwrap().is_empty());

    let logs = logs_page(&state, job_id, 0, 50).unwrap();
    assert!(logs.iter().any(|l| l.message.contains("导入会话开始")));
    assert!(logs.iter().any(|l| l.message.contains("导入会话结束")));
    assert!(logs.iter().all(|l| l.job_id == Some(job_id)));

    // 无失败 → retry 报错
    let err = retry_failed(&state, job_id).unwrap_err();
    assert!(err.contains("没有可重试"));
}

#[test]
fn device_files_passthrough_with_camel_case() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    build_source(src.path(), 2);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));
    let device_id = state
        .devices
        .lock()
        .unwrap()
        .keys()
        .next()
        .cloned()
        .unwrap();

    let files = files_by_id(&state, &device_id).unwrap();
    assert_eq!(files.len(), 2);
    assert!(files[0].rel_path.starts_with("DCIM/"));
    let json = serde_json::to_value(&files[0]).unwrap();
    assert_eq!(json["relPath"], serde_json::json!(files[0].rel_path));
    assert!(json["mtime"].as_str().is_some_and(|m| m.ends_with('Z')));

    let err = files_by_id(&state, "missing").unwrap_err();
    assert!(err.contains("不在线"));
}

#[test]
fn journal_reflects_final_states() {
    let src = tempfile::tempdir().unwrap();
    let db_dir = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    build_source(src.path(), 4);
    let state = state_with_library(db_dir.path(), src.path(), Duration::from_millis(1));

    let job_id = start_import(&state, plan(&state, target.path())).unwrap();
    assert!(wait_done(&state, Duration::from_secs(10)));

    let db = open_library_db(&state);
    let states: Vec<FileState> = db
        .all_job_files(job_id)
        .unwrap()
        .into_iter()
        .map(|r| r.state)
        .collect();
    assert!(states.iter().all(|s| *s == FileState::Verified));
    // 目标落位
    let count =
        db.0.query_row("SELECT COUNT(*) FROM assets", [], |r| r.get::<_, i64>(0))
            .unwrap();
    assert_eq!(count, 4);
}
