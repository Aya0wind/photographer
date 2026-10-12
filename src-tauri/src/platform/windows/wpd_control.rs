// Runs inside wpd_backend::com. No COM objects cross the timed worker boundary.
use super::super::super::backend::{CameraSetting, SettingKind};
use super::super::super::ptp_control::{self as ptp, Brand, Property, PtpTransport, SettingSpec};
use super::super::super::vendor_dispatch;
use super::*;
use windows::Win32::Devices::PortableDevices::{
    WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_WRITE, WPD_COMMAND_MTP_EXT_WRITE_DATA,
    WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_WRITE,
    WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_WRITTEN,
};

pub(super) struct WpdTransport {
    pub(super) device: IPortableDevice,
}
impl PtpTransport for WpdTransport {
    fn read(
        &mut self,
        operation: u16,
        params: &[u32],
        limit: usize,
    ) -> Result<(u32, Vec<u8>), String> {
        let (response, data) = execute_with_data_limit(&self.device, operation, params, limit)
            .map_err(|e| e.to_string())?;
        if data.len() > limit {
            return Err("PTP 数据超过请求上限".into());
        }
        Ok((response, data))
    }
    fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
        execute_without_data(&self.device, operation, params).map_err(|e| e.to_string())
    }
    fn write(&mut self, operation: u16, params: &[u32], data: &[u8]) -> Result<u32, String> {
        write_data(&self.device, operation, params, data).map_err(|e| e.to_string())
    }
}

fn write_data(
    device: &IPortableDevice,
    operation: u16,
    params: &[u32],
    data: &[u8],
) -> DeviceResult<u32> {
    if data.len() > 4096 {
        return Err(DeviceError::Other("相机参数数据过大".into()));
    }
    let values = build_command_values(
        &WPD_COMMAND_MTP_EXT_EXECUTE_COMMAND_WITH_DATA_TO_WRITE,
        operation,
        params,
    )?;
    // SAFETY: static property key; sizes are bounded above.
    unsafe {
        values.SetUnsignedLargeIntegerValue(
            &WPD_PROPERTY_MTP_EXT_TRANSFER_TOTAL_DATA_SIZE,
            data.len() as u64,
        )
    }
    .map_err(wpd_com::win_error)?;
    let result = send_values(device, &values)?;
    // SAFETY: the result owns this allocated NUL-terminated context string.
    let pointer = unsafe { result.GetStringValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT) }
        .map_err(wpd_com::win_error)?;
    let context = unsafe { wpd_com::pwstr_to_string(pointer) };
    unsafe { wpd_com::free_pwstr(pointer) };
    if context.is_empty() {
        return Err(DeviceError::Other("MTP 参数写入缺少传输上下文".into()));
    }
    let written = (|| -> DeviceResult<u32> {
        let values = base_command_values(&WPD_COMMAND_MTP_EXT_WRITE_DATA)?;
        let wide = wpd_com::to_wide(&context);
        // SAFETY: owned buffers remain alive through SendCommand; the collection copies them.
        unsafe {
            values.SetStringValue(
                &WPD_PROPERTY_MTP_EXT_TRANSFER_CONTEXT,
                windows::core::PCWSTR(wide.as_ptr()),
            )
        }
        .map_err(wpd_com::win_error)?;
        unsafe {
            values.SetUnsignedIntegerValue(
                &WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_TO_WRITE,
                data.len() as u32,
            )
        }
        .map_err(wpd_com::win_error)?;
        unsafe { values.SetBufferValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_DATA, data) }
            .map_err(wpd_com::win_error)?;
        let result = send_values(device, &values)?;
        unsafe { result.GetUnsignedIntegerValue(&WPD_PROPERTY_MTP_EXT_TRANSFER_NUM_BYTES_WRITTEN) }
            .map_err(wpd_com::win_error)
    })();
    // Complete the transaction even when its data phase fails.
    let end = end_transfer(device, &context);
    if written? as usize != data.len() {
        return Err(DeviceError::Other("MTP 参数未完整写入".into()));
    }
    response_code(&end?)
}

/// Brand + advertised operations, read once per settings transaction from
/// GetDeviceInfo. Brand detection lives in ptp_control; vendor gating in
/// vendor_dispatch — this layer only moves bytes.
struct DeviceProfile {
    brand: Brand,
    operations: Vec<u16>,
}

fn device_profile<T: PtpTransport>(transport: &mut T) -> Result<DeviceProfile, String> {
    let (response, data) = transport.read(mtp::PTP_OP_GET_DEVICE_INFO, &[], 256 * 1024)?;
    ptp::require_ok(response, mtp::PTP_OP_GET_DEVICE_INFO)?;
    let parsed = mtp::parse_device_info(&data).map_err(|e| e.to_string())?;
    Ok(DeviceProfile {
        brand: ptp::detect_brand(
            &parsed.manufacturer,
            &parsed.model,
            &parsed.operations_supported,
        ),
        operations: parsed.operations_supported,
    })
}

/// Read the core property descriptors.
///
/// Error semantics (plan §A descriptors): a single property answering
/// 0x200A (DevicePropNotSupported) / 0xA006 (vendor skip) or returning an
/// empty descriptor body is *unsupported here* → skip that property. Any
/// other failure — transport/HRESULT/IO error, unexpected response code,
/// malformed descriptor — propagates as Err instead of silently degrading
/// the whole list to "no settings".
fn descriptors<T: PtpTransport>(
    transport: &mut T,
    brand: Brand,
) -> Result<Vec<(SettingSpec, Property)>, String> {
    let mut properties: Vec<(SettingSpec, Property)> = Vec::new();
    for &spec in ptp::CORE_PROPERTIES {
        // Vendor D1xx codes only exist on the matching vendor.
        if spec.code >= 0xd000 && !vendor_dispatch::profile(brand).vendor_properties_supported() {
            continue;
        }
        if properties
            .iter()
            .any(|(previous, _)| previous.id == spec.id)
        {
            continue;
        }
        let (response, bytes) =
            transport.read(ptp::GET_PROP_DESC, &[spec.code as u32], 64 * 1024)?;
        // Vendor "not supported here" answers skip just this property…
        if response == ptp::RESP_PROP_NOT_SUPPORTED || response == ptp::RESP_NIKON_PROP_SKIP {
            continue;
        }
        // …any other non-OK response (GeneralError, AccessDenied…) is a real
        // failure and must surface instead of degrading the whole list.
        ptp::require_ok(response, ptp::GET_PROP_DESC)?;
        // OK without a descriptor body: the property is not exposed here.
        if bytes.is_empty() {
            continue;
        }
        let property = Property::parse(&bytes)?;
        if property.code != spec.code {
            return Err("相机返回的参数编号不匹配".into());
        }
        properties.push((spec, property));
    }
    Ok(properties)
}

fn autofocus_action() -> CameraSetting {
    CameraSetting {
        id: "autofocus".into(),
        label: "Autofocus".into(),
        kind: SettingKind::Action,
        current: "0".into(),
        writable: true,
        options: Vec::new(),
        min: None,
        max: None,
        step: None,
    }
}

/// Transport-independent settings assembly (mock-testable): vendor protocol
/// routing, property reads and the autofocus action listing.
fn camera_settings<T: PtpTransport>(
    transport: &mut T,
    profile: &DeviceProfile,
) -> Result<Vec<CameraSetting>, String> {
    let vendor = vendor_dispatch::profile(profile.brand);
    // Sony 等自带参数布局的厂商走厂商协议（含远端会话握手），不经
    // GetDevicePropDesc 探测；Nikon/标准机型维持标准描述路径。
    if vendor.handles_properties() {
        return vendor.settings(transport);
    }
    let properties = descriptors(transport, profile.brand)?;
    let mut settings: Vec<CameraSetting> = properties
        .iter()
        .map(|(spec, property)| property.setting(*spec, profile.brand))
        .collect();
    if vendor.autofocus_supported(&profile.operations) {
        settings.push(autofocus_action());
    }
    Ok(settings)
}

fn open(id: &str) -> DeviceResult<WpdTransport> {
    Ok(WpdTransport {
        device: wpd_com::open_device(id)?,
    })
}

pub(crate) fn settings(id: &str) -> DeviceResult<Vec<CameraSetting>> {
    let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
    let mut transport = open(id)?;
    let profile = device_profile(&mut transport).map_err(DeviceError::Other)?;
    camera_settings(&mut transport, &profile).map_err(DeviceError::Other)
}

pub(crate) fn set_setting(id: &str, setting: &str, value: &str) -> DeviceResult<()> {
    let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
    let mut transport = open(id)?;
    let profile = device_profile(&mut transport).map_err(DeviceError::Other)?;
    let vendor = vendor_dispatch::profile(profile.brand);
    if setting == "autofocus" {
        if value != "1" {
            return Err(DeviceError::Other("对焦动作参数无效".into()));
        }
        return vendor
            .autofocus(&mut transport, &profile.operations)
            .map_err(DeviceError::Other);
    }
    if vendor.handles_properties() {
        return vendor
            .set_setting(&mut transport, setting, value)
            .map_err(DeviceError::Other);
    }
    let properties = descriptors(&mut transport, profile.brand).map_err(DeviceError::Other)?;
    let (_, property) = properties
        .into_iter()
        .find(|(spec, _)| spec.id == setting)
        .ok_or_else(|| DeviceError::Other("相机不支持此参数".into()))?;
    let bytes = property.encode(value).map_err(DeviceError::Other)?;
    let response = transport
        .write(ptp::SET_PROP_VALUE, &[property.code as u32], &bytes)
        .map_err(DeviceError::Other)?;
    ptp::require_ok(response, ptp::SET_PROP_VALUE).map_err(DeviceError::Other)
}

/// 点击取景对焦（无原生取景连接时的回退路径，独立连接）：无帧头可用，
/// 各厂商 profile 退化为整帧 AF 驱动。主路径见 native::focus_at（帧头
/// 坐标换算 + 选区 + AF）。
pub(crate) fn focus_at(id: &str, x: f64, y: f64) -> DeviceResult<()> {
    let _com = wpd_com::ComApartment::init().map_err(wpd_com::win_error)?;
    let mut transport = open(id)?;
    let profile = device_profile(&mut transport).map_err(DeviceError::Other)?;
    vendor_dispatch::profile(profile.brand)
        .focus_at(&mut transport, &profile.operations, x, y, None)
        .map_err(DeviceError::Other)
}

#[cfg(test)]
mod native_control_tests {
    use super::*;
    #[test]
    #[ignore = "reads camera settings from an explicitly selected connected camera"]
    fn live_wpd_settings() {
        let id = std::env::var("PHOTOGRAPHER_TEST_CAMERA").expect("set PHOTOGRAPHER_TEST_CAMERA");
        let values = settings(&id).unwrap();
        for setting in &values {
            println!(
                "{} current={} writable={} options={:?}",
                setting.id, setting.current, setting.writable, setting.options
            );
        }
        for required in ["iso", "f-number", "shutterspeed", "whitebalance"] {
            assert!(
                values.iter().any(|setting| setting.id == required),
                "missing {required}"
            );
        }
    }
}

#[cfg(test)]
mod dispatch_tests {
    use super::*;
    use std::collections::BTreeMap;

    fn ptp_string(s: &str) -> Vec<u8> {
        let mut out = vec![(s.chars().count() + 1) as u8];
        for ch in s.chars() {
            out.extend_from_slice(&(ch as u16).to_le_bytes());
        }
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn au16(values: &[u16]) -> Vec<u8> {
        let mut out = (values.len() as u32).to_le_bytes().to_vec();
        for v in values {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    fn device_info(manufacturer: &str, operations: &[u16]) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&6u32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&ptp_string("MTP"));
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&au16(operations));
        data.extend_from_slice(&au16(&[0x4002]));
        data.extend_from_slice(&au16(&[]));
        data.extend_from_slice(&au16(&[]));
        data.extend_from_slice(&au16(&[]));
        data.extend_from_slice(&ptp_string(manufacturer));
        data.extend_from_slice(&ptp_string("TEST MODEL"));
        data
    }

    /// u16-enum property descriptor: writable, current 200, options 100/200.
    fn prop_desc(code: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(code.to_le_bytes());
        bytes.extend(4u16.to_le_bytes());
        bytes.push(1);
        bytes.extend(0u16.to_le_bytes());
        bytes.extend(200u16.to_le_bytes());
        bytes.push(2);
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(100u16.to_le_bytes());
        bytes.extend(200u16.to_le_bytes());
        bytes
    }

    enum DescOutcome {
        Ok(Vec<u8>),
        Response(u32),
        Fail(&'static str),
    }

    /// Scripted transport: one DeviceInfo payload plus per-code descriptor
    /// outcomes. Unscripted properties answer DevicePropNotSupported.
    struct MockTransport {
        info: Vec<u8>,
        outcomes: BTreeMap<u16, DescOutcome>,
        probed: Vec<u16>,
        commands: Vec<(u16, Vec<u32>)>,
    }

    impl PtpTransport for MockTransport {
        fn read(
            &mut self,
            operation: u16,
            params: &[u32],
            _limit: usize,
        ) -> Result<(u32, Vec<u8>), String> {
            match operation {
                mtp::PTP_OP_GET_DEVICE_INFO => Ok((0x2001, self.info.clone())),
                ptp::GET_PROP_DESC => {
                    let code = params[0] as u16;
                    self.probed.push(code);
                    match self.outcomes.get(&code) {
                        Some(DescOutcome::Ok(bytes)) => Ok((0x2001, bytes.clone())),
                        Some(DescOutcome::Response(code)) => Ok((*code, Vec::new())),
                        Some(DescOutcome::Fail(message)) => Err((*message).into()),
                        None => Ok((ptp::RESP_PROP_NOT_SUPPORTED, Vec::new())),
                    }
                }
                _ => Err("unexpected read operation".into()),
            }
        }
        fn write(
            &mut self,
            _operation: u16,
            _params: &[u32],
            _data: &[u8],
        ) -> Result<u32, String> {
            Err("unexpected write operation".into())
        }
        fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
            self.commands.push((operation, params.to_vec()));
            Ok(0x2001)
        }
    }

    fn nikon_profile() -> DeviceProfile {
        DeviceProfile {
            brand: Brand::Nikon,
            operations: vec![0x90c1],
        }
    }

    #[test]
    fn unsupported_property_responses_skip_only_that_property() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert(0xd100, DescOutcome::Response(ptp::RESP_PROP_NOT_SUPPORTED));
        outcomes.insert(0x5007, DescOutcome::Response(ptp::RESP_NIKON_PROP_SKIP));
        outcomes.insert(0x5005, DescOutcome::Ok(vec![])); // empty body → skip
        outcomes.insert(0x500d, DescOutcome::Ok(prop_desc(0x500d)));
        outcomes.insert(0x500f, DescOutcome::Ok(prop_desc(0x500f)));
        outcomes.insert(0x500a, DescOutcome::Ok(prop_desc(0x500a)));
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[0x90c1]),
            outcomes,
            probed: Vec::new(),
            commands: Vec::new(),
        };
        let settings = camera_settings(&mut transport, &nikon_profile()).unwrap();
        let ids: Vec<&str> = settings.iter().map(|s| s.id.as_str()).collect();
        // Skipped: Nikon shutter (falls through to 0x500D), aperture, WB.
        assert_eq!(ids, vec!["shutterspeed", "iso", "focusmode", "autofocus"]);
        // 0xD100 skipped → the standard shutter code was still probed.
        assert!(transport.probed.contains(&0x500d));
        assert!(transport.probed.contains(&0xd100));
        // ISO listing shows the current value from its descriptor.
        let iso = settings.iter().find(|s| s.id == "iso").unwrap();
        assert_eq!(iso.current, "200");
    }

    #[test]
    fn transport_errors_propagate_instead_of_empty_lists() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert(0x500f, DescOutcome::Fail("HRESULT 0x80070005"));
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[0x90c1]),
            outcomes,
            probed: Vec::new(),
            commands: Vec::new(),
        };
        // 0xD100 answers unsupported first (script default), then ISO fails at
        // the transport layer → the whole read must surface the error.
        let error = camera_settings(&mut transport, &nikon_profile()).unwrap_err();
        assert!(error.contains("HRESULT"), "unexpected error: {error}");
    }

    #[test]
    fn unexpected_response_codes_are_not_silently_skipped() {
        let mut outcomes = BTreeMap::new();
        outcomes.insert(0xd100, DescOutcome::Response(0x2002)); // GeneralError
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[0x90c1]),
            outcomes,
            probed: Vec::new(),
            commands: Vec::new(),
        };
        assert!(camera_settings(&mut transport, &nikon_profile()).is_err());
    }

    #[test]
    fn vendor_property_codes_are_probed_for_nikon_only() {
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        assert!(camera_settings(
            &mut transport,
            &DeviceProfile {
                brand: Brand::Nikon,
                operations: vec![],
            }
        )
        .is_ok());
        assert!(transport.probed.contains(&0xd100));
        assert!(transport.probed.contains(&0xd061));

        let mut transport = MockTransport {
            info: device_info("Generic", &[]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        assert!(camera_settings(
            &mut transport,
            &DeviceProfile {
                brand: Brand::Standard,
                operations: vec![],
            }
        )
        .is_ok());
        // Standard cameras never see vendor D1xx codes.
        assert!(!transport.probed.contains(&0xd100));
        assert!(!transport.probed.contains(&0xd061));
        assert!(transport.probed.contains(&0x500d));
    }

    #[test]
    fn autofocus_action_requires_nikon_brand_and_af_operation() {
        let with_af = DeviceProfile {
            brand: Brand::Nikon,
            operations: vec![0x90c1],
        };
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[0x90c1]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        let settings = camera_settings(&mut transport, &with_af).unwrap();
        assert!(settings.iter().any(|s| s.id == "autofocus"));

        // Nikon without the advertised AF operation → no action.
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        let settings = camera_settings(
            &mut transport,
            &DeviceProfile {
                brand: Brand::Nikon,
                operations: vec![],
            },
        )
        .unwrap();
        assert!(!settings.iter().any(|s| s.id == "autofocus"));

        // Same numeric operation on a non-Nikon camera → still no action.
        let mut transport = MockTransport {
            info: device_info("Generic Vendor", &[0x90c1]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        let settings = camera_settings(
            &mut transport,
            &DeviceProfile {
                brand: Brand::Standard,
                operations: vec![0x90c1],
            },
        )
        .unwrap();
        assert!(!settings.iter().any(|s| s.id == "autofocus"));
    }

    #[test]
    fn sony_cameras_route_through_the_vendor_protocol_not_std_descriptors() {
        // Sony 参数走厂商协议（遥控握手 + 批量描述），本 mock 只实现了
        // 标准探测路径，握手第一步即被拒——恰好证明没有误入标准路径，
        // 也没有对 Sony 发出 GetDevicePropDesc。
        let mut transport = MockTransport {
            info: device_info("Sony Corp", &[0x9201, 0x9205]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        let error = camera_settings(
            &mut transport,
            &DeviceProfile {
                brand: Brand::Sony,
                operations: vec![0x9201, 0x9205],
            },
        )
        .unwrap_err();
        assert!(error.contains("unexpected read operation"), "unexpected: {error}");
        assert!(
            transport.probed.is_empty(),
            "Sony 路径不得探测标准属性描述"
        );
    }

    #[test]
    fn device_profile_detects_brand_from_manufacturer() {
        let mut transport = MockTransport {
            info: device_info("NIKON CORPORATION", &[0x90c1]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        assert_eq!(device_profile(&mut transport).unwrap().brand, Brand::Nikon);

        let mut transport = MockTransport {
            info: device_info("SONY", &[]),
            outcomes: BTreeMap::new(),
            probed: Vec::new(),
            commands: Vec::new(),
        };
        assert_eq!(device_profile(&mut transport).unwrap().brand, Brand::Sony);
    }
}
