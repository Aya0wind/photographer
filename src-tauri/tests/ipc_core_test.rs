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
use import::engine::ImportPlan;
use ipc::{
    active_library_db, cancel_import, files_by_id, jobs_page, logs_page, retry_failed,
    set_import_paused, start_import, AppState, DeviceEntry,
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
        duplicate: settings::DuplicatePolicy::Skip,
        skip_imported: true,
        streams: 2,
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
