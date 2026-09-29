mod common;
pub use common::{
    platform,
    ai, bursts, db, devices, events, import, index, ipc, metadata, migrate, settings, tasks, thumbs,
};

use devices::{DeviceResult, DeviceSource, FileEntry, SourceKind};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

struct BlockedSource {
    inner: devices::volume::VolumeSource,
    started: mpsc::Sender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl DeviceSource for BlockedSource {
    fn id(&self) -> String {
        self.inner.id()
    }
    fn name(&self) -> String {
        "旧连接".into()
    }
    fn kind(&self) -> SourceKind {
        SourceKind::Volume
    }
    fn list(&self) -> DeviceResult<Vec<FileEntry>> {
        self.started.send(()).unwrap();
        self.resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        self.inner.list()
    }
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>> {
        self.inner.open_head(id, max)
    }
    fn list_with_progress(
        &self,
        callback: devices::FileBatchCallback,
    ) -> DeviceResult<Vec<FileEntry>> {
        let files = self.inner.list()?;
        callback(files.clone());
        self.started.send(()).unwrap();
        self.resume
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        Ok(files)
    }
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn std::io::Read + Send>> {
        self.inner.stream(id)
    }
}

fn blocked(
    state: &ipc::AppState,
    path: &std::path::Path,
) -> (String, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (started, wait) = mpsc::channel();
    let (resume, gate) = mpsc::channel();
    let source = Arc::new(BlockedSource {
        inner: devices::volume::VolumeSource::new(path),
        started,
        resume: Mutex::new(gate),
    });
    let id = source.id();
    state.devices.lock().unwrap().get_mut(&id).unwrap().source = source;
    (id, wait, resume)
}

#[test]
fn file_listing_does_not_lock_out_device_removal() {
    let dir = tempfile::tempdir().unwrap();
    common::build_many(dir.path(), 1);
    let db = tempfile::tempdir().unwrap();
    let state = Arc::new(common::state_with_library(
        db.path(),
        dir.path(),
        Duration::ZERO,
    ));
    let (id, started, release) = blocked(&state, dir.path());
    let task_state = state.clone();
    let task_id = id.clone();
    let task = std::thread::spawn(move || ipc::files_by_id(&task_state, &task_id));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(
        state.devices.try_lock().is_ok(),
        "设备 I/O 不能持有注册表锁"
    );
    ipc::reconcile::remove_device(&state, &id);
    release.send(()).unwrap();
    assert!(task.join().unwrap().is_err(), "旧连接的文件清单必须作废");
    assert!(!state.devices.lock().unwrap().contains_key(&id));
}

#[test]
fn old_manual_scan_cannot_overwrite_reconnected_source_or_emit_scanned() {
    let dir = tempfile::tempdir().unwrap();
    common::build_many(dir.path(), 1);
    let db = tempfile::tempdir().unwrap();
    let state = Arc::new(common::state_with_library(
        db.path(),
        dir.path(),
        Duration::ZERO,
    ));
    let (id, started, release) = blocked(&state, dir.path());
    let mut events = state.bus.subscribe();
    let task_state = state.clone();
    let task_id = id.clone();
    let task = std::thread::spawn(move || ipc::scan_by_id(&task_state, &task_id));
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(state.devices.try_lock().is_ok());
    state.devices.lock().unwrap().get_mut(&id).unwrap().source =
        Arc::new(devices::volume::VolumeSource::new(dir.path()));
    release.send(()).unwrap();
    assert!(task.join().unwrap().unwrap_err().contains("重新连接"));
    assert!(events.try_recv().is_err());
}

#[test]
fn old_background_scan_cannot_revive_removed_device() {
    let dir = tempfile::tempdir().unwrap();
    common::build_many(dir.path(), 1);
    let db = tempfile::tempdir().unwrap();
    let state = Arc::new(common::state_with_library(
        db.path(),
        dir.path(),
        Duration::ZERO,
    ));
    let (id, started, release) = blocked(&state, dir.path());
    state.devices.lock().unwrap().get_mut(&id).unwrap().scan =
        ipc::DeviceScan::Failed("重试".into(), Instant::now());
    let mut events = state.bus.subscribe();
    ipc::reconcile::reconcile_with_truth(
        &state,
        "retry",
        &[(id.clone(), SourceKind::Volume, "卡".into())],
    );
    started.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(
        state.devices.lock().unwrap()[&id].scan,
        ipc::DeviceScan::Scanning
    ));
    assert!(
        ipc::scan_by_id(&state, &id)
            .unwrap_err()
            .contains("正在扫描"),
        "自动扫描期间不能再排入手动扫描"
    );
    let mut progress = false;
    while let Ok(event) = events.try_recv() {
        if let events::AppEvent::DeviceFilesProgress { files, .. } = event {
            assert_eq!(files.len(), 1);
            progress = true;
        }
    }
    assert!(progress, "扫描任务尚未结束也应发布文件增量");
    ipc::reconcile::remove_device(&state, &id);
    release.send(()).unwrap();
    // 扫描任务退出后也不得有晚到的 DeviceScanned。
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if state.supervisor.running_count() == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(state.supervisor.running_count(), 0);
    assert!(!state.devices.lock().unwrap().contains_key(&id));
    while let Ok(event) = events.try_recv() {
        assert!(!matches!(event, events::AppEvent::DeviceScanned { .. }));
    }
}
