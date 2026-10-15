//! Sony SDK runs in a separate helper process: callbacks and native SDK faults
//! never run on the UI thread. All control requests share one serialized channel.
use super::backend::{
    usb_vid_pid_from_pnp, CameraBackend, CameraInfo, CameraSetting, Capabilities, CapturedObject,
    LiveViewFrame, TetherError,
};
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::Duration;

fn helper_path() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(path) = std::env::var_os("PHOTO_HUB_SONY_BRIDGE") {
        candidates.push(path.into());
    }
    candidates.extend(crate::platform::sony_helper_candidates());
    candidates.into_iter().find(|p| p.is_file())
}
fn command() -> Result<Command, TetherError> {
    let path =
        helper_path().ok_or_else(|| TetherError::Other("尚未安装 Sony 联机拍摄支持".into()))?;
    let mut cmd = Command::new(&path);
    cmd.current_dir(path.parent().unwrap())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::platform::configure_background_command(&mut cmd);
    Ok(cmd)
}
fn enqueue_download(
    value: &Value, root: &std::path::Path,
    queue: &Mutex<VecDeque<CapturedObject>>, errors: &Mutex<Option<String>>,
) {
    let Some(path) = value["path"].as_str() else { return; };
    let path = PathBuf::from(path);
    let object = CapturedObject {
        object_id: path.to_string_lossy().into_owned(),
        object_name: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
        object_size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
    };
    if let Err(error) = super::staging::seal(root, &object) {
        *errors.lock().unwrap() = Some(error.to_string());
    }
    // The SDK event means download completed. Receipt failure must not stop
    // normal ingestion; only cross-session recovery requires its manifest.
    queue.lock().unwrap().push_back(object);
}

struct Transport {
    child: Child,
    input: ChildStdin,
    replies: mpsc::Receiver<Value>,
}
struct Connection {
    transport: Mutex<Transport>,
    objects: Arc<Mutex<VecDeque<CapturedObject>>>,
    connected: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    incoming: PathBuf,
    recovery: super::staging::Recovery,
}
impl Connection {
    fn spawn() -> Result<Self, TetherError> {
        let incoming = std::env::temp_dir()
            .join("Photographer-tethering")
            .join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(&incoming).map_err(|e| TetherError::Other(e.to_string()))?;
        let mut child = command()?
            .spawn()
            .map_err(|e| TetherError::Other(e.to_string()))?;
        let stdout = child.stdout.take().unwrap();
        let input = child.stdin.take().unwrap();
        let (tx, replies) = mpsc::channel();
        let objects = Arc::new(Mutex::new(VecDeque::new()));
        let queue = objects.clone();
        let connected = Arc::new(AtomicBool::new(true));
        let flag = connected.clone();
        let error = Arc::new(Mutex::new(None));
        let errors = error.clone();
        let receipt_root = incoming.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                match value["type"].as_str() {
                    Some("result") => {
                        if tx.send(value).is_err() {
                            break;
                        }
                    }
                    Some("photo") => {
                        enqueue_download(&value, &receipt_root, &queue, &errors);
                    }
                    Some("disconnected") => {
                        flag.store(false, Ordering::Release);
                    }
                    Some("cameraError") => {
                        *errors.lock().unwrap() = Some(
                            value["error"]
                                .as_str()
                                .unwrap_or("Sony camera error")
                                .to_string(),
                        );
                    }
                    _ => {}
                }
            }
            flag.store(false, Ordering::Release);
        });
        Ok(Self {
            transport: Mutex::new(Transport {
                child,
                input,
                replies,
            }),
            objects,
            connected,
            error,
            incoming,
            recovery: super::staging::Recovery::default(),
        })
    }
    fn request(&self, request: &str) -> Result<Value, TetherError> {
        if request.contains('\n') || request.contains('\r') {
            return Err(TetherError::Other("无效相机请求".into()));
        }
        let mut io = self.transport.lock().unwrap();
        if !self.connected.load(Ordering::Acquire) {
            return Err(TetherError::Disconnected);
        }
        writeln!(io.input, "{request}")
            .and_then(|_| io.input.flush())
            .map_err(|_| TetherError::Disconnected)?;
        let value = io
            .replies
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| {
                self.connected.store(false, Ordering::Release);
                let _ = io.child.kill();
                TetherError::Timeout
            })?;
        if let Some(error) = value["error"].as_str() {
            return Err(TetherError::Other(error.into()));
        }
        Ok(value)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(io) = self.transport.get_mut() {
            let _ = writeln!(io.input, "quit");
            let _ = io.input.flush();
            for _ in 0..30 {
                if io.child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            let _ = io.child.kill();
            let _ = io.child.wait();
        }
        // Downloaded originals are retained on disk if ingestion has not succeeded.
    }
}
#[derive(Default)]
pub struct SonyBackend {
    connections: Mutex<HashMap<String, Arc<Connection>>>,
}
impl SonyBackend {
    fn connection(&self, id: &str) -> Result<Arc<Connection>, TetherError> {
        self.connections
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or(TetherError::Disconnected)
    }
}
impl CameraBackend for SonyBackend {
    fn id(&self) -> &'static str {
        "sony-sdk"
    }
    fn name(&self) -> &'static str {
        "Sony Camera Remote SDK"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::FILE_TRANSFER
            .union(Capabilities::STANDARD_CAPTURE)
            .union(Capabilities::OBJECT_ADDED_EVENTS)
            .union(Capabilities::LIVE_VIEW)
    }
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        if helper_path().is_none() {
            return Ok(Vec::new());
        }
        let mut child = command()?
            .arg("--list")
            .spawn()
            .map_err(|e| TetherError::Other(e.to_string()))?;
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let values = BufReader::new(stdout)
                .lines()
                .map_while(Result::ok)
                .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
                .collect::<Vec<_>>();
            let _ = tx.send(values);
        });
        let result = rx.recv_timeout(Duration::from_secs(12));
        if result.is_err() {
            let _ = child.kill();
        }
        let _ = child.wait();
        let values = result.map_err(|_| TetherError::Timeout)?;
        let Some(value) = values.into_iter().find(|v| v["cameras"].is_array()) else {
            return Ok(Vec::new());
        };
        Ok(value["cameras"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|c| {
                let pnp_id = c["pnpId"].as_str()?;
                // 物理身份（§E）：桥端点报了就填，没报 None（诚实降级，
                // 不按型号名猜）。端点路径含 USB vid/pid 时一并解析。
                let (vid, pid) = usb_vid_pid_from_pnp(pnp_id);
                Some(CameraInfo {
                    pnp_id: pnp_id.into(),
                    name: c["name"].as_str()?.into(),
                    capabilities: self.capabilities(),
                    serial: c["serial"].as_str().map(str::to_string),
                    vid,
                    pid,
                })
            })
            .collect())
    }
    fn connect(&self, id: &str) -> Result<CameraInfo, TetherError> {
        let connection = Arc::new(Connection::spawn()?);
        connection.request(&format!("connect\t{id}\t{}", connection.incoming.display()))?;
        self.connections
            .lock()
            .unwrap()
            .insert(id.into(), connection);
        Ok(CameraInfo {
            pnp_id: id.into(),
            name: "Sony".into(),
            capabilities: self.capabilities(),
            // connect 返回不携带身份（桥协议未定义）；枚举路径填充。
            serial: None,
            vid: None,
            pid: None,
        })
    }
    fn disconnect(&self, id: &str) {
        self.connections.lock().unwrap().remove(id);
    }
    fn settings(&self, id: &str) -> Result<Vec<CameraSetting>, TetherError> {
        let value = self.connection(id)?.request("settings")?;
        serde_json::from_value(value["settings"].clone())
            .map_err(|e| TetherError::Other(e.to_string()))
    }
    fn set_setting(&self, id: &str, setting: &str, value: &str) -> Result<(), TetherError> {
        if !["shutter", "aperture", "iso"].contains(&setting) || value.parse::<u64>().is_err() {
            return Err(TetherError::Other("无效拍摄参数".into()));
        }
        self.connection(id)?
            .request(&format!("set\t{setting}\t{value}"))?;
        Ok(())
    }
    fn live_view_frame(&self, id: &str) -> Result<LiveViewFrame, TetherError> {
        let connection = self.connection(id)?;
        let value = connection.request("frame")?;
        let path = value["path"]
            .as_str()
            .ok_or_else(|| TetherError::Other("取景帧不可用".into()))?;
        std::fs::read(path)
            .map(LiveViewFrame::plain)
            .map_err(|e| TetherError::Other(e.to_string()))
    }
    fn trigger_capture(&self, id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        self.connection(id)?.request("capture")?;
        Ok(Vec::new())
    }
    fn poll_objects(&self, id: &str) -> Result<Vec<CapturedObject>, TetherError> {
        let connection = self.connection(id)?;
        let mut objects = connection.recovery.take();
        objects.extend(connection.objects.lock().unwrap().drain(..));
        if !objects.is_empty() { return Ok(objects); }
        if !connection.connected.load(Ordering::Acquire) {
            return Err(TetherError::Disconnected);
        }
        if let Some(error) = connection.error.lock().unwrap().take() {
            return Err(TetherError::Other(error));
        }
        Ok(objects)
    }
    fn request_objects(&self, id: &str, ids: &[String]) -> Result<Vec<String>, TetherError> {
        let connection = self.connection(id)?;
        Ok(connection.recovery.request(&connection.incoming, ids))
    }
    fn acknowledge_captured(&self, id: &str, object: &CapturedObject) -> Result<(), TetherError> {
        let connection = self.connection(id)?;
        let root = connection.recovery.root(&connection.incoming, object);
        let path = std::fs::canonicalize(&object.object_id).map_err(|e| TetherError::Other(e.to_string()))?;
        let root = std::fs::canonicalize(root).map_err(|e| TetherError::Other(e.to_string()))?;
        if !path.starts_with(root) { return Err(TetherError::AccessDenied); }
        std::fs::remove_file(&path).map_err(|e| TetherError::Other(e.to_string()))?;
        let mut manifest = path.as_os_str().to_owned();
        manifest.push(".photographer-receipt.json");
        let _ = std::fs::remove_file(PathBuf::from(manifest));
        Ok(())
    }
    fn open_captured(
        &self,
        id: &str,
        object: &CapturedObject,
    ) -> Result<Box<dyn Read + Send>, TetherError> {
        let connection = self.connection(id)?;
        let path = std::fs::canonicalize(&object.object_id).map_err(|e| TetherError::Other(e.to_string()))?;
        let root = std::fs::canonicalize(connection.recovery.root(&connection.incoming, object))
            .map_err(|e| TetherError::Other(e.to_string()))?;
        if !path.starts_with(root) {
            return Err(TetherError::AccessDenied);
        }
        std::fs::File::open(path)
            .map(|f| Box::new(f) as Box<dyn Read + Send>)
            .map_err(|e| TetherError::Other(e.to_string()))
    }
    fn capture_still(&self, id: &str, timeout: Duration) -> Result<CapturedObject, TetherError> {
        self.trigger_capture(id)?;
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(object) = self.poll_objects(id)?.into_iter().next() {
                return Ok(object);
            }
            if std::time::Instant::now() >= deadline {
                return Err(TetherError::Timeout);
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}
