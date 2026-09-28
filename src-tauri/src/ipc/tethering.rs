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
use tauri::State;

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
        let cameras = backend.enumerate().map_err(|e| e.to_string())?;
        out.extend(cameras.into_iter().map(CameraDto::from));
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
        cameras_from_backends(backend_registry().all())
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
