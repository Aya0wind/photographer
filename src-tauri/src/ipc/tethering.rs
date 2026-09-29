//! 联机拍摄命令（阶段 E-1）：`tethering_camera_list` / `camera_probe` /
//! `camera_capture`。
//!
//! 命名说明：设备枚举命令带 `tethering_` 前缀——`camera_list` 已被
//! ipc::assets（相机型号计数，FilterPanel 在用）占用，Tauri 命令名全局
//! 唯一（宏导出冲突）。`camera_probe` / `camera_capture` 无撞名，按契约
//! 原名。
//!
//! 契约（与前端 lane 定稿，字段不得偏移）：
//! - `camera_probe` 探测失败返回 **null**（前端按「未探测」降级，不抛错）；
//! - `camera_capture` 业务失败（超时/不支持/拒绝）一律返回
//!   `{ objectName: null, objectSize: null, error: "…" }` 形态，仅 IPC 级
//!   异常走 Err；超时文案「相机未响应拍摄命令」；
//! - 事件 `tetheringObjectAdded { pnpId, objectName, objectSize }` 进
//!   `app://event`（拍摄成功收片即发）。
//!
//! 铁律：WPD COM / 秒级探测 / 长等待一律 async + run_blocking 后台执行。
//! 无 DB 表——联拍会话态不入库（瞬态；导入落库走既有导入管线）。

use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{Manager, State};

use super::{run_blocking, SharedState};
use crate::events::{AppEvent, EventBus};
// super::super：lib 树解析为 crate::tethering；tests 的 #[path] 包含树
// 解析为 common::tethering（tethering 不在各测试文件的标准再导出清单，
// crate:: 绝对路径在测试 crate 根不可达——与 edit 模块不进测试树不同，
// 本文件经 ipc 随树编译，必须双上下文可解析）。
use super::super::tethering::backend::{backend_registry, CameraBackend, CameraInfo};

/// 相机能力 DTO（布尔位与 [`crate::tethering::backend::Capabilities`]
/// 一一对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraCapabilitiesDto {
    pub file_transfer: bool,
    pub standard_capture: bool,
    pub vendor_capture_nikon: bool,
    pub object_added_events: bool,
    pub live_view: bool,
}

/// 联拍相机 DTO（tethering_camera_list / camera_probe 返回）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraDto {
    pub pnp_id: String,
    pub name: String,
    pub capabilities: CameraCapabilitiesDto,
}

impl From<CameraInfo> for CameraDto {
    fn from(info: CameraInfo) -> Self {
        let caps = info.capabilities;
        Self {
            pnp_id: info.pnp_id,
            name: info.name,
            capabilities: CameraCapabilitiesDto {
                file_transfer: caps.file_transfer(),
                standard_capture: caps.standard_capture(),
                vendor_capture_nikon: caps.vendor_capture_nikon(),
                object_added_events: caps.object_added_events(),
                live_view: caps.live_view(),
            },
        }
    }
}

/// 拍摄结果 DTO（camera_capture 返回）：error=null 即成功；业务失败时
/// objectName/objectSize 为 null。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureResultDto {
    pub object_name: Option<String>,
    pub object_size: Option<u64>,
    pub error: Option<String>,
}

/// 拍摄等待默认/上限/下限（毫秒）：机身反光板-卡写入有秒级延迟，
/// 默认 15s；用户可传 timeout_ms 调整（命令层钳制，核心函数不重复钳）。
const DEFAULT_CAPTURE_TIMEOUT_MS: u64 = 15_000;
const MIN_CAPTURE_TIMEOUT_MS: u64 = 1_000;
const MAX_CAPTURE_TIMEOUT_MS: u64 = 120_000;

fn clamp_capture_timeout(timeout_ms: Option<u64>) -> Duration {
    Duration::from_millis(
        timeout_ms
            .unwrap_or(DEFAULT_CAPTURE_TIMEOUT_MS)
            .clamp(MIN_CAPTURE_TIMEOUT_MS, MAX_CAPTURE_TIMEOUT_MS),
    )
}

// ---------------------------------------------------------------------------
// 核心编排（可测：后端经参数注入，COM 边界被 trait seam 隔离）
// ---------------------------------------------------------------------------

/// 枚举全部后端可见的相机（不逐台探测；已探测设备带缓存能力位）。
pub fn cameras_from_backends(
    backends: &[std::sync::Arc<dyn CameraBackend>],
) -> Result<Vec<CameraDto>, String> {
    let mut out = Vec::new();
    for backend in backends {
        match backend.enumerate() {
            Ok(cameras) => out.extend(cameras.into_iter().map(CameraDto::from)),
            Err(error) => crate::devices::diagnostics::record(format!(
                "Camera enumeration {} failed: {error}",
                backend.id()
            )),
        }
    }
    Ok(out)
}

/// 探测单台相机：成功返回 DTO；失败返回 None（契约：null=未探测降级，
/// 不向前端抛错）。
pub fn probe_with_backend(backend: &dyn CameraBackend, pnp_id: &str) -> Option<CameraDto> {
    backend.connect(pnp_id).ok().map(CameraDto::from)
}

/// 拍摄一次并转发收片事件：成功 → 发布 `tetheringObjectAdded` + 成功
/// DTO；业务失败（含超时「相机未响应拍摄命令」）→ error 字段承载。
pub fn capture_with_backend(
    backend: &dyn CameraBackend,
    bus: &EventBus,
    pnp_id: &str,
    timeout: Duration,
) -> CaptureResultDto {
    match backend.capture_still(pnp_id, timeout) {
        Ok(object) => {
            bus.publish(AppEvent::TetheringObjectAdded {
                pnp_id: pnp_id.to_string(),
                object_name: object.object_name.clone(),
                object_size: object.object_size,
            });
            CaptureResultDto {
                object_name: Some(object.object_name),
                object_size: Some(object.object_size),
                error: None,
            }
        }
        Err(err) => CaptureResultDto {
            object_name: None,
            object_size: None,
            error: Some(err.to_string()),
        },
    }
}

/// 超时错误文案（契约常量；测试断言 `TetherError::Timeout` 文案与此一致）。
#[allow(dead_code)] // lib 内无直接调用点（文案由 TetherError::Timeout 携带）
pub const CAPTURE_TIMEOUT_MESSAGE: &str = "相机未响应拍摄命令";

// ---------------------------------------------------------------------------
// 命令（async + run_blocking；WPD COM 绝不上主线程）
// ---------------------------------------------------------------------------

/// 枚举可联拍相机（WPD 设备 + 已探测能力缓存；不逐台 Open——探测昂贵，
/// camera_probe 单独做）。
#[tauri::command]
pub async fn tethering_camera_list(
    state: State<'_, SharedState>,
) -> Result<Vec<CameraDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |_| {
        let mut cameras = cameras_from_backends(backend_registry().all())?;
        // USB 已被本应用占用时，驱动枚举可能省略该相机；仍提供唤起入口。
        for session in super::super::tethering::session::all() {
            if !session.is_stopping() && !cameras.iter().any(|c| c.pnp_id == session.camera.pnp_id)
            {
                cameras.push(session.camera.clone());
            }
        }
        Ok(cameras)
    })
    .await
}

/// 全量探测指定相机（Open + MTP GetDeviceInfo + WPD 能力交叉），刷新
/// 能力缓存。探测失败/设备不可用返回 null（前端按「未探测」降级）。
#[tauri::command]
pub async fn camera_probe(
    state: State<'_, SharedState>,
    pnp_id: String,
) -> Result<Option<CameraDto>, String> {
    let shared = state.inner().clone();
    run_blocking(shared, move |_| {
        Ok(probe_with_backend(
            backend_registry().primary().as_ref(),
            &pnp_id,
        ))
    })
    .await
}

/// 触发一次拍摄（按能力选 0x90C0/0x100E），等待 OBJECT_ADDED 收片。
/// 业务失败（超时/不支持/拒绝/断开）返回 `{ objectName: null,
/// objectSize: null, error }`；仅 IPC 级异常走 Err。成功即向
/// `app://event` 发布 `tetheringObjectAdded`。
#[tauri::command]
pub async fn camera_capture(
    state: State<'_, SharedState>,
    pnp_id: String,
    timeout_ms: Option<u64>,
) -> Result<CaptureResultDto, String> {
    let shared = state.inner().clone();
    let timeout = clamp_capture_timeout(timeout_ms);
    run_blocking(shared, move |state| {
        Ok(capture_with_backend(
            backend_registry().primary().as_ref(),
            &state.bus,
            &pnp_id,
            timeout,
        ))
    })
    .await
}
// The dedicated window shares the app state, but each session pins its destination.
#[tauri::command]
pub async fn tethering_start(
    app: tauri::AppHandle,
    state: State<'_, SharedState>,
    album_id: i64,
    camera_id: String,
) -> Result<super::super::tethering::session::SessionDto, String> {
    let shared = state.inner().clone();
    // 串行化查窗、清理和建窗，避免并发点击把尚未建窗的新会话视为孤儿。
    static START_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _start = START_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    for session in super::super::tethering::session::all() {
        let window = app.get_webview_window(&format!("tethering-{}", session.id));
        if window.is_none() || session.is_stopping() {
            let id = session.id.clone();
            tauri::async_runtime::spawn_blocking(move || {
                super::super::tethering::session::stop(&id);
            })
            .await
            .map_err(|e| e.to_string())?;
            if let Some(window) = window {
                let _ = window.close();
            }
        } else if session.camera.pnp_id == camera_id {
            let window = window.unwrap();
            window.unminimize().map_err(|e| e.to_string())?;
            window.show().map_err(|e| e.to_string())?;
            window.set_focus().map_err(|e| e.to_string())?;
            return Ok(session.dto());
        }
    }
    let dto = tauri::async_runtime::spawn_blocking(move || {
        super::super::tethering::session::start(shared, album_id, &camera_id)
    })
    .await
    .map_err(|e| e.to_string())??;
    let id = dto.id.clone();
    // 窗口 label 按会话 id 隔离：多相机可同时各开一个联拍窗口
    let window = tauri::WebviewWindowBuilder::new(
        &app,
        format!("tethering-{id}"),
        tauri::WebviewUrl::App(format!("tethering?session={id}").into()),
    )
    .title("Photo Hub · Tethered Capture")
    .inner_size(1100.0, 760.0)
    .min_inner_size(860.0, 600.0)
    .decorations(false)
    .build();
    match window {
        Ok(window) => {
            let close_id = id.clone();
            window.on_window_event(move |event| {
                if matches!(
                    event,
                    tauri::WindowEvent::CloseRequested { .. } | tauri::WindowEvent::Destroyed
                ) {
                    super::super::tethering::session::request_stop(&close_id);
                    let id = close_id.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        super::super::tethering::session::stop(&id);
                    });
                }
            });
            Ok(dto)
        }
        Err(e) => {
            tauri::async_runtime::spawn_blocking(move || {
                super::super::tethering::session::stop(&id);
            })
            .await
            .map_err(|error| error.to_string())?;
            Err(format!("无法打开拍摄窗口：{e}"))
        }
    }
}
#[tauri::command]
pub async fn tethering_session(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<super::super::tethering::session::SessionDto, String> {
    run_blocking(state.inner().clone(), move |_| {
        let session = super::super::tethering::session::get(&session_id)?;
        Ok(session.dto())
    })
    .await
}
#[tauri::command]
pub async fn tethering_settings(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<Vec<super::super::tethering::backend::CameraSetting>, String> {
    run_blocking(state.inner().clone(), move |_| {
        super::super::tethering::session::get(&session_id)?.refresh_settings()
    })
    .await
}
#[tauri::command]
pub async fn tethering_setting_set(
    state: State<'_, SharedState>,
    session_id: String,
    id: String,
    value: String,
) -> Result<Vec<super::super::tethering::backend::CameraSetting>, String> {
    run_blocking(state.inner().clone(), move |_| {
        let session = super::super::tethering::session::get(&session_id)?;
        {
            let _guard = session.operation.lock().unwrap();
            session.ensure_open()?;
            session
                .backend
                .set_setting(&session.camera.pnp_id, &id, &value)
                .map_err(|e| e.to_string())?;
        }
        session.refresh_settings()
    })
    .await
}
#[tauri::command]
pub async fn tethering_focus_at(
    state: State<'_, SharedState>,
    session_id: String,
    x: f64,
    y: f64,
) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |_| {
        let session = super::super::tethering::session::get(&session_id)?;
        let _guard = session.operation.lock().unwrap();
        session.ensure_open()?;
        session
            .backend
            .focus_at(&session.camera.pnp_id, x, y)
            .map_err(|e| e.to_string())
    })
    .await
}
#[tauri::command]
pub async fn tethering_capture(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<(), String> {
    let shared = state.inner().clone();
    let work = shared.clone();
    run_blocking(shared, move |_| {
        super::super::tethering::session::capture(&work, &session_id)
    })
    .await
}
#[tauri::command]
pub async fn tethering_frame(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<Option<String>, String> {
    run_blocking(state.inner().clone(), move |_| {
        let session = super::super::tethering::session::get(&session_id)?;
        let Ok(_guard) = session.operation.try_lock() else {
            return Ok(None);
        };
        if session.ensure_open().is_err() {
            return Ok(None);
        }
        let bytes = session
            .backend
            .live_view_frame(&session.camera.pnp_id)
            .map_err(|e| e.to_string())?;
        use base64::Engine;
        Ok(Some(format!(
            "data:image/jpeg;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )))
    })
    .await
}
#[tauri::command]
pub async fn tethering_photo_preview(
    state: State<'_, SharedState>,
    session_id: String,
    asset_id: i64,
    size: u16,
) -> Result<Option<String>, String> {
    run_blocking(state.inner().clone(), move |_| {
        let session = super::super::tethering::session::get(&session_id)?;
        let db = super::open_library_db(std::path::Path::new(&session.library.db_dir))?;
        let row = db
            .asset_by_id(asset_id)
            .map_err(|e| e.to_string())?
            .ok_or("照片不存在")?;
        let path = crate::thumbs::thumb_file(
            std::path::Path::new(&session.library.db_dir),
            std::path::Path::new(&row.path),
            if size > 512 { 2048 } else { 256 },
        );
        match path {
            Some(path) => {
                use base64::Engine;
                let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
                Ok(Some(format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(bytes)
                )))
            }
            None => Ok(None),
        }
    })
    .await
}
#[tauri::command]
pub async fn tethering_stop(
    state: State<'_, SharedState>,
    session_id: String,
) -> Result<(), String> {
    run_blocking(state.inner().clone(), move |_| {
        super::super::tethering::session::stop(&session_id);
        Ok(())
    })
    .await
}
