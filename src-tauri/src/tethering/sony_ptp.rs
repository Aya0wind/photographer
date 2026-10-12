//! Sony 原生 PTP 遥控协议层（docs/plans/2026-10-11-native-tethering.md §C）。
//!
//! 纯协议层：只依赖 [`super::ptp_control::PtpTransport`] 传输 seam 与标准类型，
//! 不含任何 OS 代码（Windows WPD / macOS ImageCaptureCore 均可承载）。
//!
//! # 厂商判定（铁律）
//!
//! Sony 与 Nikon 复用同一批 0x92xx 数值表示完全不同的功能（Nikon 0x9201/02/03
//! = 取景开始/结束/取帧；Sony 0x9201/02/03 = SDIO 连接/扩展信息/属性描述）。
//! 绝不能只凭操作码发厂商指令——必须先用 [`is_sony`]（Manufacturer 含
//! "SONY" 且声明 Sony 扩展操作码）确认厂商。本模块也绝不复用 Nikon 取景
//! 状态机；Sony 取景走「魔术对象句柄 0xFFFFC002」路径。
//!
//! # 已核实来源（libgphoto2 master，LGPL，仅按公开协议事实对照实现）
//!
//! - `camlibs/ptp2/ptp.h`：`PTP_VENDOR_SONY=0x00000011`、`PTP_OC_SONY_SDIO_*`
//!   操作码、`PTP_DPC_SONY_*` 属性码、Sony 事件码。
//! - `camlibs/ptp2/ptp.c`：`ptp_sony_sdioconnect`（0x9201，3 参数 + 数据读
//!   相位）、`ptp_sony_get_vendorpropcodes`（0x9202，参数 (0x12c,1)）、
//!   `ptp_sony_getdevicepropdesc`（0x9203）、
//!   `_ptp_sony_getalldevicepropdesc`（0x9209 批量布局）、
//!   `ptp_sony_setdevicecontrolvaluea/b`（0x9205/0x9207）、
//!   `ptp_generic_setdevicepropvalue`（Sony 写入优先 0x9205，退 0x1016）。
//! - `camlibs/ptp2/library.c`：`fixup_cached_deviceinfo` Sony 段（SDIO 连接
//!   阶段 1/2 → 0x9202 查询(≤20 次) → 阶段 3 → PriorityMode=1）；
//!   `camera_sony_capture`（0x9207 半按/全按/释放）；`camera_preview` Sony 段
//!   （取景 = 读对象 0xFFFFC002，JPEG 前 4 字节小端偏移提示）。
//! - `camlibs/ptp2/ptp-pack.c`：`ptp_unpack_Sony_DPD`（Sony 属性描述二进制
//!   布局与 mode2/mode3 可写位规则）。
//! - `camlibs/ptp2/config.c`：快门 0xD20D u32 有理数 `(分子<<16)|分母`、
//!   ISO 0xD21E/0xD226 u32（低 24 位 ISO，0xFFFFFF=Auto，高 8 位 MFNR）、
//!   光圈 0x5007 u16×100（mode3 写入按 u32 4 字节打包）、白平衡/对焦模式
//!   Sony 扩展枚举表。
//!
//! 来源 URL：
//! - <https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/ptp.h>
//! - <https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/ptp.c>
//! - <https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/library.c>
//! - <https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/config.c>
//! - <https://github.com/gphoto/libgphoto2/blob/master/camlibs/ptp2/ptp-pack.c>
//!
//! # 真机用例
//!
//! 本模块无法直接构造 OS 传输（WPD transport 在 platform 层为模块私有），
//! 真机验证需等 vendor_dispatch 接线后进行：接机后用
//! `init_remote_detailed` + `settings`（只读，不触发快门）先行验收；
//! `capture` 会真实触发快门。

// 集成层 vendor_dispatch::SonyProfile 已接线（init_remote/settings/
// set_setting/capture/live_view_* 经厂商 profile 转发到本模块）。保留
// `#[allow(dead_code)]` 直至真机验收——模拟 transport 测试全绿不能证明
// 真机行为，届时按真机结果清理未使用项。
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::thread::sleep;
use std::time::Duration;

use super::backend::{CameraSetting, CameraSettingOption, SettingKind};
use super::mtp::PTP_OP_INITIATE_CAPTURE;
use super::ptp_control::{
    preview_jpeg, require_ok, value_label, Form, Property, PtpTransport, SET_PROP_VALUE, Value,
};

// ---------------------------------------------------------------------------
// 操作码 / 属性码 / 响应码（来源见模块文档；数值逐条对照上游源码）
// ---------------------------------------------------------------------------

/// Sony SDIO 连接阶段切换（Nikon 同数值 0x9201 是取景开始——厂商判定先行）。
pub const SDIO_CONNECT: u16 = 0x9201;
/// Sony 扩展能力查询：返回协议版本 + 操作/事件/属性码表。
pub const SDIO_GET_EXT_DEVICE_INFO: u16 = 0x9202;
/// Sony 单属性描述（Sony 专有布局，非标准 0x1014 数据集）。
pub const SONY_GET_DEVICE_PROP_DESC: u16 = 0x9203;
/// Sony 属性写入（`ptp_generic_setdevicepropvalue` 对 Sony 的首选路径）。
pub const SDIO_SET_EXT_DEVICE_PROP_VALUE: u16 = 0x9205;
/// Sony 控制指令（快门半按/全按等动作型写入）。
pub const SDIO_CONTROL_DEVICE: u16 = 0x9207;
/// Sony 批量属性描述（布局：u32 条数 + u32 保留 0 + 连续 Sony DPD）。
pub const SDIO_GET_ALL_EXT_DEVICE_PROP_INFO: u16 = 0x9209;
// 0x9210 SDIO_OpenSession：上游 ptp.h 有定义但 libgphoto2 从不调用（仅出现在
// 调试名表）。TODO-verify(Sony Camera Remote Command 文档)：语义未核对，禁用。

/// 协议版本（`SONY_PROTOCOL_3_00` = 2020 及以后机型的直写协议）。
const PROTOCOL_3_00: u16 = 0x012c;
/// 旧协议（2020 以前；快门/ISO/光圈需步进写入——本模块禁用该写入路径）。
const PROTOCOL_2_00: u16 = 0x00c8;

// 设备属性码（ptp.h PTP_DPC_SONY_* / 标准 DPC）。
const DPC_SONY_SHUTTER_SPEED: u16 = 0xd20d; // u32，(分子<<16)|分母
const DPC_SONY_ISO: u16 = 0xd21e; // u32，低 24 位 ISO
const DPC_SONY_ISO2: u16 = 0xd226; // u32，2020+ 机型优先
const DPC_F_NUMBER: u16 = 0x5007; // 标准，u16 ×100
const DPC_WHITE_BALANCE: u16 = 0x5005; // 标准，u16
const DPC_FOCUS_MODE: u16 = 0x500a; // 标准，u16
const DPC_SONY_PRIORITY_MODE: u16 = 0xd25a; // i8，1=application
const DPC_SONY_SHUTTER_HALF_RELEASE: u16 = 0xd2c1; // 控制，u16 2=按下 1=释放
const DPC_SONY_SHUTTER_RELEASE: u16 = 0xd2c2; // 控制，u16 2=按下 1=释放
const DPC_SONY_FOCUS_FOUND: u16 = 0xd213; // u8，2/3=已合焦

// 标准操作码（读取取景对象用）。
const PTP_OP_GET_OBJECT_INFO: u16 = 0x1008;
const PTP_OP_GET_OBJECT: u16 = 0x1009;

// 响应码。
const RESP_OK: u32 = 0x2001;
const RESP_OPERATION_NOT_SUPPORTED: u32 = 0x2005;
const RESP_INVALID_OBJECT_HANDLE: u32 = 0x2009;
const RESP_ACCESS_DENIED: u32 = 0x200f;

/// Sony Alpha 取景帧的魔术对象句柄（library.c camera_preview Sony 段）。
const LIVE_VIEW_OBJECT: u32 = 0xffff_c002;

/// 上游取景探测的节奏（ZV-1 上 InvalidObjectHandle 状态可持续约 1 秒）。
const LIVE_VIEW_START_ATTEMPTS: usize = 20;
const LIVE_VIEW_START_DELAY: Duration = Duration::from_millis(50);
const LIVE_VIEW_FRAME_ATTEMPTS: usize = 20;
const LIVE_VIEW_FRAME_DELAY: Duration = Duration::from_millis(20);
/// 快门等待合焦的上限（camera_sony_capture 按 1 秒设计）。
const FOCUS_WAIT_ATTEMPTS: usize = 20;
const FOCUS_WAIT_DELAY: Duration = Duration::from_millis(50);

/// 判定为 Sony 扩展协议的操作码集合（ptp.h PTP_OC_SONY_*；含 QX 系列）。
const SONY_SDIO_OPERATIONS: &[u16] = &[
    0x9201, 0x9202, 0x9203, 0x9204, 0x9205, 0x9206, 0x9207, 0x9209, 0x9210, 0x9211, 0x9212,
    0x9215, 0x9223, 0x922f, 0x96f6, 0x96f8, 0x96fa, 0x96fd, 0x96fe,
];

/// 项目范围五参数的 Sony 属性映射（ISO 两码按机型二选一）。
const SCOPE_SPECS: &[(u16, &str, &str)] = &[
    (DPC_SONY_SHUTTER_SPEED, "shutterspeed", "Shutter speed"),
    (DPC_F_NUMBER, "f-number", "Aperture"),
    (DPC_SONY_ISO2, "iso", "ISO"),
    (DPC_SONY_ISO, "iso", "ISO"),
    (DPC_WHITE_BALANCE, "whitebalance", "White balance"),
    (DPC_FOCUS_MODE, "focusmode", "Focus mode"),
];

// ---------------------------------------------------------------------------
// 厂商判定
// ---------------------------------------------------------------------------

/// 厂商判定（铁律 1）：Manufacturer 含 "SONY"（大小写不敏感）**且**
/// OperationsSupported 声明任一 Sony 扩展操作码。两个条件缺一不可：
/// Nikon 同样声明 0x9201/02/03/05/07，只看操作码会把 Nikon 误判成 Sony；
/// 只看名字则会对未进遥控协议的 Sony 存储设备误发扩展码。
pub fn is_sony(manufacturer: &str, operations: &[u16]) -> bool {
    manufacturer.to_ascii_uppercase().contains("SONY")
        && operations
            .iter()
            .any(|code| SONY_SDIO_OPERATIONS.contains(code))
}

// ---------------------------------------------------------------------------
// 会话初始化
// ---------------------------------------------------------------------------

/// Sony 遥控协议版本（0x9202 查询结果；决定属性写入方式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// 2020 以前机型：快门/ISO/光圈需步进写入（本模块禁用，见 TODO-verify）。
    V2,
    /// 2020 及以后机型：0x9205 直写。
    V3,
}

/// `init_remote_detailed` 的结果：协议版本 + 0x9202 揭示的扩展码表。
///
/// Sony 把大部分扩展操作/属性藏在 0x9202 之后，GetDeviceInfo 里看不到；
/// 能力判断应把 `ext_codes` 并入 OperationsSupported 再调
/// [`capabilities_supported`]。
#[derive(Debug, Clone)]
pub struct RemoteSession {
    pub protocol: Protocol,
    pub ext_codes: Vec<u16>,
}

/// 初始化 Sony 遥控会话（集成合同签名）。
///
/// 序列逐条对照 library.c `fixup_cached_deviceinfo` Sony 段：
/// 0x9201 阶段 1 → 阶段 2 → 0x9202 扩展查询（≤20 次重试空结果）→
/// 0x9201 阶段 3 → 0x9205 写 PriorityMode(0xD25A)=1（仅记录错误，不中断，
/// 与上游 `LOG_ON_PTP_E` 一致）。上游每次会话建立都重发整套握手，因此重复
/// 调用即「已在遥控态」的幂等路径。需要协议版本/扩展码表时用
/// [`init_remote_detailed`]。
pub fn init_remote(transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
    init_remote_detailed(transport).map(|_| ())
}

/// [`init_remote`] 的完整版本：返回协议版本与扩展码表。
pub fn init_remote_detailed(transport: &mut (impl PtpTransport + ?Sized)) -> Result<RemoteSession, String> {
    for phase in [1u32, 2] {
        let (response, _) = transport.read(SDIO_CONNECT, &[phase, 0, 0], 256)?;
        require_ok(response, SDIO_CONNECT)?;
    }

    // 0x9202 请求协议 3.00：不支持的相机回其最高版本；空数据=无扩展码。
    let mut protocol = Protocol::V2;
    let mut ext_codes = Vec::new();
    for _ in 0..20 {
        let (response, data) =
            transport.read(SDIO_GET_EXT_DEVICE_INFO, &[PROTOCOL_3_00 as u32, 1], 64 * 1024)?;
        require_ok(response, SDIO_GET_EXT_DEVICE_INFO)?;
        if data.is_empty() {
            continue; // 上游对空结果整体重试（≤20 次）
        }
        if let Some((version, codes)) = parse_ext_device_info(&data) {
            protocol = if version == PROTOCOL_3_00 {
                Protocol::V3
            } else {
                Protocol::V2
            };
            if codes.is_empty() {
                continue;
            }
            ext_codes = codes;
            break;
        }
        // 布局异常按空结果处理并重试（同上游解包失败归零）。
    }

    let (response, _) = transport.read(SDIO_CONNECT, &[3, 0, 0], 256)?;
    require_ok(response, SDIO_CONNECT)?;

    // PriorityMode=1（application）。上游仅记录错误；初始化不因此失败。
    let _ = transport.write(
        SDIO_SET_EXT_DEVICE_PROP_VALUE,
        &[DPC_SONY_PRIORITY_MODE as u32],
        &[1],
    );

    Ok(RemoteSession {
        protocol,
        ext_codes,
    })
}

/// 解析 0x9202 数据集：u16 版本 + AUINT16 码表 ×2（上游把两个数组拼接为
/// 一个列表；元素分类由 `code & 0x7000` 决定：0x1000 操作/0x4000 事件/
/// 0x5000 属性）。
fn parse_ext_device_info(data: &[u8]) -> Option<(u16, Vec<u16>)> {
    let mut offset = 0usize;
    let version = take_u16(data, &mut offset)?;
    let first = take_au16(data, &mut offset)?;
    let second = take_au16(data, &mut offset)?;
    let mut codes = first;
    codes.extend(second);
    Some((version, codes))
}

fn take_u16(data: &[u8], offset: &mut usize) -> Option<u16> {
    let bytes = data.get(*offset..*offset + 2)?;
    *offset += 2;
    Some(u16::from_le_bytes([bytes[0], bytes[1]]))
}

/// AUINT16 容器：u32 元素数 + N×u16（ptp-pack.c `ptp_unpack_uint16_t_array`）。
fn take_au16(data: &[u8], offset: &mut usize) -> Option<Vec<u16>> {
    let count_bytes = data.get(*offset..*offset + 4)?;
    *offset += 4;
    let count = u32::from_le_bytes(count_bytes.try_into().unwrap()) as usize;
    let mut values = Vec::with_capacity(count.min(1024));
    for _ in 0..count {
        values.push(take_u16(data, offset)?);
    }
    Some(values)
}

// ---------------------------------------------------------------------------
// Sony 属性描述解析（ptp-pack.c ptp_unpack_Sony_DPD 的布局）
// ---------------------------------------------------------------------------

/// 小端读取器（与 ptp_control::Reader 同构；该类型私有，无法复用）。
struct Reader<'a> {
    data: &'a [u8],
    offset: usize,
}
impl<'a> Reader<'a> {
    fn take(&mut self, size: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(size).ok_or("PTP 数据长度溢出")?;
        let bytes = self
            .data
            .get(self.offset..end)
            .ok_or("PTP 参数数据不完整")?;
        self.offset = end;
        Ok(bytes)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.offset)
    }
    /// 标准 PTP DTC 值（编码与 ISO 15740 / ptp_control::Reader::value 一致）。
    fn value(&mut self, kind: u16) -> Result<Value, String> {
        Ok(match kind {
            1 => Value::Signed(i8::from_le_bytes(self.take(1)?.try_into().unwrap()) as i64),
            2 => Value::Unsigned(self.u8()? as u64),
            3 => Value::Signed(i16::from_le_bytes(self.take(2)?.try_into().unwrap()) as i64),
            4 => Value::Unsigned(self.u16()? as u64),
            5 => Value::Signed(i32::from_le_bytes(self.take(4)?.try_into().unwrap()) as i64),
            6 => Value::Unsigned(u32::from_le_bytes(self.take(4)?.try_into().unwrap()) as u64),
            7 => Value::Signed(i64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            8 => Value::Unsigned(u64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            0xffff => {
                let count = self.u8()? as usize;
                let bytes = self.take(count * 2)?;
                let mut units: Vec<u16> = bytes
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                if units.last() == Some(&0) {
                    units.pop();
                }
                Value::Text(String::from_utf16(&units).map_err(|_| "PTP 字符串无效")?)
            }
            _ => return Err(format!("不支持的 PTP 参数类型 0x{kind:04X}")),
        })
    }
}

/// Sony 可写位推导（ptp-pack.c mode2/mode3 两套规则）。
fn sony_writable(protocol: Protocol, getset: u8, isenabled: u8, code: u16) -> bool {
    match protocol {
        // 新模式：isenabled==1 时 GetSet 位本身正确；0=灰化/2=仅显示 → 只读。
        Protocol::V3 => isenabled == 1 && getset == 1,
        // 旧模式：0x80 位 = 控制值（按钮/步进/锁），可写；isenabled==1 可写；
        // 「仅显示」里 FocusMode 例外可写（sony_mode2_settable_props）。
        Protocol::V2 => getset & 0x80 != 0 || isenabled == 1 || (isenabled == 2 && code == DPC_FOCUS_MODE),
    }
}

/// 解析单条 Sony 属性描述，返回（属性, 消费字节数）。
///
/// 布局（ptp-pack.c）：`u16 属性码 · u16 数据类型 · u8 GetSet · u8 IsEnabled
/// · 默认值 · 当前值 · [u8 FormFlag; 表单]`。字符串属性可能缺当前值/表单；
/// 数据在当前值之后结束（无表单字节）是合法的（上游 `return 1`）。
fn parse_sony_prop_desc(data: &[u8], protocol: Protocol) -> Result<(Property, usize), String> {
    let mut reader = Reader { data, offset: 0 };
    let code = reader.u16()?;
    let data_type = reader.u16()?;
    let getset = reader.u8()?;
    let isenabled = reader.u8()?;
    let _default = reader.value(data_type)?;
    let current = if data_type == 0xffff && reader.remaining() == 0 {
        // 上游：字符串默认值吃完全部数据时当前值缺省（置零）。
        Value::Text(String::new())
    } else {
        reader.value(data_type)?
    };
    let form = if reader.remaining() >= 1 {
        match reader.u8()? {
            1 => Form::Range(
                reader.value(data_type)?,
                reader.value(data_type)?,
                reader.value(data_type)?,
            ),
            2 => {
                let count = reader.u16()? as usize;
                if count > 4096 {
                    return Err("PTP 参数选项过多".into());
                }
                let mut values = Vec::with_capacity(count);
                for _ in 0..count {
                    values.push(reader.value(data_type)?);
                }
                Form::Enum(values)
            }
            _ => Form::None,
        }
    } else {
        Form::None
    };
    Ok((
        Property {
            code,
            data_type,
            writable: sony_writable(protocol, getset, isenabled, code),
            current,
            form,
        },
        reader.offset,
    ))
}

// ---------------------------------------------------------------------------
// 属性读取 / 写入
// ---------------------------------------------------------------------------

/// 读取项目范围五参数（快门/光圈/ISO/白平衡/对焦模式）。
///
/// 优先 Sony 批量描述 0x9209（已核实布局）；失败则逐属性 0x9203。
/// V2 协议下快门/ISO/光圈报告为不可写（步进写入路径未实现，见 TODO-verify）。
pub fn settings(
    transport: &mut (impl PtpTransport + ?Sized),
    protocol: Protocol,
) -> Result<Vec<CameraSetting>, String> {
    let mut props = BTreeMap::new();
    match transport.read(SDIO_GET_ALL_EXT_DEVICE_PROP_INFO, &[], 1024 * 1024) {
        Ok((response, data)) if response == RESP_OK && data.len() > 8 => {
            // 头部：u32 条数 + u32 保留 0；之后为连续 DPD，解析失败即止
            // （与上游 while(size>0) + break 一致）。
            let mut rest = &data[8..];
            while !rest.is_empty() {
                match parse_sony_prop_desc(rest, protocol) {
                    Ok((property, consumed)) => {
                        if consumed == 0 {
                            break; // 防御：零消费会死循环
                        }
                        let code = property.code;
                        props.insert(code, property);
                        rest = &rest[consumed..];
                    }
                    Err(_) => break,
                }
            }
        }
        _ => {
            for (code, _, _) in SCOPE_SPECS {
                if let Some(property) = fetch_property(transport, protocol, *code)? {
                    props.insert(*code, property);
                }
            }
        }
    }

    let mut result = Vec::new();
    for (code, id, label) in SCOPE_SPECS {
        // ISO 双码：新机型 0xD226 优先（config.c 表序），旧机型只有 0xD21E。
        if *code == DPC_SONY_ISO && props.contains_key(&DPC_SONY_ISO2) {
            continue;
        }
        if *code == DPC_SONY_ISO2 && !props.contains_key(&DPC_SONY_ISO2) {
            continue;
        }
        if let Some(mut property) = props.remove(code) {
            if protocol == Protocol::V2 && is_mode2_stepping_code(*code) {
                property.writable = false;
            }
            result.push(camera_setting(&property, id, label));
        }
    }
    Ok(result)
}

/// 逐属性 0x9203 读取；不支持（0x2005/0x2009）返回 None，其他错误上抛。
fn fetch_property(
    transport: &mut (impl PtpTransport + ?Sized),
    protocol: Protocol,
    code: u16,
) -> Result<Option<Property>, String> {
    match transport.read(SONY_GET_DEVICE_PROP_DESC, &[code as u32], 16 * 1024) {
        Ok((response, data)) if response == RESP_OK => {
            // TODO-verify(Sony 文档)：ptp.c 注释称 0x9203 数据开头为 0x00C8
            // 版本字，但 `ptp_unpack_Sony_DPD` 直接从属性码起解析——以上游
            // 实际代码为准；真机验收时核对首两字节。
            parse_sony_prop_desc(&data, protocol)
                .map(|(property, _)| Some(property))
                .map_err(|e| format!("Sony 属性 0x{code:04X} 描述无效：{e}"))
        }
        Ok((response, _)) if response == RESP_OPERATION_NOT_SUPPORTED => Ok(None),
        Ok((response, _)) if response == RESP_INVALID_OBJECT_HANDLE => Ok(None),
        Ok((response, _)) => {
            require_ok(response, SONY_GET_DEVICE_PROP_DESC)?;
            Ok(None)
        }
        Err(e) => Err(e),
    }
}

/// V2 协议下必须走「步进」写入的参数（上游 `_put_sony_value_*` 相对步进
/// 算法）：直写 0x9205 的效果未核对，按铁律禁用。
fn is_mode2_stepping_code(code: u16) -> bool {
    matches!(
        code,
        DPC_SONY_SHUTTER_SPEED | DPC_SONY_ISO | DPC_SONY_ISO2 | DPC_F_NUMBER
    )
}

/// id → 首选属性码（ISO 先按新码 0xD226 解析）。
fn primary_code(id: &str) -> Option<u16> {
    match id {
        "shutterspeed" => Some(DPC_SONY_SHUTTER_SPEED),
        "f-number" => Some(DPC_F_NUMBER),
        "iso" => Some(DPC_SONY_ISO2),
        "whitebalance" => Some(DPC_WHITE_BALANCE),
        "focusmode" => Some(DPC_FOCUS_MODE),
        _ => None,
    }
}

/// 写入一个项目范围参数（token 为 UI 提交的原始值，与
/// ptp_control::Property::encode 的约定一致）。
///
/// 写入路径对照 `ptp_generic_setdevicepropvalue`：Sony 首选 0x9205，相机
/// 回 OperationNotSupported 时退标准 0x1016。
pub fn set_setting(
    transport: &mut (impl PtpTransport + ?Sized),
    protocol: Protocol,
    id: &str,
    token: &str,
) -> Result<(), String> {
    let primary = primary_code(id).ok_or_else(|| format!("未知参数 {id}"))?;
    if protocol == Protocol::V2 && is_mode2_stepping_code(primary) {
        // TODO-verify(libgphoto2 _put_sony_value_u16/u32 步进算法)：
        // V2 直写未核对，禁用。
        return Err("Sony 旧协议(V2)的快门/ISO/光圈步进写入未核对，禁用".into());
    }
    let mut fetched = fetch_property(transport, protocol, primary)?;
    if fetched.is_none() && primary == DPC_SONY_ISO2 {
        fetched = fetch_property(transport, protocol, DPC_SONY_ISO)?;
    }
    let property = fetched.ok_or_else(|| format!("相机不支持参数 {id}"))?;
    if !property.writable {
        return Err("当前拍摄模式下此参数不可修改".into());
    }
    let payload = encode_set_payload(&property, token, protocol)?;
    let response = transport.write(SDIO_SET_EXT_DEVICE_PROP_VALUE, &[property.code as u32], &payload)?;
    if response == RESP_OPERATION_NOT_SUPPORTED {
        let fallback = transport.write(SET_PROP_VALUE, &[property.code as u32], &payload)?;
        require_ok(fallback, SET_PROP_VALUE)
    } else {
        require_ok(response, SDIO_SET_EXT_DEVICE_PROP_VALUE)
    }
}

/// 组装写入载荷。
///
/// 光圈 0x5007 在 V3 下按 u32 4 字节打包（config.c `_put_Sony_FNumber`
/// 传 `PTP_DTC_UINT32`——上游实证 A7R4/A7R5 可用的字节数）；其余按描述
/// 声明的类型走标准 [`Property::encode`]（枚举/范围校验复用）。
fn encode_set_payload(
    property: &Property,
    token: &str,
    protocol: Protocol,
) -> Result<Vec<u8>, String> {
    if property.code == DPC_F_NUMBER && protocol == Protocol::V3 {
        let value: u64 = token.parse().map_err(|_| "参数值无效")?;
        if let Form::Enum(values) = &property.form {
            if !values.contains(&Value::Unsigned(value)) {
                return Err("参数不在相机支持的选项中".into());
            }
        }
        if let Form::Range(min, max, step) = &property.form {
            let n = value as i128;
            let lo = min.token().parse::<i128>().map_err(|_| "参数范围无效")?;
            let hi = max.token().parse::<i128>().map_err(|_| "参数范围无效")?;
            let st = step.token().parse::<i128>().map_err(|_| "参数范围无效")?;
            if n < lo || n > hi || (st > 0 && (n - lo) % st != 0) {
                return Err("参数不在相机支持的范围中".into());
            }
        }
        if value > u16::MAX as u64 {
            return Err("参数超出数据类型范围".into());
        }
        return Ok((value as u32).to_le_bytes().to_vec());
    }
    property.encode(token)
}

/// 把 Sony 属性投影为 UI 设置项（标签用 Sony 专属取值表，来源 config.c）。
fn camera_setting(property: &Property, id: &str, label: &str) -> CameraSetting {
    let mut choices = match &property.form {
        Form::Enum(values) => values.clone(),
        _ => Vec::new(),
    };
    if !choices.contains(&property.current) {
        choices.push(property.current.clone());
    }
    CameraSetting {
        id: id.into(),
        label: label.into(),
        kind: SettingKind::Choice,
        current: property.current.token(),
        writable: property.writable,
        options: choices
            .iter()
            .map(|value| CameraSettingOption {
                value: value.token(),
                label: sony_value_label(property.code, value),
            })
            .collect(),
        min: None,
        max: None,
        step: None,
    }
}

/// Sony 参数取值标签（白平衡/对焦模式扩展枚举、快门有理数、ISO 掩码；
/// 数值含义逐条对照 config.c 的 PTP_VENDOR_SONY 表，不与 Nikon 混用）。
fn sony_value_label(code: u16, value: &Value) -> String {
    let Some(number) = value_number(value) else {
        return value.token();
    };
    match code {
        // 快门：u32 (分子<<16)|分母；0= Bulb（config.c：文档暗示 0 为 B 门）。
        DPC_SONY_SHUTTER_SPEED => {
            if number == 0 {
                return "Bulb".into();
            }
            let mut numerator = (number >> 16) as u64;
            let mut denominator = number & 0xffff;
            // Sony 高速快门报 300/10 这类形式，归一化成 30/1（config.c）。
            if denominator == 10 && numerator % 10 == 0 {
                numerator /= 10;
                denominator = 1;
            }
            let rational = (numerator << 16) | denominator;
            value_label(0xd100, &Value::Unsigned(rational))
        }
        // ISO：低 24 位为值，0xFFFFFF=Auto，高 8 位为多帧降噪（config.c）。
        DPC_SONY_ISO | DPC_SONY_ISO2 => {
            let base = number & 0x00ff_ffff;
            let noise_reduction = (number >> 24) as u8;
            let mut label = if base == 0x00ff_ffff {
                "Auto ISO".to_string()
            } else {
                base.to_string()
            };
            if noise_reduction > 0 {
                label.push_str(" Multi Frame Noise Reduction");
                if noise_reduction == 2 {
                    label.push('+');
                }
            }
            label
        }
        DPC_WHITE_BALANCE => match number {
            0x8001 => "Fluorescent: Warm White".into(),
            0x8002 => "Fluorescent: Cold White".into(),
            0x8003 => "Fluorescent: Day White".into(),
            0x8004 => "Fluorescent: Daylight".into(),
            0x8010 => "Cloudy".into(),
            0x8011 => "Shade".into(),
            0x8012 => "Choose Color Temperature".into(),
            0x8020 => "Preset 1".into(),
            0x8021 => "Preset 2".into(),
            0x8022 => "Preset 3".into(),
            0x8023 => "Preset".into(),
            0x8030 => "Underwater: Auto".into(),
            _ => value_label(code, value),
        },
        DPC_FOCUS_MODE => match number {
            1 => "MF".into(),
            2 => "AF".into(),
            3 => "AF Macro".into(),
            0x8004 => "AF-C".into(),
            0x8005 => "AF-A".into(),
            0x8006 => "DMF".into(),
            0x8007 => "Manual Reverse".into(),
            0x8008 => "AF-D".into(),
            0x8009 => "Preset Focus".into(),
            _ => value.token(),
        },
        _ => value_label(code, value),
    }
}

fn value_number(value: &Value) -> Option<u64> {
    match value {
        Value::Signed(n) => u64::try_from(*n).ok(),
        Value::Unsigned(n) => Some(*n),
        Value::Text(_) => None,
    }
}

// ---------------------------------------------------------------------------
// 拍摄
// ---------------------------------------------------------------------------

/// 触发一次拍摄（集成合同签名；收片由既有 receipt 队列负责）。
///
/// 对照 `camera_sony_capture`：0x9207 半按(0xD2C1=2) → 全按(0xD2C2=2) →
/// 等待合焦（0xD213==2/3，手动对焦 0x500A==1 时跳过；≤1 秒）→ 释放全按 →
/// 释放半按。任一步失败先释放已按下的键再返回错误（比上游更安全：
/// 上游等待循环出错会带着按键直接中止）。相机不支持 0x9207 时退标准
/// InitiateCapture(0,0)（library.c 的通用回退路径）。
pub fn capture(transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
    let manual_focus = fetch_property(transport, Protocol::V3, DPC_FOCUS_MODE)
        .ok()
        .flatten()
        .is_some_and(|property| value_number(&property.current) == Some(1));

    let half = transport.write(
        SDIO_CONTROL_DEVICE,
        &[DPC_SONY_SHUTTER_HALF_RELEASE as u32],
        &2u16.to_le_bytes(),
    )?;
    if half == RESP_OPERATION_NOT_SUPPORTED {
        let response = transport.command(PTP_OP_INITIATE_CAPTURE, &[0, 0])?;
        return require_ok(response, PTP_OP_INITIATE_CAPTURE);
    }
    require_ok(half, SDIO_CONTROL_DEVICE)?;

    let result = shutter_full_sequence(transport, manual_focus);
    // 无论如何都释放半按，不带按键退出。
    let release = transport.write(
        SDIO_CONTROL_DEVICE,
        &[DPC_SONY_SHUTTER_HALF_RELEASE as u32],
        &1u16.to_le_bytes(),
    );
    result.and_then(|_| require_ok(release?, SDIO_CONTROL_DEVICE))
}

/// 全按 → 等合焦 → 释放全按。全按未成功时直接上抛（不释放未按下的键）。
fn shutter_full_sequence(
    transport: &mut (impl PtpTransport + ?Sized),
    manual_focus: bool,
) -> Result<(), String> {
    let full = transport.write(
        SDIO_CONTROL_DEVICE,
        &[DPC_SONY_SHUTTER_RELEASE as u32],
        &2u16.to_le_bytes(),
    )?;
    require_ok(full, SDIO_CONTROL_DEVICE)?;

    if !manual_focus {
        wait_focus(transport);
    }

    let release = transport.write(
        SDIO_CONTROL_DEVICE,
        &[DPC_SONY_SHUTTER_RELEASE as u32],
        &1u16.to_le_bytes(),
    );
    require_ok(release?, SDIO_CONTROL_DEVICE)
}

/// 等待合焦（尽力而为，不放大错误）：0xD213 当前值 2/3 视为合焦；
/// 首次读取即失败（属性不存在/链路错误）直接放弃等待。
fn wait_focus(transport: &mut (impl PtpTransport + ?Sized)) {
    for attempt in 0..FOCUS_WAIT_ATTEMPTS {
        match transport.read(
            SONY_GET_DEVICE_PROP_DESC,
            &[DPC_SONY_FOCUS_FOUND as u32],
            16 * 1024,
        ) {
            Ok((response, data)) if response == RESP_OK => {
                match parse_sony_prop_desc(&data, Protocol::V3) {
                    Ok((property, _))
                        if !matches!(value_number(&property.current), Some(2 | 3)) => {}
                    _ => return, // 已合焦，或属性数据异常：不再等待
                }
            }
            _ => return, // 属性不可读：不再空转重试
        }
        if attempt + 1 < FOCUS_WAIT_ATTEMPTS {
            sleep(FOCUS_WAIT_DELAY);
        }
    }
}

// ---------------------------------------------------------------------------
// 实时取景（读魔术对象 0xFFFFC002；无显式启停指令——上游同路径）
// ---------------------------------------------------------------------------

/// 启动实时取景（集成合同签名）。
///
/// Sony 无独立取景启动操作码（library.c Sony 预览段不发任何启动指令，
/// 遥控会话建立后机身自动向对象 0xFFFFC002 提供帧流）。本函数只做就绪
/// 探测：GetObjectInfo(0xFFFFC002) 在 ZV-1 上可能持续约 1 秒回
/// InvalidObjectHandle，按上游节奏重试。不改变机身任何状态。
pub fn live_view_start(transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
    wait_live_object_ready(
        transport,
        LIVE_VIEW_START_ATTEMPTS,
        LIVE_VIEW_START_DELAY,
    )
}

/// 读取一帧实时取景 JPEG（集成合同签名）。
///
/// GetObject(0xFFFFC002)；AccessDenied 表示读得太快，按上游节奏重试。
/// 帧提取：数据前 4 字节小端偏移指向 JPEG SOI 时优先采用（gphoto2 #389：
/// 头部 blob 可能含假 SOI），否则退回 [`preview_jpeg`] 全扫描（JPEG 容器
/// 与 Nikon 路径一致）。
pub fn live_view_frame(transport: &mut (impl PtpTransport + ?Sized)) -> Result<Vec<u8>, String> {
    for attempt in 0..LIVE_VIEW_FRAME_ATTEMPTS {
        match transport.read(PTP_OP_GET_OBJECT, &[LIVE_VIEW_OBJECT], 16 * 1024 * 1024) {
            Ok((response, data)) if response == RESP_OK => {
                return extract_live_view_frame(&data);
            }
            Ok((response, _)) if response == RESP_ACCESS_DENIED => {}
            Ok((response, _)) => {
                return require_ok(response, PTP_OP_GET_OBJECT).map(|_| Vec::new())
            }
            Err(e) => return Err(e),
        }
        if attempt + 1 < LIVE_VIEW_FRAME_ATTEMPTS {
            sleep(LIVE_VIEW_FRAME_DELAY);
        }
    }
    Err("索尼取景帧读取过快，相机持续拒绝".into())
}

/// 结束实时取景（集成合同签名）：无操作。
///
/// 上游对 Sony 不发任何取景结束指令；本会话也从未改变机身取景状态，
/// 因此无需恢复。保留该函数以满足集成合同并显式声明语义。
pub fn live_view_stop(_transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
    Ok(())
}

fn wait_live_object_ready(
    transport: &mut (impl PtpTransport + ?Sized),
    attempts: usize,
    delay: Duration,
) -> Result<(), String> {
    for attempt in 0..attempts {
        match transport.read(PTP_OP_GET_OBJECT_INFO, &[LIVE_VIEW_OBJECT], 4096) {
            Ok((response, _)) if response == RESP_OK => return Ok(()),
            Ok((response, _)) if response == RESP_INVALID_OBJECT_HANDLE => {}
            Ok((response, _)) => return require_ok(response, PTP_OP_GET_OBJECT_INFO),
            Err(e) => return Err(e),
        }
        if attempt + 1 < attempts {
            sleep(delay);
        }
    }
    Err("索尼取景流未就绪（相机持续报告取景对象不存在）".into())
}

fn extract_live_view_frame(data: &[u8]) -> Result<Vec<u8>, String> {
    if data.len() > 4 {
        let offset = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        if offset < data.len() && data[offset..].starts_with(&[0xff, 0xd8]) {
            return preview_jpeg(&data[offset..]);
        }
    }
    preview_jpeg(data)
}

// ---------------------------------------------------------------------------
// 能力报告
// ---------------------------------------------------------------------------

/// 给 vendor 层的能力布尔集：按实际声明的操作生成，不硬编码 true。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SonyCapabilities {
    /// 可触发拍摄（0x9207 控制码，或标准 0x100E）。
    pub capture: bool,
    /// 取景流可读。推断项：Sony 无专用取景操作码，上游路径 = 遥控会话下
    /// 读对象 0xFFFFC002，因此要求标准 GetObjectInfo/GetObject **且**
    /// 遥控控制码 0x9207 在列（mass-storage 模式不满足）。
    pub live_view: bool,
    /// 参数可调（0x9205 直写，或标准 0x1016）。
    pub adjustable_settings: bool,
}

/// 按操作码表生成能力报告。
///
/// `operations` 建议传 GetDeviceInfo 的 OperationsSupported 与
/// [`RemoteSession::ext_codes`] 的并集（Sony 扩展码大多藏在 0x9202 之后）。
pub fn capabilities_supported(operations: &[u16]) -> SonyCapabilities {
    let contains = |code: u16| operations.contains(&code);
    SonyCapabilities {
        capture: contains(SDIO_CONTROL_DEVICE) || contains(PTP_OP_INITIATE_CAPTURE),
        live_view: contains(PTP_OP_GET_OBJECT_INFO)
            && contains(PTP_OP_GET_OBJECT)
            && contains(SDIO_CONTROL_DEVICE),
        adjustable_settings: contains(SDIO_SET_EXT_DEVICE_PROP_VALUE) || contains(SET_PROP_VALUE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 脚本化传输：按 FIFO 顺序回放期望的调用，并记录实际调用日志。
    struct Scripted {
        steps: Vec<Step>,
        log: Vec<(u16, Vec<u32>)>,
    }
    #[derive(Debug)]
    enum Step {
        Read(u16, Vec<u32>, u32, Vec<u8>),
        Write(u16, Vec<u32>, Vec<u8>, u32),
        Command(u16, Vec<u32>, u32),
    }
    impl Scripted {
        fn new(steps: Vec<Step>) -> Self {
            Self {
                steps,
                log: Vec::new(),
            }
        }
        fn next(&mut self) -> Step {
            self.steps.remove(0)
        }
    }
    impl PtpTransport for Scripted {
        fn read(
            &mut self,
            operation: u16,
            params: &[u32],
            _: usize,
        ) -> Result<(u32, Vec<u8>), String> {
            self.log.push((operation, params.to_vec()));
            match self.next() {
                Step::Read(op, expected, response, data) => {
                    assert_eq!(operation, op, "read 操作码不符");
                    assert_eq!(params, expected.as_slice(), "read 参数不符");
                    Ok((response, data))
                }
                step => panic!("脚本不匹配：read 0x{operation:04X} 撞上 {step:?}"),
            }
        }
        fn write(
            &mut self,
            operation: u16,
            params: &[u32],
            data: &[u8],
        ) -> Result<u32, String> {
            self.log.push((operation, params.to_vec()));
            match self.next() {
                Step::Write(op, expected, expected_data, response) => {
                    assert_eq!(operation, op, "write 操作码不符");
                    assert_eq!(params, expected.as_slice(), "write 参数不符");
                    assert_eq!(data, expected_data.as_slice(), "write 数据不符");
                    Ok(response)
                }
                step => panic!("脚本不匹配：write 0x{operation:04X} 撞上 {step:?}"),
            }
        }
        fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
            self.log.push((operation, params.to_vec()));
            match self.next() {
                Step::Command(op, expected, response) => {
                    assert_eq!(operation, op, "command 操作码不符");
                    assert_eq!(params, expected.as_slice(), "command 参数不符");
                    Ok(response)
                }
                step => panic!("脚本不匹配：command 0x{operation:04X} 撞上 {step:?}"),
            }
        }
    }

    // ---- 字节构造 ----

    /// `ptp_control::Form` 未实现 PartialEq（不能改该文件），测试用手写比较。
    fn same_form(actual: &Form, expected: &Form) -> bool {
        match (actual, expected) {
            (Form::None, Form::None) => true,
            (Form::Range(a1, b1, c1), Form::Range(a2, b2, c2)) => {
                a1 == a2 && b1 == b2 && c1 == c2
            }
            (Form::Enum(x), Form::Enum(y)) => x == y,
            _ => false,
        }
    }

    /// Sony DPD：u16 码 · u16 类型 · u8 getset · u8 isenabled · 值×2 · 表单。
    fn dpd(code: u16, data_type: u16, getset: u8, isenabled: u8) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(code.to_le_bytes());
        bytes.extend(data_type.to_le_bytes());
        bytes.push(getset);
        bytes.push(isenabled);
        bytes
    }
    fn u32_value(bytes: &mut Vec<u8>, value: u32) {
        bytes.extend(value.to_le_bytes());
    }
    fn u16_value(bytes: &mut Vec<u8>, value: u16) {
        bytes.extend(value.to_le_bytes());
    }
    fn enum_u32(values: &[u32]) -> Vec<u8> {
        let mut bytes = vec![2u8];
        u16_value(&mut bytes, values.len() as u16);
        for value in values {
            u32_value(&mut bytes, *value);
        }
        bytes
    }
    fn enum_u16(values: &[u16]) -> Vec<u8> {
        let mut bytes = vec![2u8];
        u16_value(&mut bytes, values.len() as u16);
        for value in values {
            u16_value(&mut bytes, *value);
        }
        bytes
    }
    /// u32 枚举属性（默认/当前 + 枚举表单）。
    fn prop_u32_enum(code: u16, getset: u8, isenabled: u8, current: u32, values: &[u32]) -> Vec<u8> {
        let mut bytes = dpd(code, 6, getset, isenabled);
        u32_value(&mut bytes, values[0]);
        u32_value(&mut bytes, current);
        bytes.extend(enum_u32(values));
        bytes
    }
    fn prop_u16_enum(code: u16, getset: u8, isenabled: u8, current: u16, values: &[u16]) -> Vec<u8> {
        let mut bytes = dpd(code, 4, getset, isenabled);
        u16_value(&mut bytes, values[0]);
        u16_value(&mut bytes, current);
        bytes.extend(enum_u16(values));
        bytes
    }
    /// 0x9202 扩展信息：u16 版本 + 两个 AUINT16（码全在第一个数组）。
    fn ext_info(version: u16, codes: &[u16]) -> Vec<u8> {
        let mut bytes = version.to_le_bytes().to_vec();
        bytes.extend((codes.len() as u32).to_le_bytes());
        for code in codes {
            bytes.extend(code.to_le_bytes());
        }
        bytes.extend(0u32.to_le_bytes()); // 第二个数组为空
        bytes
    }

    // ---- is_sony ----

    #[test]
    fn vendor_requires_both_name_and_sony_opcode() {
        assert!(is_sony("Sony Corporation", &[0x9202]));
        assert!(is_sony("SONY", &[0x1001, 0x9201]));
        // Nikon 同样声明 0x9201/02/03/05/07——名字才是判别器。
        assert!(!is_sony("Nikon Corporation", &[0x9201, 0x9202, 0x9203]));
        // 名字像但没进遥控协议的 Sony 存储设备。
        assert!(!is_sony("Sony Corporation", &[0x1001, 0x1002]));
    }

    // ---- init ----

    #[test]
    fn init_remote_replays_verified_handshake() {
        let mut transport = Scripted::new(vec![
            Step::Read(SDIO_CONNECT, vec![1, 0, 0], RESP_OK, vec![]),
            Step::Read(SDIO_CONNECT, vec![2, 0, 0], RESP_OK, vec![]),
            Step::Read(
                SDIO_GET_EXT_DEVICE_INFO,
                vec![PROTOCOL_3_00 as u32, 1],
                RESP_OK,
                ext_info(PROTOCOL_3_00, &[0x5007, 0xc203]),
            ),
            Step::Read(SDIO_CONNECT, vec![3, 0, 0], RESP_OK, vec![]),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![DPC_SONY_PRIORITY_MODE as u32],
                vec![1],
                RESP_OK,
            ),
        ]);
        let session = init_remote_detailed(&mut transport).unwrap();
        assert_eq!(session.protocol, Protocol::V3);
        assert_eq!(session.ext_codes, vec![0x5007, 0xc203]);
        assert_eq!(transport.log.len(), 5);
    }

    #[test]
    fn init_remote_reports_old_protocol_version() {
        let mut transport = Scripted::new(vec![
            Step::Read(SDIO_CONNECT, vec![1, 0, 0], RESP_OK, vec![]),
            Step::Read(SDIO_CONNECT, vec![2, 0, 0], RESP_OK, vec![]),
            Step::Read(
                SDIO_GET_EXT_DEVICE_INFO,
                vec![PROTOCOL_3_00 as u32, 1],
                RESP_OK,
                ext_info(PROTOCOL_2_00, &[0xd20d]),
            ),
            Step::Read(SDIO_CONNECT, vec![3, 0, 0], RESP_OK, vec![]),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![DPC_SONY_PRIORITY_MODE as u32],
                vec![1],
                RESP_OK,
            ),
        ]);
        assert_eq!(
            init_remote_detailed(&mut transport).unwrap().protocol,
            Protocol::V2
        );
    }

    #[test]
    fn init_remote_retries_empty_ext_info_and_tolerates_priority_mode_error() {
        let mut transport = Scripted::new(vec![
            Step::Read(SDIO_CONNECT, vec![1, 0, 0], RESP_OK, vec![]),
            Step::Read(SDIO_CONNECT, vec![2, 0, 0], RESP_OK, vec![]),
            Step::Read(
                SDIO_GET_EXT_DEVICE_INFO,
                vec![PROTOCOL_3_00 as u32, 1],
                RESP_OK,
                Vec::new(),
            ),
            Step::Read(
                SDIO_GET_EXT_DEVICE_INFO,
                vec![PROTOCOL_3_00 as u32, 1],
                RESP_OK,
                ext_info(PROTOCOL_3_00, &[0x5005]),
            ),
            Step::Read(SDIO_CONNECT, vec![3, 0, 0], RESP_OK, vec![]),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![DPC_SONY_PRIORITY_MODE as u32],
                vec![1],
                0xa106, // PTP_RC_Sony_CameraStatusError：上游仅记录，不阻断
            ),
        ]);
        let session = init_remote_detailed(&mut transport).unwrap();
        assert_eq!(session.protocol, Protocol::V3);
        assert_eq!(session.ext_codes, vec![0x5005]);
    }

    #[test]
    fn init_remote_propagates_connect_failure() {
        let mut transport = Scripted::new(vec![Step::Read(
            SDIO_CONNECT,
            vec![1, 0, 0],
            RESP_OPERATION_NOT_SUPPORTED,
            vec![],
        )]);
        assert!(init_remote(&mut transport).is_err());
    }

    // ---- Sony DPD 解析 ----

    #[test]
    fn parse_desc_mode3_and_mode2_writable_rules() {
        let bytes = prop_u32_enum(0xd20d, 1, 1, 0x0001_0032, &[0x0001_0032, 0x0002_0001]);
        let (property, consumed) = parse_sony_prop_desc(&bytes, Protocol::V3).unwrap();
        assert_eq!(consumed, bytes.len());
        assert!(property.writable);
        assert_eq!(property.current, Value::Unsigned(0x0001_0032));
        assert!(same_form(
            &property.form,
            &Form::Enum(vec![
                Value::Unsigned(0x0001_0032),
                Value::Unsigned(0x0002_0001)
            ])
        ));

        // V3：getset=0 或 isenabled!=1 → 只读。
        assert!(
            !parse_sony_prop_desc(&prop_u32_enum(0xd20d, 0, 1, 1, &[1]), Protocol::V3)
                .unwrap()
                .0
                .writable
        );
        assert!(
            !parse_sony_prop_desc(&prop_u32_enum(0xd20d, 1, 2, 1, &[1]), Protocol::V3)
                .unwrap()
                .0
                .writable
        );

        // V2：0x80 控制位可写；isenabled==1 可写；「仅显示」FocusMode 例外。
        assert!(
            parse_sony_prop_desc(&prop_u16_enum(0x500a, 0x82, 2, 1, &[1]), Protocol::V2)
                .unwrap()
                .0
                .writable
        );
        assert!(
            parse_sony_prop_desc(&prop_u16_enum(0x5007, 0, 1, 280, &[280]), Protocol::V2)
                .unwrap()
                .0
                .writable
        );
        assert!(
            parse_sony_prop_desc(&prop_u16_enum(0x500a, 0, 2, 1, &[1, 2]), Protocol::V2)
                .unwrap()
                .0
                .writable
        );
        assert!(
            !parse_sony_prop_desc(&prop_u16_enum(0x5007, 0, 2, 280, &[280]), Protocol::V2)
                .unwrap()
                .0
                .writable
        );
        assert!(
            !parse_sony_prop_desc(&prop_u16_enum(0x5007, 0, 0, 280, &[280]), Protocol::V2)
                .unwrap()
                .0
                .writable
        );
    }

    #[test]
    fn parse_desc_string_without_current_or_form() {
        let mut bytes = dpd(0xd403, 0xffff, 1, 1);
        bytes.push(4); // 4 个 UTF-16 单元含 NUL
        for unit in [0x41u16, 0x42, 0x43, 0] {
            bytes.extend(unit.to_le_bytes());
        }
        let (property, consumed) = parse_sony_prop_desc(&bytes, Protocol::V3).unwrap();
        assert_eq!(consumed, bytes.len());
        assert_eq!(property.current, Value::Text(String::new()));
        assert!(same_form(&property.form, &Form::None));
    }

    #[test]
    fn parse_desc_range_form() {
        let mut bytes = dpd(0xd226, 6, 1, 1);
        u32_value(&mut bytes, 100);
        u32_value(&mut bytes, 400);
        bytes.push(1); // Range
        u32_value(&mut bytes, 50);
        u32_value(&mut bytes, 25600);
        u32_value(&mut bytes, 50);
        let (property, _) = parse_sony_prop_desc(&bytes, Protocol::V3).unwrap();
        assert!(same_form(
            &property.form,
            &Form::Range(
                Value::Unsigned(50),
                Value::Unsigned(25600),
                Value::Unsigned(50)
            )
        ));
    }

    #[test]
    fn parse_desc_truncated_data_errors() {
        let bytes = prop_u32_enum(0xd20d, 1, 1, 50, &[50, 100]);
        // 头部 6 + 默认 4 + 当前 4 = 14：恰好在当前值后结束是合法的（无表单）。
        let (property, consumed) = parse_sony_prop_desc(&bytes[..14], Protocol::V3).unwrap();
        assert_eq!(consumed, 14);
        assert!(same_form(&property.form, &Form::None));
        // 其余任何截断（表单字节半截/枚举缺项）都必须报错。
        for len in 0..14 {
            assert!(parse_sony_prop_desc(&bytes[..len], Protocol::V3).is_err());
        }
        for len in 15..bytes.len() {
            assert!(parse_sony_prop_desc(&bytes[..len], Protocol::V3).is_err());
        }
    }

    // ---- settings ----

    #[test]
    fn settings_use_batch_and_sony_labels() {
        let mut batch = Vec::new();
        batch.extend(2u32.to_le_bytes()); // 条数（上游不校验）
        batch.extend(0u32.to_le_bytes()); // 保留 0
        batch.extend(prop_u32_enum(
            0xd20d,
            1,
            1,
            0x0001_0032,
            &[0x0001_0032, 0x0002_0001],
        ));
        batch.extend(prop_u16_enum(0x5005, 0, 1, 0x8010, &[1, 0x8010])); // 白平衡只读
        let mut transport = Scripted::new(vec![Step::Read(
            SDIO_GET_ALL_EXT_DEVICE_PROP_INFO,
            Vec::new(),
            RESP_OK,
            batch,
        )]);
        let values = settings(&mut transport, Protocol::V3).unwrap();
        assert_eq!(values.len(), 2);
        let shutter = values.iter().find(|s| s.id == "shutterspeed").unwrap();
        assert_eq!(shutter.current, "65586"); // 0x00010032
        assert!(shutter.writable);
        assert_eq!(shutter.options[0].label, "1/50s");
        assert_eq!(shutter.options[1].label, "2s");
        let white = values.iter().find(|s| s.id == "whitebalance").unwrap();
        assert!(!white.writable);
        assert_eq!(white.options[1].label, "Cloudy");
    }

    #[test]
    fn settings_fall_back_to_per_property_reads() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SDIO_GET_ALL_EXT_DEVICE_PROP_INFO,
                Vec::new(),
                RESP_OPERATION_NOT_SUPPORTED,
                vec![],
            ),
            // ISO 新码不支持 → 旧码；对焦模式属性不支持 → 跳过。
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd20d],
                RESP_OK,
                prop_u32_enum(0xd20d, 1, 1, 0x0001_0032, &[0x0001_0032]),
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x5007],
                RESP_OK,
                prop_u16_enum(0x5007, 1, 1, 280, &[280]),
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd226],
                RESP_OPERATION_NOT_SUPPORTED,
                vec![],
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd21e],
                RESP_OK,
                prop_u32_enum(0xd21e, 1, 1, 100, &[100, 200]),
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x5005],
                RESP_OK,
                prop_u16_enum(0x5005, 1, 1, 4, &[1, 4]),
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x500a],
                RESP_INVALID_OBJECT_HANDLE,
                vec![],
            ),
        ]);
        let values = settings(&mut transport, Protocol::V3).unwrap();
        let ids: Vec<&str> = values.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["shutterspeed", "f-number", "iso", "whitebalance"]);
        let iso = values.iter().find(|s| s.id == "iso").unwrap();
        assert_eq!(iso.options[1].label, "200");
    }

    #[test]
    fn settings_disable_mode2_tuning_writes() {
        let mut batch = Vec::new();
        batch.extend(3u32.to_le_bytes());
        batch.extend(0u32.to_le_bytes());
        batch.extend(prop_u32_enum(0xd20d, 1, 1, 0x0001_0032, &[0x0001_0032]));
        batch.extend(prop_u32_enum(0xd21e, 1, 1, 100, &[100]));
        batch.extend(prop_u16_enum(0x500a, 1, 1, 2, &[1, 2]));
        let mut transport = Scripted::new(vec![Step::Read(
            SDIO_GET_ALL_EXT_DEVICE_PROP_INFO,
            Vec::new(),
            RESP_OK,
            batch,
        )]);
        let values = settings(&mut transport, Protocol::V2).unwrap();
        for setting in &values {
            match setting.id.as_str() {
                "shutterspeed" | "iso" => assert!(!setting.writable, "{} 应禁写", setting.id),
                "focusmode" => assert!(setting.writable, "focusmode 应保持可写"),
                other => panic!("意外参数 {other}"),
            }
        }
    }

    // ---- set_setting ----

    #[test]
    fn set_setting_writes_ext_opcode_with_standard_value_encoding() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd226],
                RESP_OK,
                prop_u32_enum(0xd226, 1, 1, 100, &[100, 200]),
            ),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![0xd226],
                200u32.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        set_setting(&mut transport, Protocol::V3, "iso", "200").unwrap();
    }

    #[test]
    fn set_setting_resolves_iso_fallback_code() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd226],
                RESP_OPERATION_NOT_SUPPORTED,
                vec![],
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd21e],
                RESP_OK,
                prop_u32_enum(0xd21e, 1, 1, 100, &[100, 200]),
            ),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![0xd21e],
                200u32.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        set_setting(&mut transport, Protocol::V3, "iso", "200").unwrap();
    }

    #[test]
    fn set_setting_fnumber_mode3_packs_u32_like_upstream() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x5007],
                RESP_OK,
                prop_u16_enum(0x5007, 1, 1, 280, &[280, 350]),
            ),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![0x5007],
                350u32.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        set_setting(&mut transport, Protocol::V3, "f-number", "350").unwrap();
    }

    #[test]
    fn set_setting_falls_back_to_standard_opcode() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x5005],
                RESP_OK,
                prop_u16_enum(0x5005, 1, 1, 1, &[1, 4]),
            ),
            Step::Write(
                SDIO_SET_EXT_DEVICE_PROP_VALUE,
                vec![0x5005],
                4u16.to_le_bytes().to_vec(),
                RESP_OPERATION_NOT_SUPPORTED,
            ),
            Step::Write(
                SET_PROP_VALUE,
                vec![0x5005],
                4u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        set_setting(&mut transport, Protocol::V3, "whitebalance", "4").unwrap();
    }

    #[test]
    fn set_setting_mode2_tuning_returns_disabled_error() {
        let mut transport = Scripted::new(vec![]);
        let error = set_setting(&mut transport, Protocol::V2, "shutterspeed", "65586").unwrap_err();
        assert!(error.contains("未核对"), "错误应说明禁用原因：{error}");
    }

    #[test]
    fn set_setting_rejects_values_outside_camera_options() {
        let mut transport = Scripted::new(vec![Step::Read(
            SONY_GET_DEVICE_PROP_DESC,
            vec![0xd226],
            RESP_OK,
            prop_u32_enum(0xd226, 1, 1, 100, &[100, 200]),
        )]);
        assert!(set_setting(&mut transport, Protocol::V3, "iso", "300").is_err());

        let mut readonly_bytes = dpd(0xd226, 6, 0, 2); // 只读（仅显示）
        u32_value(&mut readonly_bytes, 100);
        u32_value(&mut readonly_bytes, 100);
        readonly_bytes.push(0);
        let mut readonly = Scripted::new(vec![Step::Read(
            SONY_GET_DEVICE_PROP_DESC,
            vec![0xd226],
            RESP_OK,
            readonly_bytes,
        )]);
        assert!(set_setting(&mut readonly, Protocol::V3, "iso", "100").is_err());
    }

    // ---- capture ----

    fn focus_found(value: u8) -> Vec<u8> {
        let mut bytes = dpd(0xd213, 2, 0, 2);
        bytes.push(1);
        bytes.push(value);
        bytes.push(0); // 无表单
        bytes
    }

    #[test]
    fn capture_uses_sony_control_device_press_sequence() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x500a],
                RESP_OK,
                prop_u16_enum(0x500a, 1, 1, 0x8004, &[1, 2, 0x8004]),
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                2u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c2],
                2u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0xd213],
                RESP_OK,
                focus_found(2),
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c2],
                1u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                1u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        capture(&mut transport).unwrap();
        assert_eq!(transport.log.len(), 6);
    }

    #[test]
    fn capture_skips_focus_wait_in_manual_focus() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x500a],
                RESP_OK,
                prop_u16_enum(0x500a, 1, 1, 1, &[1, 2]),
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                2u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c2],
                2u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c2],
                1u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                1u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        capture(&mut transport).unwrap();
    }

    #[test]
    fn capture_falls_back_to_standard_initiate_capture() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x500a],
                RESP_OPERATION_NOT_SUPPORTED,
                vec![],
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                2u16.to_le_bytes().to_vec(),
                RESP_OPERATION_NOT_SUPPORTED,
            ),
            Step::Command(PTP_OP_INITIATE_CAPTURE, vec![0, 0], RESP_OK),
        ]);
        capture(&mut transport).unwrap();
    }

    #[test]
    fn capture_releases_half_press_when_full_press_fails() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                SONY_GET_DEVICE_PROP_DESC,
                vec![0x500a],
                RESP_OK,
                prop_u16_enum(0x500a, 1, 1, 1, &[1]),
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                2u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c2],
                2u16.to_le_bytes().to_vec(),
                0xa106,
            ),
            Step::Write(
                SDIO_CONTROL_DEVICE,
                vec![0xd2c1],
                1u16.to_le_bytes().to_vec(),
                RESP_OK,
            ),
        ]);
        assert!(capture(&mut transport).is_err());
        // 最后一条必须是半按释放，不能带着按键退出。
        let last = transport.log.last().unwrap();
        assert_eq!(last.0, SDIO_CONTROL_DEVICE);
        assert_eq!(last.1, vec![DPC_SONY_SHUTTER_HALF_RELEASE as u32]);
    }

    // ---- live view ----

    #[test]
    fn live_view_frame_honors_leading_offset_hint_over_false_soi() {
        let real_jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x01, 0xff, 0xd9];
        let mut data = Vec::new();
        data.extend(9u32.to_le_bytes()); // 偏移提示
        data.extend([0x00, 0xff, 0xd8, 0xff, 0xd9]); // 头部 blob 含假 SOI
        data.extend_from_slice(&real_jpeg);
        let frame = extract_live_view_frame(&data).unwrap();
        assert_eq!(frame, real_jpeg.to_vec());
    }

    #[test]
    fn live_view_frame_without_hint_scans_for_jpeg() {
        let data = [0x00, 0x11, 0x22, 0x33, 0xff, 0xd8, 0xff, 0x02, 0xff, 0xd9];
        assert_eq!(extract_live_view_frame(&data).unwrap(), data[4..].to_vec());
    }

    #[test]
    fn live_view_start_waits_for_object_availability() {
        let mut transport = Scripted::new(vec![
            Step::Read(
                PTP_OP_GET_OBJECT_INFO,
                vec![LIVE_VIEW_OBJECT],
                RESP_INVALID_OBJECT_HANDLE,
                vec![],
            ),
            Step::Read(PTP_OP_GET_OBJECT_INFO, vec![LIVE_VIEW_OBJECT], RESP_OK, vec![0x30; 48]),
        ]);
        live_view_start(&mut transport).unwrap();

        let mut transport = Scripted::new(vec![
            Step::Read(
                PTP_OP_GET_OBJECT_INFO,
                vec![LIVE_VIEW_OBJECT],
                RESP_INVALID_OBJECT_HANDLE,
                vec![],
            ),
            Step::Read(
                PTP_OP_GET_OBJECT_INFO,
                vec![LIVE_VIEW_OBJECT],
                RESP_INVALID_OBJECT_HANDLE,
                vec![],
            ),
        ]);
        assert!(wait_live_object_ready(&mut transport, 2, Duration::from_millis(1)).is_err());
    }

    #[test]
    fn live_view_frame_retries_access_denied() {
        let mut data = Vec::new();
        data.extend(4u32.to_le_bytes());
        data.extend_from_slice(&[0xff, 0xd8, 0xff, 0x01, 0xff, 0xd9]);
        let mut transport = Scripted::new(vec![
            Step::Read(
                PTP_OP_GET_OBJECT,
                vec![LIVE_VIEW_OBJECT],
                RESP_ACCESS_DENIED,
                vec![],
            ),
            Step::Read(PTP_OP_GET_OBJECT, vec![LIVE_VIEW_OBJECT], RESP_OK, data),
        ]);
        let frame = live_view_frame(&mut transport).unwrap();
        assert_eq!(frame, vec![0xff, 0xd8, 0xff, 0x01, 0xff, 0xd9]);
    }

    #[test]
    fn live_view_stop_is_a_documented_noop() {
        struct UntouchedTransport;
        impl PtpTransport for UntouchedTransport {
            fn read(&mut self, _: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
                panic!("live_view_stop 不应触碰传输");
            }
            fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
                panic!("live_view_stop 不应触碰传输");
            }
            fn command(&mut self, _: u16, _: &[u32]) -> Result<u32, String> {
                panic!("live_view_stop 不应触碰传输");
            }
        }
        let mut transport = UntouchedTransport;
        live_view_stop(&mut transport).unwrap();
    }

    // ---- capabilities ----

    #[test]
    fn capabilities_come_from_declared_operations_only() {
        let caps = capabilities_supported(&[
            SDIO_CONTROL_DEVICE,
            PTP_OP_GET_OBJECT_INFO,
            PTP_OP_GET_OBJECT,
            SDIO_SET_EXT_DEVICE_PROP_VALUE,
        ]);
        assert!(caps.capture && caps.live_view && caps.adjustable_settings);

        // 只有标准拍摄码：可拍、无取景、无可调参数。
        let caps = capabilities_supported(&[PTP_OP_INITIATE_CAPTURE]);
        assert!(caps.capture);
        assert!(!caps.live_view);
        assert!(!caps.adjustable_settings);

        let caps = capabilities_supported(&[PTP_OP_INITIATE_CAPTURE, SET_PROP_VALUE]);
        assert!(caps.adjustable_settings);

        // 存储模式（无遥控控制码）：全部禁用。
        let caps = capabilities_supported(&[PTP_OP_GET_OBJECT_INFO, PTP_OP_GET_OBJECT]);
        assert!(!caps.capture && !caps.live_view && !caps.adjustable_settings);

        assert!(!capabilities_supported(&[]).capture);
    }

    // ---- labels ----

    #[test]
    fn sony_labels_match_upstream_tables() {
        assert_eq!(sony_value_label(0xd20d, &Value::Unsigned(0x0001_0032)), "1/50s");
        // 30/10 归一化为 3 秒（config.c 对 Sony 高速快门的处理）。
        assert_eq!(sony_value_label(0xd20d, &Value::Unsigned(0x001e_000a)), "3s");
        assert_eq!(sony_value_label(0xd20d, &Value::Unsigned(0)), "Bulb");
        assert_eq!(sony_value_label(0xd21e, &Value::Unsigned(100)), "100");
        assert_eq!(
            sony_value_label(0xd226, &Value::Unsigned(0x00ff_ffff)),
            "Auto ISO"
        );
        assert_eq!(
            sony_value_label(0xd226, &Value::Unsigned(0x0200_0064)),
            "100 Multi Frame Noise Reduction+"
        );
        assert_eq!(sony_value_label(0x500a, &Value::Unsigned(0x8006)), "DMF");
        assert_eq!(sony_value_label(0x5007, &Value::Unsigned(280)), "2.8");
    }
}
