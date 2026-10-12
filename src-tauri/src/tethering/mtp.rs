//! PTP/MTP 协议纯函数（跨平台、无 COM；单测见 tests/tethering_test.rs）。
//!
//! - 操作码/事件码/响应码常量（数值为 ISO 15740 / MTP 公开协议事实，
//!   与 libmtp ptp.h、评估文档一致）；
//! - GetDeviceInfo 数据集二进制解析（小端）；
//! - 探测结果 → 能力位推导（OperationsSupported + EventsSupported +
//!   WPD OBJECT_ADDED 事件交叉验证）；
//! - 拍摄操作码选择（透传参数组装的纯函数面）。

use super::backend::Capabilities;

// ---------------------------------------------------------------------------
// 常量
// ---------------------------------------------------------------------------

/// PTP GetDeviceInfo（无会话也可用的少数操作之一；WPD 透传经
/// WITH_DATA_TO_READ 数据相位取回数据集）。
pub const PTP_OP_GET_DEVICE_INFO: u16 = 0x1001;
/// PTP OpenSession（WPD 驱动自行管理 PTP 会话；支持情况仅记录诊断）。
pub const PTP_OP_OPEN_SESSION: u16 = 0x1002;
/// PTP 标准 InitiateCapture（机型支持面不一，调用前查 OperationsSupported）。
pub const PTP_OP_INITIATE_CAPTURE: u16 = 0x100E;
/// Nikon 厂商扩展拍摄码（libmtp/libgphoto2 公开；1 参数、无数据相位）。
pub const PTP_OP_NIKON_CAPTURE: u16 = 0x90C0;
/// Nikon InitiateCaptureRecInMedia: mode + target (0 = card).
pub const PTP_OP_NIKON_CAPTURE_MEDIA: u16 = 0x9207;
/// PTP ObjectAdded 事件码（EventsSupported 交叉验证用）。
pub const PTP_EVENT_OBJECT_ADDED: u16 = 0x4002;
/// PTP 响应 OK。
pub const PTP_RESP_OK: u32 = 0x2001;

/// 拍摄命令（操作码 + 32 位参数表——WPD 透传的完整载荷面）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureCommand {
    pub code: u16,
    /// MTP 参数均为 u32（PTP 事务参数宽度）。
    pub params: Vec<u32>,
}

/// 按设备能力选择拍摄命令（透传参数组装纯函数）。
///
/// 优先级：Nikon 0x90C0（Nikon 对标准 0x100E 报 Parameter Not Supported，
/// 评估文档 §1.2 / gphoto2 #635）> 标准 0x100E。两者皆无 →
/// [`super::backend::TetherError::CaptureNotSupported`]。
///
/// 参数取值（真机验证项 N3，见 mod.rs 清单）：
/// - 0x90C0：1 参数 0xFFFFFFFF（libgphoto2 的普通拍摄参数，目标为 SDRAM）；
/// - 0x100E：StorageID=0（任意存储）+ ObjectFormatCode=0（任意格式）。
pub fn capture_command(caps: Capabilities) -> Result<CaptureCommand, super::backend::TetherError> {
    if caps.vendor_capture_nikon() {
        Ok(CaptureCommand {
            code: PTP_OP_NIKON_CAPTURE,
            params: vec![u32::MAX],
        })
    } else if caps.standard_capture() {
        Ok(CaptureCommand {
            code: PTP_OP_INITIATE_CAPTURE,
            params: vec![0x0, 0x0],
        })
    } else {
        Err(super::backend::TetherError::CaptureNotSupported)
    }
}

/// GetDeviceInfo 透传命令（数据相位读取由 wpd_backend COM 层完成）。
pub fn get_device_info_command() -> CaptureCommand {
    CaptureCommand {
        code: PTP_OP_GET_DEVICE_INFO,
        params: Vec::new(),
    }
}

// ---------------------------------------------------------------------------
// GetDeviceInfo 数据集解析
// ---------------------------------------------------------------------------

/// GetDeviceInfo 数据集的前半部（本层需要的字段；后续数组/字符串不解析）。
///
/// 布局（ISO 15740 DeviceInfo DataSet，小端）：
/// `u16 StandardVersion · u32 VendorExtensionID · u16 VendorExtensionVersion
/// · STR VendorExtensionDesc · u16 FunctionalMode · AUINT16
/// OperationsSupported · AUINT16 EventsSupported · AUINT16
/// DevicePropsSupported · AUINT16 CaptureFormats · AUINT16 ImageFormats
/// · STR Manufacturer · STR Model · …`
/// 容器格式：AUINT16 = u32 元素数 + N×u16；STR = u8 字符数(含 NUL，0=空)
/// + N×u16。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfoDataset {
    pub standard_version: u16,
    pub vendor_extension_id: u32,
    pub vendor_extension_version: u16,
    pub operations_supported: Vec<u16>,
    pub events_supported: Vec<u16>,
    /// 厂商名/机型（品牌判定输入；数据集尾部缺失时为空串）。
    pub manufacturer: String,
    pub model: String,
}

/// 解析失败（数据集截断/畸形）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("MTP DeviceInfo 解析失败: {0}")]
pub struct ParseDeviceInfoError(pub String);

/// 解析 GetDeviceInfo 数据集（纯函数）。
///
/// 只读取到 EventsSupported 为止（能力推导所需的全部字段）；尾部
/// （DevicePropsSupported 及以后）不校验——不同标准版本（1.0/1.1）尾部
/// 布局有差异，前半部两种版本一致。
pub fn parse_device_info(data: &[u8]) -> Result<DeviceInfoDataset, ParseDeviceInfoError> {
    let mut cur = Cursor { data, pos: 0 };
    let standard_version = cur.u16("StandardVersion")?;
    let vendor_extension_id = cur.u32("VendorExtensionID")?;
    let vendor_extension_version = cur.u16("VendorExtensionVersion")?;
    cur.ptp_string("VendorExtensionDesc")?;
    let _functional_mode = cur.u16("FunctionalMode")?;
    let operations_supported = cur.au16("OperationsSupported")?;
    let events_supported = cur.au16("EventsSupported")?;
    // 身份尾部尽力解析：能力字段已完整，尾部缺失/截断不构成解析失败
    // （品牌判定对空串回退 Standard）。
    let (manufacturer, model) = parse_identity_tail(&mut cur);
    Ok(DeviceInfoDataset {
        standard_version,
        vendor_extension_id,
        vendor_extension_version,
        operations_supported,
        events_supported,
        manufacturer,
        model,
    })
}

/// 解析 Manufacturer/Model：先跳过三个 AUINT16 数组再读两个字符串。
/// 任一字段缺失/截断 → 两串皆空（容忍最小数据集与畸形尾部）。
fn parse_identity_tail(cur: &mut Cursor<'_>) -> (String, String) {
    for field in ["DevicePropsSupported", "CaptureFormats", "ImageFormats"] {
        if cur.au16(field).is_err() {
            return (String::new(), String::new());
        }
    }
    let manufacturer = cur.ptp_string_value("Manufacturer").unwrap_or_default();
    let model = cur.ptp_string_value("Model").unwrap_or_default();
    (manufacturer, model)
}

/// 游标读取器（小端；越界报「截断于字段 X」）。
struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, len: usize, field: &str) -> Result<&'a [u8], ParseDeviceInfoError> {
        let Some(slice) = self.data.get(self.pos..self.pos + len) else {
            return Err(ParseDeviceInfoError(format!(
                "数据集截断于字段 {field}（偏移 {}，还需 {len} 字节，剩余 {}）",
                self.pos,
                self.data.len().saturating_sub(self.pos)
            )));
        };
        self.pos += len;
        Ok(slice)
    }

    fn u16(&mut self, field: &str) -> Result<u16, ParseDeviceInfoError> {
        let s = self.take(2, field)?;
        Ok(u16::from_le_bytes([s[0], s[1]]))
    }

    fn u32(&mut self, field: &str) -> Result<u32, ParseDeviceInfoError> {
        let s = self.take(4, field)?;
        Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    /// PTP 字符串：u8 字符数（含 NUL 终止；0=空串）+ N×u16。内容仅跳过。
    fn ptp_string(&mut self, field: &str) -> Result<(), ParseDeviceInfoError> {
        let len = self.take(1, field)?[0] as usize;
        if len > 0 {
            self.take(len * 2, field)?;
        }
        Ok(())
    }

    /// PTP 字符串内容（UTF-16 解码，剔除 NUL 终止；解码失败 → 空串）。
    fn ptp_string_value(&mut self, field: &str) -> Result<String, ParseDeviceInfoError> {
        let len = self.take(1, field)?[0] as usize;
        let units: Vec<u16> = self
            .take(len * 2, field)?
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .filter(|unit| *unit != 0)
            .collect();
        Ok(String::from_utf16(&units).unwrap_or_default())
    }

    /// AUINT16 容器：u32 元素数 + N×u16。空列表（N=0）合法。
    fn au16(&mut self, field: &str) -> Result<Vec<u16>, ParseDeviceInfoError> {
        let count = self.u32(&format!("{field}.count"))? as usize;
        // 防御异常设备的天文数字计数：可用字节不足即截断错误。
        let mut out = Vec::with_capacity(count.min(1024));
        for _ in 0..count {
            out.push(self.u16(field)?);
        }
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// 能力推导
// ---------------------------------------------------------------------------

/// 探测结果 → 设备能力位。
///
/// - `operations`：GetDeviceInfo 的 OperationsSupported；
/// - `events`：GetDeviceInfo 的 EventsSupported（PTP ObjectAdded=0x4002）；
/// - `wpd_object_added_event`：WPD Capabilities::GetSupportedEvents 含
///   WPD_EVENT_OBJECT_ADDED（wpd_probe 同款交叉验证）。
///
/// 事件位取两者之并（任一在列即认为可用；映射机制为评估文档推断项，
/// 真机清单 N3/S3 验证）。file_transfer 恒真（WPD 文件栈既有）；
/// live_view v1 恒假。
pub fn capabilities_from_probe(
    operations: &[u16],
    events: &[u16],
    wpd_object_added_event: bool,
) -> Capabilities {
    let mut caps = Capabilities::FILE_TRANSFER;
    if operations.contains(&PTP_OP_INITIATE_CAPTURE) {
        caps = caps.union(Capabilities::STANDARD_CAPTURE);
    }
    if operations.contains(&PTP_OP_NIKON_CAPTURE)
        || operations.contains(&PTP_OP_NIKON_CAPTURE_MEDIA)
    {
        caps = caps.union(Capabilities::VENDOR_CAPTURE_NIKON);
    }
    if wpd_object_added_event || events.contains(&PTP_EVENT_OBJECT_ADDED) {
        caps = caps.union(Capabilities::OBJECT_ADDED_EVENTS);
    }
    caps
}

/// WPD needs a card-backed object. Prefer Nikon's media capture when advertised,
/// then standard card capture; use legacy SDRAM capture only as a last resort.
/// Parameters match libgphoto2 camlibs/ptp2/ptp.h (ptp_nikon_capture2).
pub fn capture_command_for_operations(
    operations: &[u16],
) -> Result<CaptureCommand, super::backend::TetherError> {
    if operations.contains(&PTP_OP_NIKON_CAPTURE_MEDIA) {
        Ok(CaptureCommand {
            code: PTP_OP_NIKON_CAPTURE_MEDIA,
            params: vec![u32::MAX, 0],
        })
    } else if operations.contains(&PTP_OP_INITIATE_CAPTURE) {
        capture_command(Capabilities::STANDARD_CAPTURE)
    } else if operations.contains(&PTP_OP_NIKON_CAPTURE) {
        capture_command(Capabilities::VENDOR_CAPTURE_NIKON)
    } else {
        Err(super::backend::TetherError::CaptureNotSupported)
    }
}

#[cfg(test)]
mod capture_profile_tests {
    use super::*;

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

    /// 标准容器 + 完整身份尾部。
    fn dataset_with_identity(manufacturer: &str, model: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&6u32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&ptp_string("MTP"));
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&au16(&[0x1001, 0x90c1]));
        data.extend_from_slice(&au16(&[0x4002]));
        data.extend_from_slice(&au16(&[0x5007, 0x500f])); // DevicePropsSupported
        data.extend_from_slice(&au16(&[])); // CaptureFormats
        data.extend_from_slice(&au16(&[0x3801])); // ImageFormats
        data.extend_from_slice(&ptp_string(manufacturer));
        data.extend_from_slice(&ptp_string(model));
        data
    }

    #[test]
    fn identity_tail_carries_manufacturer_and_model() {
        let parsed = parse_device_info(&dataset_with_identity("NIKON CORPORATION", "Z 5II"))
            .expect("完整数据集应解析成功");
        assert_eq!(parsed.manufacturer, "NIKON CORPORATION");
        assert_eq!(parsed.model, "Z 5II");
        assert_eq!(parsed.operations_supported, vec![0x1001, 0x90c1]);
    }

    #[test]
    fn minimal_dataset_without_tail_parses_with_empty_identity() {
        let mut data = Vec::new();
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&6u32.to_le_bytes());
        data.extend_from_slice(&100u16.to_le_bytes());
        data.extend_from_slice(&ptp_string(""));
        data.extend_from_slice(&0u16.to_le_bytes());
        data.extend_from_slice(&au16(&[0x1001]));
        data.extend_from_slice(&au16(&[]));
        let parsed = parse_device_info(&data).expect("尾部缺失应容忍");
        assert_eq!(parsed.manufacturer, "");
        assert_eq!(parsed.model, "");

        // 尾部在 Model 字符串中间截断同样回退，不报错：Manufacturer 完整
        // 读到，Model 退为空串。
        let full = dataset_with_identity("SONY", "ILCE-7M4");
        let cut = &full[..full.len() - 4];
        let parsed = parse_device_info(cut).expect("截断尾部应容忍");
        assert_eq!(parsed.manufacturer, "SONY");
        assert_eq!(parsed.model, "");
    }

    #[test]
    fn modern_nikon_targets_card_with_valid_parameters() {
        let command = capture_command_for_operations(&[0x90c0, 0x100e, 0x9207]).unwrap();
        assert_eq!(
            command,
            CaptureCommand {
                code: 0x9207,
                params: vec![0xffffffff, 0]
            }
        );
    }
    #[test]
    fn standard_capture_precedes_legacy_sdram() {
        assert_eq!(
            capture_command_for_operations(&[0x90c0, 0x100e])
                .unwrap()
                .code,
            0x100e
        );
        assert_eq!(
            capture_command_for_operations(&[0x90c0]).unwrap().params,
            vec![0xffffffff]
        );
        assert!(capture_command_for_operations(&[]).is_err());
    }
}
