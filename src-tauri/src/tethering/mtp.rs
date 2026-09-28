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
/// - 0x90C0：1 参数 0x1（libgphoto2 同款「拍摄至卡」取值）；
/// - 0x100E：StorageID=0（任意存储）+ ObjectFormatCode=0（任意格式）。
pub fn capture_command(caps: Capabilities) -> Result<CaptureCommand, super::backend::TetherError> {
    if caps.vendor_capture_nikon() {
        Ok(CaptureCommand {
            code: PTP_OP_NIKON_CAPTURE,
            params: vec![0x1],
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
/// OperationsSupported · AUINT16 EventsSupported · …`
/// 容器格式：AUINT16 = u32 元素数 + N×u16；STR = u8 字符数(含 NUL，0=空)
/// + N×u16。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfoDataset {
    pub standard_version: u16,
    pub vendor_extension_id: u32,
    pub vendor_extension_version: u16,
    pub operations_supported: Vec<u16>,
    pub events_supported: Vec<u16>,
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
    Ok(DeviceInfoDataset {
        standard_version,
        vendor_extension_id,
        vendor_extension_version,
        operations_supported,
        events_supported,
    })
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
    if operations.contains(&PTP_OP_NIKON_CAPTURE) {
        caps = caps.union(Capabilities::VENDOR_CAPTURE_NIKON);
    }
    if wpd_object_added_event || events.contains(&PTP_EVENT_OBJECT_ADDED) {
        caps = caps.union(Capabilities::OBJECT_ADDED_EVENTS);
    }
    caps
}
