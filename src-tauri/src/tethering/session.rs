//! A tethered session pins its destination library and album for its full lifetime.
//! Each completed download is copied and registered without creating an import job.
use super::backend::{backend_registry, CameraBackend, CameraSetting, CapturedObject};
use crate::db::{AssetRow, Db};
use crate::events::AppEvent;
use crate::ipc::tethering::CameraDto;
use crate::ipc::SharedState;
use crate::settings::Library;
use serde::Serialize;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::time::Duration;
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
pub struct Session {
    pub id: String,
    pub library: Library,
    pub album_id: i64,
    pub album_name: String,
    pub camera: CameraDto,
    pub backend: Arc<dyn CameraBackend>,
    pub operation: Mutex<()>,
    cancelled: AtomicBool,
    connected: AtomicBool,
    receiving: AtomicBool,
    settings: Mutex<Vec<CameraSetting>>,
    photos: Mutex<Vec<TetherPhoto>>,
    seen: Mutex<HashSet<String>>,
    error: Mutex<Option<String>>,
}
fn current() -> &'static Mutex<Option<Arc<Session>>> {
    static CURRENT: OnceLock<Mutex<Option<Arc<Session>>>> = OnceLock::new();
    CURRENT.get_or_init(|| Mutex::new(None))
}
pub fn get(id: &str) -> Result<Arc<Session>, String> {
    current()
        .lock()
        .unwrap()
        .as_ref()
        .filter(|s| s.id == id)
        .cloned()
        .ok_or_else(|| "拍摄会话已结束".into())
}
pub fn active() -> Option<SessionDto> {
    current().lock().unwrap().as_ref().map(|s| s.dto())
}
impl Session {
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
        let _operation = self.operation.lock().unwrap();
        let settings = self
            .backend
            .settings(&self.camera.pnp_id)
            .map_err(|e| e.to_string())?;
        *self.settings.lock().unwrap() = settings.clone();
        Ok(settings)
    }
}
pub fn start(state: SharedState, album_id: i64, camera_id: &str) -> Result<SessionDto, String> {
    let mut guard = current().lock().unwrap();
    if guard.is_some() {
        return Err("已有联机拍摄窗口，请先结束当前拍摄".into());
    }
    crate::ipc::ensure_library_not_migrating(&state)?;
    let library = state
        .settings
        .lock()
        .unwrap()
        .active_library()
        .cloned()
        .ok_or("尚未选择图库")?;
    let db = crate::ipc::open_library_db(Path::new(&library.db_dir))?;
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
        cancelled: AtomicBool::new(false),
        connected: AtomicBool::new(true),
        receiving: AtomicBool::new(false),
        settings: Mutex::new(settings),
        photos: Mutex::new(Vec::new()),
        seen: Mutex::new(HashSet::new()),
        error: Mutex::new(None),
    });
    let dto = session.dto();
    *guard = Some(session.clone());
    drop(guard);
    std::thread::spawn(move || {
        while !session.cancelled.load(Ordering::Acquire) {
            match session.backend.poll_objects(&session.camera.pnp_id) {
                Ok(objects) => {
                    for object in objects {
                        if let Err(error) = receive(&state, &session, &object) {
                            session.record_error(&state, error);
                        }
                    }
                }
                Err(error) => {
                    session.connected.store(false, Ordering::Release);
                    session.record_error(&state, error.to_string());
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        session.backend.disconnect(&session.camera.pnp_id);
    });
    Ok(dto)
}
pub fn stop(id: &str) {
    let mut guard = current().lock().unwrap();
    if guard.as_ref().is_some_and(|s| s.id == id) {
        if let Some(session) = guard.take() {
            session.cancelled.store(true, Ordering::Release);
        }
    }
}
pub fn capture(state: &SharedState, id: &str) -> Result<(), String> {
    let session = get(id)?;
    let _operation = session.operation.lock().unwrap();
    if !session.connected.load(Ordering::Acquire) {
        return Err("相机已断开".into());
    }
    *session.error.lock().unwrap() = None;
    let objects = session
        .backend
        .trigger_capture(&session.camera.pnp_id)
        .map_err(|e| e.to_string())?;
    for object in objects {
        receive(state, &session, &object)?;
    }
    Ok(())
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
        let db = crate::ipc::open_library_db(Path::new(&session.library.db_dir))?;
        let mut source = session
            .backend
            .open_captured(&session.camera.pnp_id, object)
            .map_err(|e| e.to_string())?;
        let photo = ingest(
            &db,
            &session.library,
            session.album_id,
            &object.object_name,
            object.object_size,
            &mut source,
        )?;
        session
            .seen
            .lock()
            .unwrap()
            .insert(object.object_id.clone());
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
        crate::index::kick(PathBuf::from(&session.library.db_dir), &state.supervisor);
        let settings = state.settings.lock().unwrap().clone();
        if settings.active_library_id.as_deref() == Some(&session.library.id) {
            let dir = PathBuf::from(&session.library.db_dir);
            if settings.ai.enable_clip {
                crate::ai::semantic::kick_semantic_if_ready(
                    dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            if settings.ai.enable_face {
                crate::ai::face::kick_face_if_ready(
                    dir.clone(),
                    &state.ai,
                    &state.bus,
                    &state.supervisor,
                );
            }
            crate::ai::selection::kick_eyes_if_ready(dir, &state.ai, &state.bus, &state.supervisor);
        }
        Ok(())
    })();
    session.receiving.store(false, Ordering::Release);
    result
}
/// Shared by the real camera download path and regression tests; no job/journal rows.
pub fn ingest(
    db: &Db,
    library: &Library,
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
    let home = db
        .album_item_home_rel(album_id, None)
        .map_err(|e| e.to_string())?
        .ok_or("拍摄相册已删除")?;
    let dir = Path::new(&library.photo_root).join(home.replace('/', std::path::MAIN_SEPARATOR_STR));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let destination = crate::ipc::claim::resolve_conflict(&dir, filename);
    let part = dir.join(format!(".tether-{}.part", uuid::Uuid::new_v4()));
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
        std::fs::rename(&part, &destination).map_err(|e| e.to_string())?;
        let meta = crate::metadata::exif_lite::parse(&head);
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
