//! 阶段 E-1 联拍底座单测（全部离线，不接真机——真机清单见
//! src/tethering/mod.rs 文档）。
//!
//! - GetDeviceInfo 数据集二进制解析（标准容器 / 截断 / 空列表）；
//! - 能力位映射（0x100E / 0x90C0 / 事件交叉验证）；
//! - 拍摄操作码选择与参数组装（透传纯函数面）；
//! - trait 桩：假后端验证注册表 / 超时逻辑 / 错误语义 / 事件契约
//!   （COM 边界经 `&dyn CameraBackend` seam 隔离）。

mod common;

pub use common::{
    platform, scan,
    ai, bursts, db, devices, events, import, index, geo, ipc, metadata, settings, tasks,
    tethering, thumbs,
};

use std::sync::Arc;
use std::time::{Duration, Instant};

use events::{AppEvent, EventBus};
use ipc::tethering::{
    cameras_from_backends, capture_with_backend, probe_with_backend, CameraCapabilitiesDto,
    CameraDto, CaptureResultDto, CAPTURE_TIMEOUT_MESSAGE,
};
use tethering::backend::{CameraBackend, CameraInfo, Capabilities, CapturedObject, TetherError};
use tethering::mtp::{
    self, capture_command, get_device_info_command, parse_device_info, DeviceInfoDataset,
};

/// 手动真机验证：只读参数和取景，不触发快门；正常测试不会访问 USB。
#[test]
#[ignore = "requires a connected PC Remote camera"]
fn gphoto_connected_camera_smoke() {
    let backend = tethering::gphoto_backend::GphotoBackend::default();
    let cameras = backend.enumerate().expect("enumerate camera");
    let expected = std::env::var("PHOTO_HUB_TEST_CAMERA_MODEL").expect("set expected camera model");
    let camera = cameras.iter().find(|c| c.name.contains(&expected)).expect("camera detected");
    println!("Detected: {} ({})", camera.name, camera.pnp_id);
    backend.connect(&camera.pnp_id).expect("connect camera");
    // 无论验证成功还是失败，都先释放 USB 会话供应用使用。
    let started = Instant::now();
    let settings = backend.settings(&camera.pnp_id);
    let settings_elapsed = started.elapsed();
    let frame = backend.live_view_frame(&camera.pnp_id);
    backend.disconnect(&camera.pnp_id);
    let settings = settings.expect("read camera settings");
    println!("Settings: {}, elapsed: {:?}", settings.len(), settings_elapsed);
    assert!(!settings.is_empty());
    for setting in &settings {
        if ["iso", "shutterspeed", "f-number", "aperture"].contains(&setting.id.as_str()) {
            println!("{}: {}", setting.id, setting.current);
        }
    }
    let frame = frame.expect("read live view");
    println!("Live view: {} bytes", frame.len());
    assert!(frame.starts_with(&[0xff, 0xd8]), "JPEG live view frame");
}

// ---------------------------------------------------------------------------
// GetDeviceInfo 数据集 fixture 构造（小端）
// ---------------------------------------------------------------------------

/// 构造 PTP 字符串：u8 字符数(含 NUL) + N×u16。
fn ptp_string(s: &str) -> Vec<u8> {
    let mut out = vec![(s.chars().count() + 1) as u8];
    for ch in s.chars() {
        out.extend_from_slice(&(ch as u16).to_le_bytes());
    }
    out.extend_from_slice(&0u16.to_le_bytes()); // NUL
    out
}

/// 构造 AUINT16 容器：u32 元素数 + N×u16。
fn au16(values: &[u16]) -> Vec<u8> {
    let mut out = (values.len() as u32).to_le_bytes().to_vec();
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// 标准形态数据集（ Nikon D750 世代探测结果的抽象形态）。
fn standard_dataset() -> Vec<u8> {
    let mut data = Vec::new();
    data.extend_from_slice(&100u16.to_le_bytes()); // StandardVersion (1.0)
    data.extend_from_slice(&6u32.to_le_bytes()); // VendorExtensionID (MTP)
    data.extend_from_slice(&100u16.to_le_bytes()); // VendorExtensionVersion
    data.extend_from_slice(&ptp_string("MTP")); // VendorExtensionDesc
    data.extend_from_slice(&0u16.to_le_bytes()); // FunctionalMode
    data.extend_from_slice(&au16(&[
        0x1001, // GetDeviceInfo
        0x1002, // OpenSession
        0x100E, // InitiateCapture
        0x90C0, // Nikon Capture（厂商扩展）
        0x1009, // GetObject
    ]));
    data.extend_from_slice(&au16(&[0x4002, 0x4003])); // ObjectAdded / DeviceInfoChanged
                                                      // 尾部（DevicePropsSupported 及以后）本解析器不读——不构造
    data
}

fn as_info(data: &[u8]) -> Result<DeviceInfoDataset, mtp::ParseDeviceInfoError> {
    parse_device_info(data)
}

// ---------------------------------------------------------------------------
// 解析：标准 / 截断 / 空列表
// ---------------------------------------------------------------------------

#[test]
fn parse_device_info_standard_container() {
    let dataset = as_info(&standard_dataset()).expect("标准数据集应解析成功");
    assert_eq!(dataset.standard_version, 100);
    assert_eq!(dataset.vendor_extension_id, 6);
    assert_eq!(dataset.vendor_extension_version, 100);
    assert_eq!(
        dataset.operations_supported,
        vec![0x1001, 0x1002, 0x100E, 0x90C0, 0x1009]
    );
    assert_eq!(dataset.events_supported, vec![0x4002, 0x4003]);
}

#[test]
fn parse_device_info_truncated_reports_field() {
    for cut in [1usize, 5, 9, 18, 22, 26] {
        let full = standard_dataset();
        let truncated = &full[..cut.min(full.len())];
        let err = as_info(truncated).expect_err("截断数据集必须报错");
        assert!(
            !err.to_string().is_empty(),
            "截断错误须携带字段上下文（cut={cut}）"
        );
    }
    // 从 OperationsSupported 中间截断（32 = 数组计数后第 2 个元素中段）
    let full = standard_dataset();
    let err = as_info(&full[..30]).expect_err("数组中段截断必须报错");
    assert!(
        err.0.contains("OperationsSupported"),
        "错误应指明字段: {err}"
    );
}

#[test]
fn parse_device_info_empty_and_absent_lists() {
    let mut data = Vec::new();
    data.extend_from_slice(&100u16.to_le_bytes());
    data.extend_from_slice(&0u32.to_le_bytes());
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&ptp_string(""));
    data.extend_from_slice(&0u16.to_le_bytes());
    data.extend_from_slice(&au16(&[])); // 空操作列表合法
    data.extend_from_slice(&au16(&[])); // 空事件列表合法
    let dataset = as_info(&data).expect("空列表应解析成功");
    assert!(dataset.operations_supported.is_empty());
    assert!(dataset.events_supported.is_empty());
}

// ---------------------------------------------------------------------------
// 能力位映射
// ---------------------------------------------------------------------------

#[test]
fn capabilities_from_probe_nikon_profile() {
    let caps = mtp::capabilities_from_probe(
        &[0x1001, 0x1002, 0x100E, 0x90C0],
        &[0x4002],
        false, // WPD 侧未报告 OBJECT_ADDED——PTP 侧在列即可
    );
    assert!(caps.file_transfer());
    assert!(caps.standard_capture());
    assert!(caps.vendor_capture_nikon());
    assert!(caps.object_added_events());
    assert!(!caps.live_view());
}

#[test]
fn capabilities_event_cross_validation_both_ways() {
    // 只有 WPD 事件在列（MTP EventsSupported 缺失）
    let caps = mtp::capabilities_from_probe(&[0x1001], &[], true);
    assert!(caps.object_added_events());
    assert!(!caps.standard_capture());
    assert!(!caps.vendor_capture_nikon());
    // 两边都没有 → 事件能力为假
    let caps = mtp::capabilities_from_probe(&[0x1001], &[0x4003], false);
    assert!(!caps.object_added_events());
}

// ---------------------------------------------------------------------------
// 拍摄操作码选择（透传参数组装纯函数）
// ---------------------------------------------------------------------------

#[test]
fn capture_command_prefers_nikon_vendor_code() {
    let both = Capabilities::STANDARD_CAPTURE.union(Capabilities::VENDOR_CAPTURE_NIKON);
    let cmd = capture_command(both).expect("双能力应选厂商码");
    assert_eq!(cmd.code, 0x90C0);
    assert_eq!(cmd.params, vec![0x1]);
    // 仅 Nikon
    let cmd = capture_command(Capabilities::VENDOR_CAPTURE_NIKON).expect("nikon");
    assert_eq!(cmd.code, 0x90C0);
    assert_eq!(cmd.params.len(), 1);
}

#[test]
fn capture_command_standard_when_no_vendor() {
    let cmd = capture_command(Capabilities::STANDARD_CAPTURE).expect("标准码");
    assert_eq!(cmd.code, 0x100E);
    assert_eq!(cmd.params, vec![0x0, 0x0]); // StorageID=0 + ObjectFormat=0
}

#[test]
fn capture_command_unsupported_without_bits() {
    let err = capture_command(Capabilities::FILE_TRANSFER).expect_err("无拍摄码应拒绝");
    assert!(matches!(err, TetherError::CaptureNotSupported));
    let err = capture_command(Capabilities::NONE).expect_err("空能力应拒绝");
    assert!(matches!(err, TetherError::CaptureNotSupported));
}

#[test]
fn get_device_info_command_shape() {
    let cmd = get_device_info_command();
    assert_eq!(cmd.code, mtp::PTP_OP_GET_DEVICE_INFO);
    assert!(cmd.params.is_empty());
    assert_eq!(mtp::PTP_RESP_OK, 0x2001);
}

// ---------------------------------------------------------------------------
// 假后端（COM 边界 seam）：注册表 / 超时 / 错误语义 / 事件契约
// ---------------------------------------------------------------------------

/// 假后端行为模式。
enum FakeMode {
    /// 立即返回拍摄成功。
    Ok(CapturedObject),
    /// 睡眠超过调用方超时（模拟「未响应」）。
    Hang,
    /// 返回指定错误。
    Fail(TetherError),
}

struct FakeBackend {
    mode: FakeMode,
    enumerate: Vec<CameraInfo>,
}

impl CameraBackend for FakeBackend {
    fn id(&self) -> &'static str {
        "fake-backend"
    }
    fn name(&self) -> &'static str {
        "测试后端"
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities::FILE_TRANSFER.union(Capabilities::STANDARD_CAPTURE)
    }
    fn enumerate(&self) -> Result<Vec<CameraInfo>, TetherError> {
        Ok(self.enumerate.clone())
    }
    fn connect(&self, pnp_id: &str) -> Result<CameraInfo, TetherError> {
        match &self.mode {
            FakeMode::Fail(err) => Err(clone_error(err)),
            _ => Ok(CameraInfo {
                pnp_id: pnp_id.to_string(),
                name: "假相机".into(),
                capabilities: Capabilities::FILE_TRANSFER
                    .union(Capabilities::STANDARD_CAPTURE)
                    .union(Capabilities::OBJECT_ADDED_EVENTS),
            }),
        }
    }
    fn disconnect(&self, _pnp_id: &str) {}
    fn capture_still(
        &self,
        _pnp_id: &str,
        timeout: Duration,
    ) -> Result<CapturedObject, TetherError> {
        match &self.mode {
            FakeMode::Ok(object) => Ok(object.clone()),
            FakeMode::Hang => {
                let started = Instant::now();
                // 模拟驱动不回事件：睡眠至超时再报 Timeout（真实现是
                // Condvar 等待超时——此处等价验证调用方语义）
                std::thread::sleep(timeout + Duration::from_millis(20));
                let _ = started;
                Err(TetherError::Timeout)
            }
            FakeMode::Fail(err) => Err(clone_error(err)),
        }
    }
}

/// TetherError 克隆（thiserror 无 Clone；测试桩需要）。
fn clone_error(err: &TetherError) -> TetherError {
    match err {
        TetherError::Timeout => TetherError::Timeout,
        TetherError::CaptureNotSupported => TetherError::CaptureNotSupported,
        TetherError::AccessDenied => TetherError::AccessDenied,
        TetherError::Disconnected => TetherError::Disconnected,
        TetherError::Other(m) => TetherError::Other(m.clone()),
    }
}

const FAKE_PNP: &str = r"\\?\usb#vid_fake&pid_fake#0#{6ac27878-a6fa-4155-ba85-f98f491d4f33}";

fn fake_camera(name: &str, caps: Capabilities) -> CameraInfo {
    CameraInfo {
        pnp_id: FAKE_PNP.to_string(),
        name: name.to_string(),
        capabilities: caps,
    }
}

#[test]
fn cameras_from_backends_maps_dto() {
    let backend = FakeBackend {
        mode: FakeMode::Ok(CapturedObject {
            object_id: "o1".into(),
            object_name: "DSC_0001.NEF".into(),
            object_size: 1,
        }),
        enumerate: vec![fake_camera(
            "Nikon D750",
            Capabilities::FILE_TRANSFER
                .union(Capabilities::VENDOR_CAPTURE_NIKON)
                .union(Capabilities::OBJECT_ADDED_EVENTS),
        )],
    };
    let backends: Vec<Arc<dyn CameraBackend>> = vec![Arc::new(backend)];
    let list = cameras_from_backends(&backends).expect("枚举成功");
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0],
        CameraDto {
            pnp_id: FAKE_PNP.to_string(),
            name: "Nikon D750".into(),
            capabilities: CameraCapabilitiesDto {
                file_transfer: true,
                standard_capture: false,
                vendor_capture_nikon: true,
                object_added_events: true,
                live_view: false,
            },
        }
    );
}

#[test]
fn capture_success_returns_dto_and_publishes_event() {
    let backend = FakeBackend {
        mode: FakeMode::Ok(CapturedObject {
            object_id: "o77".into(),
            object_name: "DSC_0102.JPG".into(),
            object_size: 8_000_000,
        }),
        enumerate: vec![],
    };
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let dto = capture_with_backend(&backend, &bus, FAKE_PNP, Duration::from_secs(5));
    assert_eq!(
        dto,
        CaptureResultDto {
            object_name: Some("DSC_0102.JPG".into()),
            object_size: Some(8_000_000),
            error: None,
        }
    );
    // 事件总线收到 tetheringObjectAdded（转发器→app://event 由既有链路负责）
    let event = rx.try_recv().expect("应发布收片事件");
    match event {
        AppEvent::TetheringObjectAdded {
            pnp_id,
            object_name,
            object_size,
        } => {
            assert_eq!(pnp_id, FAKE_PNP);
            assert_eq!(object_name, "DSC_0102.JPG");
            assert_eq!(object_size, 8_000_000);
        }
        other => panic!("应为 TetheringObjectAdded，实际 {other:?}"),
    }
}

#[test]
fn capture_timeout_maps_to_contract_message() {
    // 契约文案钉死（与 ipc::tethering::CAPTURE_TIMEOUT_MESSAGE 一致）
    assert_eq!(TetherError::Timeout.to_string(), CAPTURE_TIMEOUT_MESSAGE);
    let backend = FakeBackend {
        mode: FakeMode::Hang,
        enumerate: vec![],
    };
    let bus = EventBus::new();
    let started = Instant::now();
    let dto = capture_with_backend(&backend, &bus, FAKE_PNP, Duration::from_millis(80));
    assert!(started.elapsed() >= Duration::from_millis(80), "应等满超时");
    assert_eq!(
        dto,
        CaptureResultDto {
            object_name: None,
            object_size: None,
            error: Some(CAPTURE_TIMEOUT_MESSAGE.into()),
        }
    );
    // 超时不应发布收片事件
    assert!(bus.subscribe().try_recv().is_err());
}

#[test]
fn capture_business_failures_carry_error_field() {
    for err in [
        TetherError::CaptureNotSupported,
        TetherError::AccessDenied,
        TetherError::Disconnected,
        TetherError::Other("驱动拒绝".into()),
    ] {
        let backend = FakeBackend {
            mode: FakeMode::Fail(err),
            enumerate: vec![],
        };
        let dto =
            capture_with_backend(&backend, &EventBus::new(), FAKE_PNP, Duration::from_secs(1));
        assert!(dto.error.is_some(), "业务失败必须落在 error 字段");
        assert!(dto.object_name.is_none());
        assert!(dto.object_size.is_none());
    }
}

#[test]
fn probe_failure_returns_none_not_error() {
    // 契约：camera_probe 失败 → null（Option::None），不抛错
    let backend = FakeBackend {
        mode: FakeMode::Fail(TetherError::AccessDenied),
        enumerate: vec![],
    };
    assert!(probe_with_backend(&backend, FAKE_PNP).is_none());

    let ok_backend = FakeBackend {
        mode: FakeMode::Hang,
        enumerate: vec![],
    };
    let dto = probe_with_backend(&ok_backend, FAKE_PNP).expect("探测成功应返回 DTO");
    assert_eq!(dto.pnp_id, FAKE_PNP);
    assert!(dto.capabilities.standard_capture);
    assert!(dto.capabilities.object_added_events);
    assert!(!dto.capabilities.live_view);
}

#[test]
fn cached_capabilities_merge_by_normalized_id() {
    use std::collections::BTreeMap;
    // 大小写两种 PnP 形态（启动枚举小写 / 热插到达大写）应命中同一缓存
    let mut cache = BTreeMap::new();
    cache.insert(
        FAKE_PNP.to_ascii_lowercase(),
        Capabilities::FILE_TRANSFER.union(Capabilities::VENDOR_CAPTURE_NIKON),
    );
    let devices = vec![fake_camera(
        "D750",
        Capabilities::FILE_TRANSFER, // 未探测默认位
    )];
    let merged = tethering::backend::apply_cached_capabilities(devices, &cache);
    assert_eq!(merged.len(), 1);
    assert!(merged[0].capabilities.vendor_capture_nikon());
    assert!(!merged[0].capabilities.standard_capture());
}

#[test]
fn tethering_event_serializes_camel_case() {
    // 前端契约：type=tetheringObjectAdded，字段 pnpId/objectName/objectSize
    let event = AppEvent::TetheringObjectAdded {
        pnp_id: "pnp-1".into(),
        object_name: "IMG_1.NEF".into(),
        object_size: 123,
    };
    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["type"], "tetheringObjectAdded");
    assert_eq!(json["pnpId"], "pnp-1");
    assert_eq!(json["objectName"], "IMG_1.NEF");
    assert_eq!(json["objectSize"], 123);
}

#[test]
fn camera_dto_serializes_camel_case() {
    let dto = CameraDto {
        pnp_id: FAKE_PNP.into(),
        name: "D750".into(),
        capabilities: CameraCapabilitiesDto {
            file_transfer: true,
            standard_capture: false,
            vendor_capture_nikon: true,
            object_added_events: true,
            live_view: false,
        },
    };
    let json = serde_json::to_value(&dto).expect("serialize");
    assert_eq!(json["pnpId"], FAKE_PNP);
    assert_eq!(json["capabilities"]["vendorCaptureNikon"], true);
    assert_eq!(json["capabilities"]["objectAddedEvents"], true);
    assert_eq!(json["capabilities"]["liveView"], false);
}
