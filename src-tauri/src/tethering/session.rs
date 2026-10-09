//! A tethered session pins its destination library and album for its full lifetime.
//! Each completed download is copied and registered without creating an import job.
use super::backend::{backend_registry, CameraBackend, CameraSetting, CapturedObject, TetherError};
use crate::db::{AssetRow, Db};
use crate::events::AppEvent;
use crate::ipc::tethering::CameraDto;
use crate::ipc::SharedState;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex, MutexGuard, OnceLock,
};
use std::time::{Duration, Instant};

struct PendingCommand<'a>(&'a AtomicUsize);
impl Drop for PendingCommand<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}
pub struct CapturePermit<'a>(&'a AtomicBool);
impl Drop for CapturePermit<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TetherPhoto {
    pub id: i64,
    pub name: String,
    pub kind: String,
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDto {
    pub id: String,
    pub library_id: String,
    pub album_id: i64,
    pub album_name: String,
    pub camera: CameraDto,
    pub settings: Vec<CameraSetting>,
    pub photos: Vec<TetherPhoto>,
    pub connected: bool,
    pub receiving: bool,
    pub error: Option<String>,
}
/// 联拍落库归属（照片库基准）：目标照片库 id（photos_libraries.id，事件
/// 载荷 TetheringPhotoAdded.libraryId 同源）+ 库 root（纯时间布局锚点）。
#[derive(Debug, Clone)]
pub struct TetherLibrary {
    pub id: String,
    pub root: PathBuf,
}

pub struct Session {
    pub id: String,
    /// 落库归属照片库（2026-10-09 §三 联拍落库照片库基准）：id + root。
    pub library: TetherLibrary,
    pub album_id: i64,
    pub album_name: String,
    pub camera: CameraDto,
    pub backend: Arc<dyn CameraBackend>,
    pub operation: Mutex<()>,
    pending_commands: AtomicUsize,
    capture_in_flight: AtomicBool,
    cancelled: AtomicBool,
    disconnected: AtomicBool,
    connected: AtomicBool,
    receiving: AtomicBool,
    settings: Mutex<Vec<CameraSetting>>,
    photos: Mutex<Vec<TetherPhoto>>,
    seen: Mutex<HashSet<String>>,
    error: Mutex<Option<String>>,
}
/// 多会话存储：key = 会话 id。同一台相机（pnp_id）同一时刻只允许一个
/// 会话（USB 控制权互斥，物理事实）；不同相机可各自开会话，入册相册
/// 不限相同或不同。
#[derive(Default)]
pub struct SessionStore {
    sessions: std::collections::HashMap<String, Arc<Session>>,
}

impl SessionStore {
    pub fn insert(&mut self, session: Arc<Session>) {
        self.sessions.insert(session.id.clone(), session);
    }
    pub fn get(&self, id: &str) -> Option<Arc<Session>> {
        self.sessions.get(id).cloned()
    }
    pub fn remove(&mut self, id: &str) -> Option<Arc<Session>> {
        self.sessions.remove(id)
    }
    /// 该相机是否已被某个在册会话占用。
    pub fn camera_in_use(&self, pnp_id: &str) -> bool {
        self.sessions.values().any(|s| s.camera.pnp_id == pnp_id)
    }
    pub fn all(&self) -> Vec<Arc<Session>> {
        self.sessions.values().cloned().collect()
    }
}

fn current() -> &'static Mutex<SessionStore> {
    static CURRENT: OnceLock<Mutex<SessionStore>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(SessionStore::default()))
}

pub fn get(id: &str) -> Result<Arc<Session>, String> {
    current()
        .lock()
        .unwrap()
        .get(id)
        .filter(|session| !session.is_stopping())
        .ok_or_else(|| "拍摄会话已结束".into())
}
pub fn all() -> Vec<Arc<Session>> {
    current().lock().unwrap().all()
}
impl Session {
    pub fn claim_capture(&self) -> Result<CapturePermit<'_>, String> {
        self.ensure_open()?;
        if self.receiving.load(Ordering::Acquire) {
            return Err("相机正在接收照片，请稍候".into());
        }
        self.capture_in_flight
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "相机正在拍摄或接收照片，请稍候".to_string())?;
        Ok(CapturePermit(&self.capture_in_flight))
    }
    pub fn command<T>(
        &self,
        name: &str,
        action: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        self.pending_commands.fetch_add(1, Ordering::AcqRel);
        let _pending = PendingCommand(&self.pending_commands);
        let started = Instant::now();
        let _operation = self.operation.lock().unwrap();
        self.ensure_open()?;
        let result = action();
        if started.elapsed() >= Duration::from_millis(500) {
            crate::devices::diagnostics::record(format!(
                "tethering command={name} camera={} elapsed={:?} error={:?}",
                self.camera.pnp_id,
                started.elapsed(),
                result.as_ref().err()
            ));
        }
        result
    }
    pub fn try_background(&self) -> Option<MutexGuard<'_, ()>> {
        if self.is_stopping()
            || self.pending_commands.load(Ordering::Acquire) > 0
            || self.capture_in_flight.load(Ordering::Acquire)
        {
            return None;
        }
        let guard = self.operation.try_lock().ok()?;
        if self.pending_commands.load(Ordering::Acquire) > 0 || self.is_stopping() {
            return None;
        }
        Some(guard)
    }
    pub fn is_stopping(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
    pub fn ensure_open(&self) -> Result<(), String> {
        if self.is_stopping() || !self.connected.load(Ordering::Acquire) {
            return Err("拍摄会话已结束或相机已断开".into());
        }
        Ok(())
    }
    fn disconnect(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _operation = self.operation.lock().unwrap();
        if !self.disconnected.swap(true, Ordering::AcqRel) {
            self.backend.disconnect(&self.camera.pnp_id);
        }
        self.connected.store(false, Ordering::Release);
    }
    pub fn dto(&self) -> SessionDto {
        SessionDto {
            id: self.id.clone(),
            library_id: self.library.id.clone(),
            album_id: self.album_id,
            album_name: self.album_name.clone(),
            camera: self.camera.clone(),
            settings: self.settings.lock().unwrap().clone(),
            photos: self.photos.lock().unwrap().clone(),
            connected: self.connected.load(Ordering::Acquire),
            receiving: self.receiving.load(Ordering::Acquire),
            error: self.error.lock().unwrap().clone(),
        }
    }
    fn record_error(&self, state: &SharedState, error: String) {
        *self.error.lock().unwrap() = Some(error.clone());
        state.bus.publish(AppEvent::TetheringStatus {
            session_id: self.id.clone(),
            connected: self.connected.load(Ordering::Acquire),
            error: Some(error),
        });
    }
    pub fn refresh_settings(&self) -> Result<Vec<CameraSetting>, String> {
        self.ensure_open()?;
        let Some(_operation) = self.try_background() else {
            return Ok(self.settings.lock().unwrap().clone());
        };
        let settings = self
            .backend
            .settings(&self.camera.pnp_id)
            .map_err(|e| e.to_string())?;
        *self.settings.lock().unwrap() = settings.clone();
        Ok(settings)
    }
    pub fn set_setting(&self, id: &str, value: &str) -> Result<Vec<CameraSetting>, String> {
        self.command("set-parameter", || {
            self.backend
                .set_setting(&self.camera.pnp_id, id, value)
                .map_err(|e| e.to_string())?;
            // 写入和回读在同一次操作内，Sony 的异步属性需要以回读确认。
            let deadline = Instant::now() + Duration::from_millis(1500);
            loop {
                let settings = self
                    .backend
                    .settings(&self.camera.pnp_id)
                    .map_err(|e| e.to_string())?;
                let confirmed = settings.iter().any(|setting| {
                    setting.id == id
                        && (setting.current == value
                            || setting.kind == super::backend::SettingKind::Action
                            || (setting.kind == super::backend::SettingKind::Range
                                && setting
                                    .current
                                    .parse::<f64>()
                                    .ok()
                                    .zip(value.parse::<f64>().ok())
                                    .is_some_and(|(a, b)| (a - b).abs() < 0.0001)))
                });
                *self.settings.lock().unwrap() = settings.clone();
                if confirmed {
                    return Ok(settings);
                }
                self.ensure_open()?;
                if Instant::now() >= deadline {
                    return Err("相机未确认参数修改，请检查拍摄模式或机身控制".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    }
}
pub fn start(state: SharedState, album_id: i64, camera_id: &str) -> Result<SessionDto, String> {
    let mut guard = current().lock().unwrap();
    if guard.camera_in_use(camera_id) {
        return Err("该相机已在联机拍摄会话中，请先结束其窗口".into());
    }
    // 2026-10-09 §三：联拍落库切换照片库基准——目标 = 首个登记照片库
    //（多库时 UI 细化归属前取主库；无库明确报错）。
    let db = crate::ipc::app_database_db(&state)?;
    let library = db
        .photos_library_list()
        .map_err(|e| e.to_string())?
        .into_iter()
        .next()
        .ok_or("尚未建立照片库")?;
    let library = TetherLibrary {
        id: library.id,
        root: PathBuf::from(library.root_path),
    };
    let album_name: String =
        db.0.query_row("SELECT name FROM album WHERE id=?1", [album_id], |r| {
            r.get(0)
        })
        .map_err(|_| "相册不存在")?;
    let backend = backend_registry()
        .get(if camera_id.starts_with("sony-sdk:") {
            "sony-sdk"
        } else if camera_id.starts_with("gphoto:") {
            "gphoto"
        } else {
            "wpd-mtp"
        })
        .cloned()
        .ok_or("相机后端不可用")?;
    let mut camera: CameraDto = backend
        .connect(camera_id)
        .map_err(|e| e.to_string())?
        .into();
    if camera_id.starts_with("sony-sdk:") {
        camera.name = "Sony A7R V".into();
    }
    let settings = match backend.settings(camera_id) {
        Ok(value) => value,
        Err(e) => {
            backend.disconnect(camera_id);
            return Err(e.to_string());
        }
    };
    let session = Arc::new(Session {
        id: uuid::Uuid::new_v4().to_string(),
        library,
        album_id,
        album_name,
        camera,
        backend,
        operation: Mutex::new(()),
        pending_commands: AtomicUsize::new(0),
        capture_in_flight: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        disconnected: AtomicBool::new(false),
        connected: AtomicBool::new(true),
        receiving: AtomicBool::new(false),
        settings: Mutex::new(settings),
        photos: Mutex::new(Vec::new()),
        seen: Mutex::new(HashSet::new()),
        error: Mutex::new(None),
    });
    let dto = session.dto();
    guard.insert(session.clone());
    drop(guard);
    std::thread::spawn(move || {
        let mut notified_revision = session.backend.settings_revision(&session.camera.pnp_id);
        let mut last_notification = Instant::now();
        let mut consecutive_errors = 0;
        while !session.cancelled.load(Ordering::Acquire) {
            // 轮询也纳入操作锁：旧会话清理后绝不能再操作相机，影响新会话。
            let Some(operation) = session.try_background() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            if session.cancelled.load(Ordering::Acquire) {
                break;
            }
            match session.backend.poll_objects(&session.camera.pnp_id) {
                Ok(objects) => {
                    consecutive_errors = 0;
                    // 收片与用户操作（拍摄/参数）互斥：断开清理 incoming
                    // 目录时不会删到在途文件
                    for object in &objects {
                        if let Err(error) = receive(&state, &session, object) {
                            session.record_error(&state, error);
                        }
                    }
                    drop(operation);
                    let revision = session.backend.settings_revision(&session.camera.pnp_id);
                    // 一秒内的参数事件合并为一个失效通知，由窗口按需读取快照。
                    if revision != notified_revision
                        && last_notification.elapsed() >= Duration::from_secs(1)
                    {
                        state.bus.publish(AppEvent::TetheringSettingsChanged {
                            session_id: session.id.clone(),
                        });
                        notified_revision = revision;
                        last_notification = Instant::now();
                    }
                    // 排空模式（高速连拍）：拿到事件立即回轮（驱动侧事件
                    // 还在排队）；空轮才歇——固定 200ms 会把相机自拍收片
                    // 上限压到 ~5 张/秒
                    let pace = poll_pace_ms(objects.len());
                    if pace > 0 {
                        std::thread::sleep(Duration::from_millis(pace));
                    }
                }
                Err(error) => {
                    consecutive_errors += 1;
                    if matches!(error, TetherError::Disconnected | TetherError::AccessDenied)
                        || consecutive_errors >= 3
                    {
                        session.connected.store(false, Ordering::Release);
                        session.record_error(&state, error.to_string());
                        break;
                    }
                    session.record_error(&state, error.to_string());
                    drop(operation);
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
        // 与在途操作互斥后再断开（gp_camera_exit + 删收片暂存目录）
        stop(&session.id);
    });
    Ok(dto)
}
/// poller 节奏：空轮歇 200ms；拿到事件立即回轮（排空模式——高速连拍
/// 时驱动侧事件还在排队，固定间隔会把收片上限压到 ~5 张/秒）。
fn poll_pace_ms(received: usize) -> u64 {
    if received == 0 {
        200
    } else {
        0
    }
}

pub fn stop(id: &str) {
    // 断开完成后才解除相机占用；后端断开幂等，避免旧线程误断开新会话。
    let session = current().lock().unwrap().get(id);
    if let Some(session) = session {
        session.disconnect();
        current().lock().unwrap().remove(id);
    }
}
/// 窗口事件回调只发取消信号，真正断开交给后台，保持 UI 响应。
pub fn request_stop(id: &str) {
    // 建会话时可能持有存储锁；窗口回调不得等待它，后台 stop 会补发取消。
    if let Ok(store) = current().try_lock() {
        if let Some(session) = store.get(id) {
            session.cancelled.store(true, Ordering::Release);
            session.connected.store(false, Ordering::Release);
        }
    }
}
pub fn capture(state: &SharedState, id: &str) -> Result<(), String> {
    let session = get(id)?;
    let _capture = session.claim_capture()?;
    session.command("capture", || {
        *session.error.lock().unwrap() = None;
        let objects = match session.backend.trigger_capture(&session.camera.pnp_id) {
            Ok(objects) => objects,
            Err(error) => {
                // 拔线等硬断连：置断开态并发事件（UI 横幅），错误文案照常透传
                if matches!(error, TetherError::Disconnected) {
                    session.connected.store(false, Ordering::Release);
                    session.record_error(state, error.to_string());
                }
                return Err(error.to_string());
            }
        };
        for object in objects {
            receive(state, &session, &object)?;
        }
        Ok(())
    })
}
fn receive(state: &SharedState, session: &Session, object: &CapturedObject) -> Result<(), String> {
    if session.seen.lock().unwrap().contains(&object.object_id) {
        return Ok(());
    }
    if !crate::devices::is_media_ext(object.object_name.rsplit('.').next().unwrap_or("")) {
        return Ok(());
    }
    session.receiving.store(true, Ordering::Release);
    let result = (|| {
        let db = crate::ipc::app_database_db(state)?;
        let (photo, cleanup_error) = ingest_from_backend(
            &db,
            &session.library,
            session.album_id,
            session.backend.as_ref(),
            &session.camera.pnp_id,
            object,
        )?;
        session
            .seen
            .lock()
            .unwrap()
            .insert(object.object_id.clone());
        // Cleanup failure does not invalidate a committed asset or trigger a
        // duplicate ingestion. The download is retained for later recovery.
        if let Some(error) = cleanup_error {
            session.record_error(state, format!("照片已入库，暂存副本清理失败: {error}"));
        }
        {
            let mut photos = session.photos.lock().unwrap();
            photos.push(photo.clone());
            if photos.len() > 64 {
                photos.remove(0);
            }
        }
        state.bus.publish(AppEvent::TetheringPhotoAdded {
            session_id: session.id.clone(),
            library_id: session.library.id.clone(),
            album_id: session.album_id,
            asset_id: photo.id,
            name: photo.name,
        });
        let app_db_dir = crate::ipc::app_database_dir(state);
        crate::index::kick(app_db_dir.clone(), &state.supervisor);
        // 应用唯一数据库（§一）：AI 回填不再依赖库切换条件，直接触发。
        {
            let settings = state.settings.lock().unwrap().clone();
            if settings.ai.enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    app_db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            if settings.ai.enable_face {
                crate::ai::face::kick_face_if_ready(
                    app_db_dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            crate::ai::selection::kick_eyes_if_ready(
                app_db_dir,
                &state.ai,
                &state.bus,
                &state.supervisor,
            );
        }
        Ok(())
    })();
    session.receiving.store(false, Ordering::Release);
    result
}

fn ingest_from_backend(
    db: &Db,
    library: &TetherLibrary,
    album_id: i64,
    backend: &dyn CameraBackend,
    camera_id: &str,
    object: &CapturedObject,
) -> Result<(TetherPhoto, Option<String>), String> {
    let mut source = backend
        .open_captured(camera_id, object)
        .map_err(|e| e.to_string())?;
    let photo = ingest(
        db,
        library,
        album_id,
        &object.object_name,
        object.object_size,
        &mut source,
    )?;
    drop(source); // Windows cannot remove the local receipt while still open.
    let cleanup_error = backend
        .acknowledge_captured(camera_id, object)
        .err()
        .map(|e| e.to_string());
    Ok((photo, cleanup_error))
}

/// Shared by the real camera download path and regression tests; no job/journal rows.
/// 2026-10-09 §三：落盘布局 = 纯时间 `{库root}/{拍摄年}/{拍摄月}/{原文件名}`
///（EXIF 拍摄时间，缺失回退当前时刻——联拍文件刚落盘，mtime≈now）；
/// 相册只是逻辑引用（§一），不影响落位。
pub fn ingest(
    db: &Db,
    library: &TetherLibrary,
    album_id: i64,
    name: &str,
    expected_size: u64,
    source: &mut dyn Read,
) -> Result<TetherPhoto, String> {
    let filename = name
        .rsplit(['/', '\\'])
        .next()
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
        .ok_or("照片文件名无效")?;
    let kind = crate::devices::classify(filename, &[]);
    if !matches!(
        kind,
        crate::events::AssetKind::Photo | crate::events::AssetKind::Raw
    ) {
        return Err("仅接收照片文件".into());
    }
    // 先流式落 .part 到库根暂存（拍摄时间在读流后才能从 EXIF 解出，
    // 最终目录随后渲染），再原子改名进纯时间目录。
    std::fs::create_dir_all(&library.root).map_err(|e| e.to_string())?;
    let part = library
        .root
        .join(format!(".tether-{}.part", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&part)
            .map_err(|e| e.to_string())?;
        let mut head = Vec::new();
        let mut buf = vec![0u8; 4 * 1024 * 1024];
        let mut size = 0;
        let mut hash = xxhash_rust::xxh64::Xxh64::new(0);
        loop {
            let n = source.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            hash.update(&buf[..n]);
            size += n as u64;
            let take = (1024 * 1024 - head.len()).min(n);
            head.extend_from_slice(&buf[..take]);
        }
        if size == 0 || (expected_size > 0 && size != expected_size) {
            return Err("拍摄照片传输不完整，源文件已保留".into());
        }
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        let meta = crate::metadata::exif_lite::parse(&head);
        let captured =
            crate::import::templates::resolve_captured(meta.captured_at, chrono::Utc::now());
        let dir = library.root.join(captured.format("%Y/%m").to_string());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let destination = crate::ipc::claim::resolve_conflict(&dir, filename);
        std::fs::rename(&part, &destination).map_err(|e| e.to_string())?;
        let now = chrono::Utc::now().to_rfc3339();
        let filename = destination
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let row = AssetRow {
            path: destination.to_string_lossy().into_owned(),
            filename: filename.clone(),
            size,
            mtime: now.clone(),
            xxhash: hash.digest(),
            kind,
            captured_at: Some(
                meta.captured_at
                    .map(|t| t.to_rfc3339())
                    .unwrap_or_else(|| now.clone()),
            ),
            camera: meta.camera,
            source: "imported".into(),
            created_at: now,
            origin: "imported".into(),
            width: meta.width,
            height: meta.height,
            iso: meta.iso,
            f_number: meta.f_number,
            exposure_time: meta.exposure_time,
            focal_length: meta.focal_length,
            lens: meta.lens,
            pair_asset_id: None,
            thumb_state: 0,
            orientation: meta.deep.orientation,
            flash: meta.deep.flash,
            metering_mode: meta.deep.metering_mode,
            white_balance: meta.deep.white_balance,
            exposure_program: meta.deep.exposure_program,
            software: meta.deep.software,
            artist: meta.deep.artist,
            gps_lat: meta.deep.gps_lat,
            gps_lon: meta.deep.gps_lon,
            rating: 0,
            flagged: 0,
            color_label: None,
            rejected: 0,
            library_id: Some(library.id.clone()),
            missing: 0,
            xmp_dirty: 0,
            volume_serial: None,
            file_id: None,
        };
        // Check the album inside the transaction, so concurrent deletion cannot
        // silently register a photograph outside its selected destination.
        if !db.album_exists(album_id).map_err(|e| e.to_string())? {
            return Err(format!(
                "拍摄相册已删除；照片已保留在 {}",
                destination.display()
            ));
        }
        db.insert_captured_asset(&row, album_id)
            .map_err(|e| e.to_string())?;
        let id = db
            .asset_id_by_path(&row.path)
            .map_err(|e| e.to_string())?
            .ok_or("照片入册失败")?;
        Ok(TetherPhoto {
            id,
            name: filename,
            kind: kind.as_db_str().into(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct NoopBackend {
        disconnects: std::sync::atomic::AtomicUsize,
        settings_reads: AtomicUsize,
        parameter: Mutex<Option<(String, String)>>,
        disconnect_started: Mutex<Option<std::sync::mpsc::Sender<()>>>,
        disconnect_release: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    }
    impl CameraBackend for NoopBackend {
        fn id(&self) -> &'static str {
            "noop"
        }
        fn name(&self) -> &'static str {
            "noop"
        }
        fn capabilities(&self) -> super::super::backend::Capabilities {
            super::super::backend::Capabilities::FILE_TRANSFER
        }
        fn enumerate(&self) -> Result<Vec<super::super::backend::CameraInfo>, TetherError> {
            Ok(Vec::new())
        }
        fn connect(&self, _pnp_id: &str) -> Result<super::super::backend::CameraInfo, TetherError> {
            Err(TetherError::Other("noop".into()))
        }
        fn disconnect(&self, _pnp_id: &str) {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            if let Some(started) = self.disconnect_started.lock().unwrap().take() {
                started.send(()).unwrap();
            }
            if let Some(release) = self.disconnect_release.lock().unwrap().take() {
                release.recv_timeout(Duration::from_secs(5)).unwrap();
            }
        }
        fn set_setting(&self, _pnp_id: &str, id: &str, value: &str) -> Result<(), TetherError> {
            *self.parameter.lock().unwrap() = Some((id.into(), value.into()));
            self.settings_reads.store(0, Ordering::Release);
            Ok(())
        }
        fn settings(&self, _pnp_id: &str) -> Result<Vec<CameraSetting>, TetherError> {
            let reads = self.settings_reads.fetch_add(1, Ordering::AcqRel);
            let parameter = self.parameter.lock().unwrap();
            Ok(parameter
                .as_ref()
                .map(|(id, value)| CameraSetting {
                    id: id.clone(),
                    label: id.clone(),
                    kind: super::super::backend::SettingKind::Choice,
                    current: if reads == 0 {
                        "100".into()
                    } else {
                        value.clone()
                    },
                    writable: true,
                    options: Vec::new(),
                    min: None,
                    max: None,
                    step: None,
                })
                .into_iter()
                .collect())
        }
        fn capture_still(
            &self,
            _pnp_id: &str,
            _timeout: Duration,
        ) -> Result<CapturedObject, TetherError> {
            Err(TetherError::CaptureNotSupported)
        }
    }

    fn make_session(id: &str, camera_pnp: &str, album_id: i64) -> Arc<Session> {
        Arc::new(Session {
            id: id.to_string(),
            library: TetherLibrary {
                id: "lib".into(),
                root: PathBuf::from("unused"),
            },
            album_id,
            album_name: format!("相册{album_id}"),
            camera: crate::ipc::tethering::CameraDto {
                pnp_id: camera_pnp.to_string(),
                name: "假相机".into(),
                capabilities: crate::ipc::tethering::CameraCapabilitiesDto {
                    file_transfer: true,
                    standard_capture: false,
                    vendor_capture_nikon: false,
                    object_added_events: false,
                    live_view: false,
                },
            },
            backend: Arc::new(NoopBackend::default()),
            operation: Mutex::new(()),
            pending_commands: AtomicUsize::new(0),
            capture_in_flight: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            disconnected: AtomicBool::new(false),
            connected: AtomicBool::new(true),
            receiving: AtomicBool::new(false),
            settings: Mutex::new(Vec::new()),
            photos: Mutex::new(Vec::new()),
            seen: Mutex::new(HashSet::new()),
            error: Mutex::new(None),
        })
    }

    #[test]
    fn store_supports_multiple_sessions_same_or_different_albums() {
        let mut store = SessionStore::default();
        let a = make_session("s1", "gphoto:usb:001,010", 1);
        let b = make_session("s2", "gphoto:usb:001,011", 1); // 同相册不同相机
        let c = make_session("s3", "wpd:X", 2);
        store.insert(a.clone());
        store.insert(b.clone());
        store.insert(c.clone());
        assert!(
            store.get("s1").is_some() && store.get("s2").is_some() && store.get("s3").is_some()
        );
        assert_eq!(store.sessions.len(), 3);
        // 停止一个不影响其他会话
        let removed = store.remove("s2");
        assert!(removed.is_some());
        assert!(!removed.unwrap().cancelled.load(Ordering::Acquire)); // 置位由 stop() 全局入口做
        assert!(store.get("s1").is_some());
        assert!(store.get("s3").is_some());
        assert!(store.get("s2").is_none());
    }

    #[test]
    fn poll_pace_drains_burst_without_fixed_interval() {
        // 空轮限频、有事件立即回轮（高速连拍排空）
        assert_eq!(poll_pace_ms(0), 200);
        assert_eq!(poll_pace_ms(1), 0);
        assert_eq!(poll_pace_ms(10), 0);
    }

    #[test]
    fn store_rejects_second_session_on_same_camera() {
        let mut store = SessionStore::default();
        store.insert(make_session("s1", "gphoto:usb:001,010", 1));
        // 同相机不同相册也不行（USB 控制权互斥）
        assert!(store.camera_in_use("gphoto:usb:001,010"));
        assert!(!store.camera_in_use("gphoto:usb:001,011"));
        // 停止后可重开
        store.remove("s1");
        assert!(!store.camera_in_use("gphoto:usb:001,010"));
    }

    #[test]
    fn parameter_write_is_confirmed_by_readback_before_cache_update_completes() {
        let backend = Arc::new(NoopBackend::default());
        let mut session = make_session("confirm", "test-camera", 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        let settings = session.set_setting("iso", "400").unwrap();
        assert_eq!(settings[0].current, "400");
        assert_eq!(session.dto().settings[0].current, "400");
        assert_eq!(backend.settings_reads.load(Ordering::Acquire), 2);
        assert_eq!(session.pending_commands.load(Ordering::Acquire), 0);
    }

    #[test]
    fn background_refresh_uses_cache_without_queuing_behind_user_command() {
        let session = make_session("priority", "test-camera", 1);
        let operation = session.operation.lock().unwrap();
        let command_session = session.clone();
        let command = std::thread::spawn(move || command_session.command("test", || Ok(())));
        let deadline = Instant::now() + Duration::from_secs(5);
        while session.pending_commands.load(Ordering::Acquire) == 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(session.pending_commands.load(Ordering::Acquire) > 0);
        assert!(session.try_background().is_none());
        let (tx, rx) = std::sync::mpsc::channel();
        let background_session = session.clone();
        let background =
            std::thread::spawn(move || tx.send(background_session.refresh_settings()).unwrap());
        let cached = rx.recv_timeout(Duration::from_secs(1));
        drop(operation);
        command.join().unwrap().unwrap();
        background.join().unwrap();
        assert!(cached.unwrap().unwrap().is_empty());
        assert_eq!(session.pending_commands.load(Ordering::Acquire), 0);
    }

    #[test]
    fn duplicate_capture_is_rejected_and_failure_releases_busy_state() {
        let session = make_session("capture", "test-camera", 1);
        let capture = session.claim_capture().unwrap();
        assert!(session.claim_capture().is_err());
        assert!(session.try_background().is_none());
        drop(capture);
        let failed: Result<(), String> = (|| {
            let _capture = session.claim_capture()?;
            session.command("capture", || Err("camera error".into()))
        })();
        assert!(failed.is_err());
        assert!(session.claim_capture().is_ok());
    }

    #[test]
    fn stop_keeps_camera_reserved_until_disconnect_and_only_disconnects_once() {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let backend = Arc::new(NoopBackend {
            disconnect_started: Mutex::new(Some(started_tx)),
            disconnect_release: Mutex::new(Some(release_rx)),
            ..Default::default()
        });
        let id = uuid::Uuid::new_v4().to_string();
        let camera = format!("test-camera-{id}");
        let mut session = make_session(&id, &camera, 1);
        Arc::get_mut(&mut session).unwrap().backend = backend.clone();
        current().lock().unwrap().insert(session.clone());
        request_stop(&id);
        assert!(session.is_stopping());
        let stop_id = id.clone();
        let first = std::thread::spawn(move || stop(&stop_id));
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(current().lock().unwrap().camera_in_use(&camera));
        let stop_id = id.clone();
        let second = std::thread::spawn(move || stop(&stop_id));
        release_tx.send(()).unwrap();
        first.join().unwrap();
        second.join().unwrap();
        assert!(!current().lock().unwrap().camera_in_use(&camera));
        assert!(get(&id).is_err());
        assert_eq!(backend.disconnects.load(Ordering::SeqCst), 1);
        // 已释放的旧 worker 再清理，也不能断开该相机的新会话。
        let next_id = uuid::Uuid::new_v4().to_string();
        current()
            .lock()
            .unwrap()
            .insert(make_session(&next_id, &camera, 1));
        session.disconnect();
        stop(&id);
        assert!(get(&next_id).is_ok());
        assert_eq!(backend.disconnects.load(Ordering::SeqCst), 1);
        stop(&next_id);
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::super::backend::{CameraInfo, Capabilities, TetherError};
    use super::super::staging;
    use super::*;
    use std::path::Path;
    use std::sync::atomic::AtomicUsize;

    struct ReceiptBackend {
        root: PathBuf,
        acknowledged: AtomicUsize,
    }

    impl CameraBackend for ReceiptBackend {
        fn id(&self) -> &'static str {
            "receipt-test"
        }

        fn name(&self) -> &'static str {
            "receipt-test"
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::NONE
        }

        fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
            Ok(vec![])
        }

        fn connect(&self, _: &str) -> Result<CameraInfo, TetherError> {
            Err(TetherError::Disconnected)
        }

        fn disconnect(&self, _: &str) {
            staging::disconnect(&self.root);
        }

        fn capture_still(&self, _: &str, _: Duration) -> Result<CapturedObject, TetherError> {
            Err(TetherError::CaptureNotSupported)
        }

        fn open_captured(
            &self,
            _: &str,
            object: &CapturedObject,
        ) -> Result<Box<dyn Read + Send>, TetherError> {
            let file = std::fs::File::open(staging::validated_path(&self.root, object)?)
                .map_err(|error| TetherError::Other(error.to_string()))?;
            Ok(Box::new(file))
        }

        fn acknowledge_captured(
            &self,
            _: &str,
            object: &CapturedObject,
        ) -> Result<(), TetherError> {
            self.acknowledged.fetch_add(1, Ordering::SeqCst);
            staging::acknowledge(&self.root, object)
        }
    }

    #[test]
    fn failed_ingestion_keeps_receipt_and_success_acknowledges_only_after_commit() {
        let root = tempfile::tempdir().unwrap();
        let incoming = root.path().join("incoming");
        std::fs::create_dir(&incoming).unwrap();
        let db = Db::open(&root.path().join("library.db")).unwrap();
        let album = db.album_create("Capture").unwrap();
        let library = TetherLibrary {
            id: "lib-test".into(),
            root: root.path().join("photos"),
        };
        let backend = ReceiptBackend {
            root: incoming.clone(),
            acknowledged: AtomicUsize::new(0),
        };
        let receipt = staging::persist(&incoming, "original.jpg", b"retained bytes").unwrap();
        let mut truncated = receipt.clone();
        truncated.object_size += 1;
        assert!(
            ingest_from_backend(&db, &library, album.id, &backend, "camera", &truncated).is_err()
        );
        backend.disconnect("camera");
        assert_eq!(
            std::fs::read(&receipt.object_id).unwrap(),
            b"retained bytes"
        );
        assert_eq!(backend.acknowledged.load(Ordering::SeqCst), 0);

        let (photo, error) =
            ingest_from_backend(&db, &library, album.id, &backend, "camera", &receipt).unwrap();
        assert!(error.is_none());
        assert_eq!(backend.acknowledged.load(Ordering::SeqCst), 1);
        assert!(!Path::new(&receipt.object_id).exists());
        let path: String =
            db.0.query_row("SELECT path FROM assets WHERE id = ?1", [photo.id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"retained bytes");
    }
}
