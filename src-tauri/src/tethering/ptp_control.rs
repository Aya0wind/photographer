//! Portable camera-control values. OS transports must not interpret UI values.
//! Standard PTP property layouts and Nikon's shutter encoding are defined by
//! ISO 15740 and libgphoto2's public PTP implementation.

use std::time::Duration;

use super::backend::{CameraSetting, CameraSettingOption, SettingKind};

pub const GET_PROP_DESC: u16 = 0x1014;
pub const GET_PROP_VALUE: u16 = 0x1015;
pub const SET_PROP_VALUE: u16 = 0x1016;
pub const NIKON_LIVE_START: u16 = 0x9201;
pub const NIKON_LIVE_END: u16 = 0x9202;
pub const NIKON_LIVE_IMAGE: u16 = 0x9203;
pub const NIKON_AUTOFOCUS: u16 = 0x90c1;
pub const NIKON_FOCUS_AREA: u16 = 0x9205;
pub const NIKON_EVENTS: u16 = 0x90c7;

/// GetDevicePropDesc responses meaning "this camera does not have the
/// property" (PTP_RC_DevicePropNotSupported, ISO 15740): the property is
/// skipped, not treated as a connection failure.
pub const RESP_PROP_NOT_SUPPORTED: u32 = 0x200A;
/// Nikon vendor response treated the same way during property probing
/// (upstream ptp.h names 0xA006 NIKON_WbResetError; plan pins it as a
/// per-property "unsupported" skip). TODO-verify on hardware.
pub const RESP_NIKON_PROP_SKIP: u32 = 0xA006;

/// Camera vendor family. Vendor operation-code ranges overlap between brands
/// (e.g. Nikon 0x9201-0x9207 vs Sony 0x9205 SDIO) with different meanings,
/// so the brand decides which vendor protocol may run — never the raw codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Brand {
    #[default]
    Standard,
    Nikon,
    Sony,
}

/// Brand detection from GetDeviceInfo identity strings. A Manufacturer
/// containing "NIKON"/"SONY" (case-insensitive) selects the vendor family;
/// anything else falls back to Standard, which never receives vendor codes.
/// `model` and `operations` are accepted so later per-model profile checks
/// have a single detection point, but detection deliberately never guesses
/// from codes or model names alone.
pub fn detect_brand(manufacturer: &str, model: &str, operations: &[u16]) -> Brand {
    let _ = (model, operations);
    let maker = manufacturer.to_ascii_uppercase();
    if maker.contains("NIKON") {
        Brand::Nikon
    } else if maker.contains("SONY") {
        Brand::Sony
    } else {
        Brand::Standard
    }
}

/// Small, explicit surface: exposure triangle, white balance and focus only.
#[derive(Debug, Clone, Copy)]
pub struct SettingSpec {
    pub code: u16,
    pub id: &'static str,
    pub label: &'static str,
}

pub const CORE_PROPERTIES: &[SettingSpec] = &[
    SettingSpec {
        code: 0xd100,
        id: "shutterspeed",
        label: "Shutter speed",
    },
    SettingSpec {
        code: 0x500d,
        id: "shutterspeed",
        label: "Shutter speed",
    },
    SettingSpec {
        code: 0x5007,
        id: "f-number",
        label: "Aperture",
    },
    SettingSpec {
        code: 0x500f,
        id: "iso",
        label: "ISO",
    },
    SettingSpec {
        code: 0x5005,
        id: "whitebalance",
        label: "White balance",
    },
    SettingSpec {
        code: 0xd061,
        id: "focusmode",
        label: "Focus mode",
    },
    SettingSpec {
        code: 0x500a,
        id: "focusmode",
        label: "Focus mode",
    },
];

/// Transport seam used by native Windows/macOS adapters and deterministic tests.
pub trait PtpTransport {
    fn read(
        &mut self,
        operation: u16,
        params: &[u32],
        limit: usize,
    ) -> Result<(u32, Vec<u8>), String>;
    fn write(&mut self, operation: u16, params: &[u32], data: &[u8]) -> Result<u32, String>;
    fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String>;
}

pub fn require_ok(response: u32, operation: u16) -> Result<(), String> {
    if response == 0x2001 {
        Ok(())
    } else {
        Err(format!(
            "相机拒绝操作 0x{operation:04X}（响应 0x{response:04X}）"
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Signed(i64),
    Unsigned(u64),
    Text(String),
}
impl Value {
    pub fn token(&self) -> String {
        match self {
            Self::Signed(n) => n.to_string(),
            Self::Unsigned(n) => n.to_string(),
            Self::Text(s) => s.clone(),
        }
    }
    fn number(&self) -> Option<i128> {
        match self {
            Self::Signed(n) => Some(*n as i128),
            Self::Unsigned(n) => Some(*n as i128),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Form {
    None,
    Range(Value, Value, Value),
    Enum(Vec<Value>),
}
#[derive(Debug, Clone)]
pub struct Property {
    pub code: u16,
    pub data_type: u16,
    pub writable: bool,
    pub current: Value,
    pub form: Form,
}

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

impl Property {
    pub fn parse(data: &[u8]) -> Result<Self, String> {
        let mut reader = Reader { data, offset: 0 };
        let code = reader.u16()?;
        let data_type = reader.u16()?;
        let writable = reader.u8()? == 1;
        reader.value(data_type)?; // factory default
        let current = reader.value(data_type)?;
        let form = match reader.u8()? {
            0 => Form::None,
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
            _ => return Err("PTP 参数选项格式无效".into()),
        };
        Ok(Self {
            code,
            data_type,
            writable,
            current,
            form,
        })
    }

    pub fn encode(&self, token: &str) -> Result<Vec<u8>, String> {
        if !self.writable {
            return Err("当前拍摄模式下此参数不可修改".into());
        }
        let value = if self.data_type == 0xffff {
            Value::Text(token.into())
        } else if self.data_type & 1 == 1 {
            Value::Signed(token.parse().map_err(|_| "参数值无效")?)
        } else {
            Value::Unsigned(token.parse().map_err(|_| "参数值无效")?)
        };
        match &self.form {
            Form::Enum(values) if !values.contains(&value) => {
                return Err("参数不在相机支持的选项中".into())
            }
            Form::Range(min, max, step) => {
                let n = value.number().ok_or("参数值无效")?;
                let lo = min.number().ok_or("参数范围无效")?;
                let hi = max.number().ok_or("参数范围无效")?;
                let step = step.number().ok_or("参数范围无效")?;
                if n < lo || n > hi || (step > 0 && (n - lo) % step != 0) {
                    return Err("参数不在相机支持的范围中".into());
                }
            }
            _ => {}
        }
        macro_rules! encode {
            ($t:ty, $n:expr) => {
                <$t>::try_from($n)
                    .map_err(|_| "参数超出数据类型范围")?
                    .to_le_bytes()
                    .to_vec()
            };
        }
        Ok(match value {
            Value::Signed(n) => match self.data_type {
                1 => encode!(i8, n),
                3 => encode!(i16, n),
                5 => encode!(i32, n),
                7 => n.to_le_bytes().to_vec(),
                _ => return Err("参数类型不匹配".into()),
            },
            Value::Unsigned(n) => match self.data_type {
                2 => encode!(u8, n),
                4 => encode!(u16, n),
                6 => encode!(u32, n),
                8 => n.to_le_bytes().to_vec(),
                _ => return Err("参数类型不匹配".into()),
            },
            Value::Text(s) => {
                let mut units: Vec<u16> = s.encode_utf16().collect();
                if units.len() > 254 || units.contains(&0) {
                    return Err("参数字符串无效".into());
                }
                units.push(0);
                let mut bytes = vec![units.len() as u8];
                for unit in units {
                    bytes.extend_from_slice(&unit.to_le_bytes());
                }
                bytes
            }
        })
    }

    pub fn setting(&self, spec: SettingSpec, brand: Brand) -> CameraSetting {
        let sampled = self.option_values();
        // Continuous ranges without a sampled ladder stay slider-shaped; the
        // three exposure codes below always render as discrete dropdowns.
        let continuous = matches!(self.form, Form::Range(..)) && sampled.is_empty();
        let mut choices = sampled;
        if !choices.contains(&self.current) {
            choices.push(self.current.clone());
        }
        let bounds = self.range_bounds();
        CameraSetting {
            id: spec.id.into(),
            label: spec.label.into(),
            kind: if continuous {
                SettingKind::Range
            } else {
                SettingKind::Choice
            },
            current: self.current.token(),
            writable: self.writable,
            options: choices
                .iter()
                .map(|value| CameraSettingOption {
                    value: value.token(),
                    label: value_label_brand(brand, self.code, value),
                })
                .collect(),
            min: bounds.map(|b| b.0),
            max: bounds.map(|b| b.1),
            step: bounds.and_then(|b| b.2),
        }
    }

    /// Discrete candidate values: enum form verbatim; range form sampled into
    /// a photographic ladder for the exposure-triangle codes. Empty when only
    /// the current value is known (caller keeps the current value listed).
    fn option_values(&self) -> Vec<Value> {
        match &self.form {
            Form::Enum(values) => values.clone(),
            Form::Range(min, max, step) => sample_range(self.code, min, max, step),
            Form::None => Vec::new(),
        }
    }

    /// Validated (min, max, step?) metadata for slider UIs. None when the
    /// descriptor bounds are unusable (min > max, negative, non-numeric).
    fn range_bounds(&self) -> Option<(f64, f64, Option<f64>)> {
        let Form::Range(min, max, step) = &self.form else {
            return None;
        };
        let lo = min.number()?;
        let hi = max.number()?;
        if lo > hi || lo < 0 {
            return None;
        }
        Some((
            lo as f64,
            hi as f64,
            step.number().filter(|s| *s > 0).map(|s| s as f64),
        ))
    }
}

/// Full-stop shutter scale as (numerator, denominator), slowest to fastest.
/// Used both for standard ExposureTime (0x500D, 0.1 ms units) and Nikon's
/// packed shutter code (0xD100).
const SHUTTER_FULL_STOPS: &[(u32, u32)] = &[
    (30, 1),
    (15, 1),
    (8, 1),
    (4, 1),
    (2, 1),
    (1, 1),
    (1, 2),
    (1, 4),
    (1, 8),
    (1, 15),
    (1, 30),
    (1, 60),
    (1, 125),
    (1, 250),
    (1, 500),
    (1, 1000),
    (1, 2000),
    (1, 4000),
    (1, 8000),
];

/// Aperture scale in hundredths of an f-number, 1/3 stop apart (the marked
/// scale cameras themselves enumerate: 1.0, 1.1, 1.2, 1.4 … 45).
const APERTURE_THIRD_STOPS: &[u64] = &[
    100, 110, 125, 140, 160, 180, 200, 220, 250, 280, 320, 350, 400, 450, 500, 560, 630, 710,
    800, 900, 1000, 1100, 1300, 1400, 1600, 1800, 2000, 2200, 2500, 2800, 3200, 3600, 4000,
    4500,
];

fn shutter_seconds((numerator, denominator): &(u32, u32)) -> f64 {
    *numerator as f64 / *denominator as f64
}

/// Nikon shutter codes pack the fraction as (numerator << 16) | denominator.
fn nikon_shutter_seconds(code: i128) -> Option<f64> {
    let value = u32::try_from(u64::try_from(code).ok()?).ok()?;
    let numerator = (value >> 16) as u32;
    let denominator = value & 0xffff;
    if numerator == 0 || denominator == 0 {
        return None;
    }
    Some(numerator as f64 / denominator as f64)
}

fn nikon_shutter_code(numerator: u32, denominator: u32) -> Value {
    Value::Unsigned(((numerator << 16) | denominator) as u64)
}

/// Sample a range-form descriptor into discrete values the descriptor itself
/// accepts (inside bounds and on the step grid). Broken ranges (min > max)
/// yield nothing — the UI then falls back to the current value only.
fn sample_range(code: u16, min: &Value, max: &Value, step: &Value) -> Vec<Value> {
    let (Some(lo), Some(hi)) = (min.number(), max.number()) else {
        return Vec::new();
    };
    let step = step.number().unwrap_or(0);
    if code == 0xd100 {
        // Nikon's packed fraction codes are not monotonic in u32 space, so
        // bounds are validated and filtered in seconds space instead.
        let (Some(lo_s), Some(hi_s)) = (nikon_shutter_seconds(lo), nikon_shutter_seconds(hi))
        else {
            return Vec::new();
        };
        if lo_s > hi_s || lo_s <= 0.0 {
            return Vec::new();
        }
        return SHUTTER_FULL_STOPS
            .iter()
            .filter(|stop| {
                let s = shutter_seconds(stop);
                s >= lo_s && s <= hi_s
            })
            .map(|&(numerator, denominator)| nikon_shutter_code(numerator, denominator))
            .collect();
    }
    if lo > hi || lo < 0 {
        return Vec::new();
    }
    let ladder: Vec<Value> = match code {
        // ISO: doubling ladder anchored at the range floor.
        0x500f if lo > 0 => iso_ladder(lo, hi),
        // Aperture: the marked 1/3-stop scale inside the range.
        0x5007 => APERTURE_THIRD_STOPS
            .iter()
            .filter(|v| lo <= **v as i128 && **v as i128 <= hi)
            .map(|v| Value::Unsigned(*v))
            .collect(),
        // Standard ExposureTime: full stops in 0.1 ms units.
        0x500d => SHUTTER_FULL_STOPS
            .iter()
            .map(|stop| (shutter_seconds(stop) * 10_000.0).round() as u64)
            .filter(|units| lo <= *units as i128 && *units as i128 <= hi)
            .map(Value::Unsigned)
            .collect(),
        _ => Vec::new(),
    };
    ladder
        .into_iter()
        .filter(|value| {
            let n = value.number().unwrap_or(0);
            n >= lo && n <= hi && (step <= 0 || (n - lo) % step == 0)
        })
        .collect()
}

fn iso_ladder(lo: i128, hi: i128) -> Vec<Value> {
    let mut out = Vec::new();
    let mut value = lo as u64;
    while value as i128 <= hi && out.len() < 128 {
        out.push(Value::Unsigned(value));
        match value.checked_mul(2) {
            Some(next) => value = next,
            None => break,
        }
    }
    out
}

pub fn value_label(code: u16, value: &Value) -> String {
    value_label_brand(Brand::Standard, code, value)
}

/// Brand-aware labels. Standard ISO 15740 values are shared by every brand;
/// extension values (>= 0x8000) are only named when the brand's own table
/// defines them (source: libgphoto2 camlibs/ptp2/config.c, whitebalance[]).
/// Unknown extension codes stay as hex instead of guessing a meaning.
pub fn value_label_brand(brand: Brand, code: u16, value: &Value) -> String {
    let Some(number) = value.number() else {
        return value.token();
    };
    match code {
        0x5007 => format!("{:.1}", number as f64 / 100.0),
        0xd100 => {
            if number == 0xffff_ffff {
                return "Bulb".into();
            }
            if number == 0xffff_fffe {
                return "Time".into();
            }
            let numerator = (number as u32) >> 16;
            let denominator = number as u32 & 0xffff;
            if denominator == 0 {
                value.token()
            } else if numerator >= denominator {
                format!("{}s", numerator as f64 / denominator as f64)
            } else {
                format!("{numerator}/{denominator}s")
            }
        }
        0x500d if number > 0 && number < 10000 => format!("1/{:.0}s", 10000.0 / number as f64),
        0x500d if number >= 10000 => format!("{}s", number as f64 / 10000.0),
        0x500f if number == 0 || number == 65535 => "Auto".into(),
        // Nikon WhiteBalanceColorTemperature is an unsigned Kelvin value.
        0xd01e => format!("{number}K"),
        0x5005 => white_balance_label(brand, number, value),
        0x500a => match number {
            1 => "MF",
            2 => "AF",
            3 => "AF Macro",
            0x8010 => "AF-S",
            0x8011 => "AF-C",
            0x8012 => "AF-A",
            0x8013 => "AF-F",
            _ => return value.token(),
        }
        .into(),
        0xd061 => match number {
            0 => "AF-S",
            1 => "AF-C",
            2 => "AF-A",
            3 | 4 => "MF",
            _ => return value.token(),
        }
        .into(),
        _ => value.token(),
    }
}

/// White balance labels. Standard 1-7 are ISO 15740; vendor extension values
/// follow libgphoto2's per-vendor tables (Nikon and Sony reuse numbers such
/// as 0x8012/0x8013 with different meanings, so brand decides).
fn white_balance_label(brand: Brand, number: i128, raw: &Value) -> String {
    if let Some(standard) = match number {
        1 => Some("Manual"),
        2 => Some("Auto"),
        3 => Some("One Push Auto"),
        4 => Some("Daylight"),
        5 => Some("Fluorescent"),
        6 => Some("Tungsten"),
        7 => Some("Flash"),
        _ => None,
    } {
        return standard.into();
    }
    let extension = match (brand, number) {
        (Brand::Nikon, 0x8010) => Some("Cloudy"),
        (Brand::Nikon, 0x8011) => Some("Shade"),
        (Brand::Nikon, 0x8012) => Some("Color Temperature"),
        (Brand::Nikon, 0x8013) => Some("Preset"),
        (Brand::Nikon, 0x8014) => Some("Off"),
        (Brand::Nikon, 0x8015) => Some("Flash"),
        (Brand::Nikon, 0x8016) => Some("Natural Light Auto"),
        (Brand::Sony, 0x8001) => Some("Fluorescent (Warm White)"),
        (Brand::Sony, 0x8002) => Some("Fluorescent (Cold White)"),
        (Brand::Sony, 0x8003) => Some("Fluorescent (Day White)"),
        (Brand::Sony, 0x8004) => Some("Fluorescent (Daylight)"),
        (Brand::Sony, 0x8010) => Some("Cloudy"),
        (Brand::Sony, 0x8011) => Some("Shade"),
        (Brand::Sony, 0x8012) => Some("Choose Color Temperature"),
        (Brand::Sony, 0x8020) => Some("Preset 1"),
        (Brand::Sony, 0x8021) => Some("Preset 2"),
        (Brand::Sony, 0x8022) => Some("Preset 3"),
        (Brand::Sony, 0x8023) => Some("Preset"),
        (Brand::Sony, 0x8030) => Some("Underwater (Auto)"),
        _ => None,
    };
    extension.map(str::to_string).unwrap_or_else(|| {
        // Unknown vendor code: keep the hex code visible rather than guess.
        if number >= 0x8000 {
            format!("0x{number:04X}") // TODO-verify: unlabeled vendor WB code
        } else {
            raw.token()
        }
    })
}

/// Nikon previews contain a binary header before the JPEG. Do not assume a
/// model-specific header length, and never treat a partial frame as a preview.
pub fn preview_jpeg(data: &[u8]) -> Result<Vec<u8>, String> {
    let start = data
        .windows(3)
        .position(|bytes| bytes == [0xff, 0xd8, 0xff])
        .ok_or("取景帧没有有效 JPEG 图像")?;
    let end = data[start..]
        .windows(2)
        .rposition(|bytes| bytes == [0xff, 0xd9])
        .ok_or("取景帧不完整")?
        + start
        + 2;
    Ok(data[start..end].to_vec())
}

/// Nikon LiveView frame header (community-documented layout; no official
/// Nikon documentation exists). Only fields confirmed across implementations
/// are parsed — anything uncertain stays empty/None instead of guessed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveViewHeader {
    /// AF rectangles as (x, y, w, h) normalized to 0..1 of the frame. Modern
    /// bodies report one primary AF frame; a multi-point list is not
    /// confirmed, so at most one region is reported.
    pub af_regions: Vec<(f64, f64, f64, f64)>,
    /// Live view pixel dimensions (offsets 0/2) when the header carries
    /// plausible non-zero values.
    pub size: Option<(u32, u32)>,
    /// Full-image pixel dimensions (offsets 4/6) — the coordinate space
    /// ChangeAfArea and the reported AF frame live in (digiCamControl
    /// SetFocusPos scales clicks by these before 0x9205).
    pub image_size: Option<(u32, u32)>,
}

/// Header field layout (offsets in the binary header before the JPEG SOI;
/// sources: digiCamControl NikonBase.GetAdditionalLiveViewData and the
/// libgphoto2 liveview FIXME notes; header length varies by body —
/// 64/128/384 bytes are known, larger offsets are accepted defensively):
/// - 0..2 i16 live view width; 2..4 i16 live view height
/// - 4..6 i16 full image width; 6..8 i16 full image height
/// - 16..18 i16 AF frame width; 18..20 i16 AF frame height
/// - 20..22 i16 AF frame x; 22..24 i16 AF frame y (CENTER coordinates:
///   digiCamControl draws the frame centered on them)
/// - 29 u8 rotation; 40 u8 focus status; 60 u8 movie recording (not exposed)
pub fn live_view_header(data: &[u8]) -> Result<LiveViewHeader, String> {
    const MIN_HEADER: usize = 64;
    let jpeg = data
        .windows(3)
        .position(|bytes| bytes == [0xff, 0xd8, 0xff])
        .ok_or("取景帧没有 LiveView 头部")?;
    if jpeg < MIN_HEADER {
        return Err(format!("LiveView 头部过短（JPEG 偏移 {jpeg}）"));
    }
    let field = |offset: usize| i16::from_le_bytes([data[offset], data[offset + 1]]);
    let width = field(0).max(0) as u32;
    let height = field(2).max(0) as u32;
    let image_width = field(4).max(0) as u32;
    let image_height = field(6).max(0) as u32;
    let dims = |w: u32, h: u32| (w > 0 && h > 0).then_some((w, h));
    let size = dims(width, height);
    let image_size = dims(image_width, image_height);
    // AF frame coordinates are in the full-image space; normalize against it
    // (fall back to live view dims when the body omits 4/6).
    let (norm_w, norm_h) = image_size.or(size).unwrap_or((0, 0));
    let af_regions = af_region(&field, norm_w, norm_h);
    Ok(LiveViewHeader {
        af_regions,
        size,
        image_size,
    })
}

/// Normalize the documented AF frame; rectangles that do not fit inside the
/// frame are dropped rather than clamped (crop/orientation is unverified).
fn af_region(field: &impl Fn(usize) -> i16, width: u32, height: u32) -> Vec<(f64, f64, f64, f64)> {
    let (x, y, w, h) = (field(20), field(22), field(16), field(18));
    if width == 0 || height == 0 || x < 0 || y < 0 || w <= 0 || h <= 0 {
        return Vec::new();
    }
    let region = (
        x as f64 / width as f64,
        y as f64 / height as f64,
        w as f64 / width as f64,
        h as f64 / height as f64,
    );
    let eps = 1e-9;
    let inside = region.0 >= 0.0
        && region.1 >= 0.0
        && region.0 + region.2 <= 1.0 + eps
        && region.1 + region.3 <= 1.0 + eps;
    if inside {
        vec![region]
    } else {
        Vec::new()
    }
}

/// Drive Nikon autofocus (0x90C1). Plan contract: parameter data = 1.
/// TODO-verify on hardware: upstream libgphoto2 ptp.h documents 0x90C1 as
/// "no params, no data" (`ptp_nikon_afdrive` sends zero parameters); if a
/// body rejects the parameter, switch to an empty parameter list here.
pub fn nikon_autofocus(transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
    let response = transport.command(NIKON_AUTOFOCUS, &[1])?;
    require_ok(response, NIKON_AUTOFOCUS)
}

/// Map a normalized click position (0..1, origin top-left) to absolute pixel
/// coordinates for Nikon ChangeAfArea (0x9205, two u32 params, no data —
/// libgphoto2 ptp.h:604 / ptp.c `ptp_nikon_changeafarea`).
///
/// Coordinate space: the full-image dimensions from the live view header
/// (offsets 4/6) — digiCamControl `SetFocusPos` scales display clicks by
/// exactly these before calling `Focus(x, y)`; the reported AF frame center
/// (offsets 20/22) lives in the same space. Falls back to live view dims
/// (offsets 0/2) when a body omits 4/6; neither present → Err.
pub fn click_focus_offset(x: f64, y: f64, header: &LiveViewHeader) -> Result<(u32, u32), String> {
    let (width, height) = header
        .image_size
        .or(header.size)
        .ok_or("取景头缺少画面尺寸，无法换算对焦坐标")?;
    let to_pixel = |normalized: f64, dimension: u32| {
        // 输入防御性收敛到 0..1，结果夹到 dimension-1（右/下边缘点击不出界）
        let clamped = normalized.clamp(0.0, 1.0);
        ((clamped * dimension as f64).round() as u32).min(dimension.saturating_sub(1))
    };
    Ok((to_pixel(x, width), to_pixel(y, height)))
}

/// Select the Nikon live view AF area (0x9205). Works only while live view
/// runs (PTP_RC_NIKON_NotLiveView 0xA00B otherwise — libgphoto2 config.c).
/// The camera may answer DeviceBusy (0x2019) while a previous AF run settles;
/// retry briefly (libgphoto2's changeafarea had the same wait loop).
pub fn nikon_change_af_area(
    transport: &mut (impl PtpTransport + ?Sized),
    x: u32,
    y: u32,
) -> Result<(), String> {
    for attempt in 0..5 {
        let response = transport.command(NIKON_FOCUS_AREA, &[x, y])?;
        if response != 0x2019 {
            return require_ok(response, NIKON_FOCUS_AREA);
        }
        if attempt < 4 {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
    Err("相机对焦区域忙，稍后再试".into())
}

/// Native transport-independent Nikon preview lifecycle. Stop only LiveView
/// started by this session; status is checked on startup/recovery, not each frame.
#[derive(Default)]
pub struct NikonLiveView {
    ready: bool,
    owned: bool,
    /// 最近一帧解析出的头部（点击对焦的坐标换算依据；None = 还没成功取过帧）。
    last_header: Option<LiveViewHeader>,
}
impl NikonLiveView {
    pub fn frame(&mut self, transport: &mut (impl PtpTransport + ?Sized)) -> Result<Vec<u8>, String> {
        if !self.ready {
            let running = transport
                .read(GET_PROP_VALUE, &[0xd1a2], 1024)
                .ok()
                .is_some_and(|(response, data)| response == 0x2001 && data.first() == Some(&1));
            if !running {
                require_ok(transport.command(NIKON_LIVE_START, &[])?, NIKON_LIVE_START)?;
                self.owned = true;
            }
            self.ready = true;
        }
        let (response, data) = transport.read(NIKON_LIVE_IMAGE, &[], 16 * 1024 * 1024)?;
        if response != 0x2001 {
            if response != 0x2019 {
                self.ready = false;
            }
            require_ok(response, NIKON_LIVE_IMAGE)?;
        }
        self.last_header = live_view_header(&data).ok();
        preview_jpeg(&data)
    }
    /// 最近一帧的头部（画面尺寸/AF 框）；没成功取过帧为 None。
    pub fn last_header(&self) -> Option<&LiveViewHeader> {
        self.last_header.as_ref()
    }
    pub fn stop(&mut self, transport: &mut (impl PtpTransport + ?Sized)) -> Result<(), String> {
        if self.owned {
            require_ok(transport.command(NIKON_LIVE_END, &[])?, NIKON_LIVE_END)?;
            self.owned = false;
        }
        self.ready = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn enum_u16(code: u16) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend(code.to_le_bytes());
        bytes.extend(4u16.to_le_bytes());
        bytes.push(1);
        bytes.extend(100u16.to_le_bytes());
        bytes.extend(200u16.to_le_bytes());
        bytes.push(2);
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(100u16.to_le_bytes());
        bytes.extend(200u16.to_le_bytes());
        bytes
    }
    #[test]
    fn camera_options_define_what_can_be_written() {
        let property = Property::parse(&enum_u16(0x500f)).unwrap();
        assert_eq!(property.encode("100").unwrap(), 100u16.to_le_bytes());
        assert!(property.encode("300").is_err());
        assert_eq!(
            property.setting(CORE_PROPERTIES[3], Brand::Standard).current,
            "200"
        );
        let mut readonly = property;
        readonly.writable = false;
        assert!(readonly.encode("100").is_err());
    }

    fn range_u16(code: u16, min: u64, max: u64, step: u64, current: u16) -> Property {
        let mut bytes = Vec::new();
        bytes.extend(code.to_le_bytes());
        bytes.extend(4u16.to_le_bytes());
        bytes.push(1);
        bytes.extend(0u16.to_le_bytes()); // factory default
        bytes.extend(current.to_le_bytes());
        bytes.push(1); // Range form
        for value in [min, max, step] {
            bytes.extend((value as u16).to_le_bytes());
        }
        Property::parse(&bytes).unwrap()
    }

    #[test]
    fn iso_range_samples_a_doubling_ladder() {
        let property = range_u16(0x500f, 100, 1600, 0, 100);
        let setting = property.setting(CORE_PROPERTIES[3], Brand::Standard);
        assert_eq!(setting.kind, SettingKind::Choice);
        let values: Vec<&str> = setting.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, vec!["100", "200", "400", "800", "1600"]);
        assert_eq!(setting.min, Some(100.0));
        assert_eq!(setting.max, Some(1600.0));
        assert_eq!(setting.step, None);
    }

    #[test]
    fn aperture_range_samples_marked_third_stops() {
        let property = range_u16(0x5007, 280, 800, 0, 350);
        let setting = property.setting(CORE_PROPERTIES[2], Brand::Standard);
        let values: Vec<&str> = setting.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, vec!["280", "320", "350", "400", "450", "500", "560", "630", "710", "800"]);
        let labels: Vec<&str> = setting.options.iter().map(|o| o.label.as_str()).collect();
        assert_eq!(labels[0], "2.8");
        assert_eq!(labels.last().copied().unwrap(), "8.0");
    }

    #[test]
    fn shutter_ranges_sample_full_stops_for_both_encodings() {
        let standard = range_u16(0x500d, 10, 10_000, 0, 80);
        let setting = standard.setting(CORE_PROPERTIES[1], Brand::Standard);
        // 10..10000 units = 1/1000s..1s: the ladder keeps table order (slow
        // exposure first): 1s, 1/2s … 1/1000s.
        assert_eq!(
            setting.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
            vec!["1s", "1/2s", "1/4s", "1/8s", "1/15s", "1/30s", "1/60s", "1/125s", "1/250s", "1/500s", "1/1000s"]
        );

        let mut bytes = Vec::new();
        bytes.extend(0xd100u16.to_le_bytes());
        bytes.extend(6u16.to_le_bytes()); // UINT32 packed fraction
        bytes.push(1);
        bytes.extend(0u32.to_le_bytes());
        let current = (1u32 << 16) | 125; // 1/125s
        bytes.extend(current.to_le_bytes());
        bytes.push(1);
        let lo = (1u32 << 16) | 1000; // 1/1000s
        let hi = (1u32 << 16) | 1; // 1s
        for value in [lo, hi, 0] {
            bytes.extend(value.to_le_bytes());
        }
        let nikon = Property::parse(&bytes).unwrap();
        let setting = nikon.setting(CORE_PROPERTIES[0], Brand::Nikon);
        assert_eq!(
            setting.options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(),
            vec!["1s", "1/2s", "1/4s", "1/8s", "1/15s", "1/30s", "1/60s", "1/125s", "1/250s", "1/500s", "1/1000s"]
        );
    }

    #[test]
    fn broken_ranges_fall_back_to_current_only() {
        // min > max: unusable descriptor.
        let property = range_u16(0x500f, 1600, 100, 0, 100);
        let setting = property.setting(CORE_PROPERTIES[3], Brand::Standard);
        assert_eq!(setting.options.len(), 1);
        assert_eq!(setting.options[0].value, "100");
        assert_eq!(setting.min, None);
        assert_eq!(setting.max, None);
    }

    #[test]
    fn range_samples_respect_descriptor_step_grid() {
        // ISO range with a 50-unit grid: 100,200,… are aligned, odd ladders
        // like 150/300 must be filtered out.
        let property = range_u16(0x500f, 100, 400, 50, 100);
        let setting = property.setting(CORE_PROPERTIES[3], Brand::Standard);
        let values: Vec<&str> = setting.options.iter().map(|o| o.value.as_str()).collect();
        assert_eq!(values, vec!["100", "200", "400"]);
    }

    #[test]
    fn unknown_range_properties_stay_sliders_without_options() {
        // 0x5005 is not one of the sampled codes: continuous slider shape.
        let property = range_u16(0x5005, 2500, 6500, 50, 5000);
        let setting = property.setting(CORE_PROPERTIES[4], Brand::Standard);
        assert_eq!(setting.kind, SettingKind::Range);
        assert_eq!(setting.options.len(), 1); // current only
        assert_eq!(setting.options[0].value, "5000");
        assert_eq!(setting.min, Some(2500.0));
        assert_eq!(setting.max, Some(6500.0));
        assert_eq!(setting.step, Some(50.0));
    }

    #[test]
    fn brand_detection_uses_manufacturer_only() {
        assert_eq!(detect_brand("NIKON CORPORATION", "Z 5II", &[]), Brand::Nikon);
        assert_eq!(detect_brand("Sony Corp", "ILCE-7M4", &[]), Brand::Sony);
        // No vendor statement → Standard, even when Nikon-style codes appear:
        // numeric vendor codes must never imply the vendor.
        assert_eq!(detect_brand("", "D850", &[0x9201, 0x9203]), Brand::Standard);
        assert_eq!(detect_brand("FUJIFILM", "X-T5", &[]), Brand::Standard);
    }

    #[test]
    fn white_balance_extension_labels_are_brand_specific() {
        let value = |n: i128| Value::Unsigned(n as u64);
        // Shared standard values.
        assert_eq!(
            value_label_brand(Brand::Nikon, 0x5005, &value(4)),
            "Daylight"
        );
        // Same number, different meaning per vendor.
        assert_eq!(
            value_label_brand(Brand::Nikon, 0x5005, &value(0x8012)),
            "Color Temperature"
        );
        assert_eq!(
            value_label_brand(Brand::Sony, 0x5005, &value(0x8012)),
            "Choose Color Temperature"
        );
        assert_eq!(
            value_label_brand(Brand::Sony, 0x5005, &value(0x8020)),
            "Preset 1"
        );
        assert_eq!(
            value_label_brand(Brand::Nikon, 0x5005, &value(0x8016)),
            "Natural Light Auto"
        );
        // Unknown vendor code: hex, not a guess. Standard never labels
        // vendor extensions even when the number matches a known table.
        assert_eq!(
            value_label_brand(Brand::Standard, 0x5005, &value(0x8010)),
            "0x8010"
        );
        assert_eq!(
            value_label_brand(Brand::Nikon, 0x5005, &value(0x8017)),
            "0x8017"
        );
        // Kelvin-typed value (Nikon WhiteBalanceColorTemperature).
        assert_eq!(value_label(0xd01e, &value(5200)), "5200K");
    }

    #[test]
    fn live_view_header_parses_documented_fields_only() {
        let mut frame = vec![0u8; 384];
        frame[0..2].copy_from_slice(&640i16.to_le_bytes());
        frame[2..4].copy_from_slice(&480i16.to_le_bytes());
        frame[4..6].copy_from_slice(&4032i16.to_le_bytes()); // full image w
        frame[6..8].copy_from_slice(&3024i16.to_le_bytes()); // full image h
        frame[16..18].copy_from_slice(&80i16.to_le_bytes()); // AF frame w
        frame[18..20].copy_from_slice(&60i16.to_le_bytes()); // AF frame h
        frame[20..22].copy_from_slice(&280i16.to_le_bytes()); // AF frame x
        frame[22..24].copy_from_slice(&210i16.to_le_bytes()); // AF frame y
        frame.extend_from_slice(&[0xff, 0xd8, 0xff, 0x00, 0xff, 0xd9]);

        let header = live_view_header(&frame).unwrap();
        assert_eq!(header.size, Some((640, 480)));
        assert_eq!(header.image_size, Some((4032, 3024)));
        // AF 框坐标在 4/6 全图空间（digiCamControl DrawFocusPoint 按
        // bitmap/ImageWidth 缩放绘制）→ 归一化也按全图尺寸。
        assert!((header.af_regions[0].0 - 280.0 / 4032.0).abs() < 1e-9);
        assert!((header.af_regions[0].1 - 210.0 / 3024.0).abs() < 1e-9);
        assert!((header.af_regions[0].2 - 80.0 / 4032.0).abs() < 1e-9);
        assert!((header.af_regions[0].3 - 60.0 / 3024.0).abs() < 1e-9);

        // 4/6 缺省（老机身头布局）→ 归一化回退 0/2 取景尺寸。
        let mut live_only = frame.clone();
        live_only[4..8].fill(0);
        let header = live_view_header(&live_only).unwrap();
        assert_eq!(header.image_size, None);
        assert_eq!(header.size, Some((640, 480)));
        assert!((header.af_regions[0].0 - 280.0 / 640.0).abs() < 1e-9);

        // Zeroed AF fields (no AF data) → no region, size still reported.
        let mut no_af = frame.clone();
        for offset in 16..24 {
            no_af[offset] = 0;
        }
        let header = live_view_header(&no_af).unwrap();
        assert!(header.af_regions.is_empty());
        assert_eq!(header.size, Some((640, 480)));

        // Region outside the frame is dropped instead of clamped.
        let mut outside = frame.clone();
        outside[20..22].copy_from_slice(&4000i16.to_le_bytes()); // 4000+80 > 4032
        let header = live_view_header(&outside).unwrap();
        assert!(header.af_regions.is_empty());

        // No JPEG / header shorter than the 64-byte minimum → error.
        assert!(live_view_header(&frame[..64]).is_err());
        let mut short = vec![0xff, 0xd8, 0xff, 0xff, 0xd9];
        short.splice(0..0, [0u8; 16]); // SOI at offset 16 < 64
        assert!(live_view_header(&short).is_err());
    }

    #[test]
    fn click_focus_offset_maps_to_full_image_pixel_space() {
        // digiCamControl SetFocusPos 证据：点击 × 头部 4/6 全图尺寸 = 0x9205 参数。
        let header = LiveViewHeader {
            af_regions: Vec::new(),
            size: Some((640, 480)),
            image_size: Some((4032, 3024)),
        };
        assert_eq!(click_focus_offset(0.25, 0.75, &header).unwrap(), (1008, 2268));
        // 全图尺寸缺省 → 回退取景尺寸（0/2）。
        let fallback = LiveViewHeader {
            af_regions: Vec::new(),
            size: Some((640, 480)),
            image_size: None,
        };
        assert_eq!(click_focus_offset(0.5, 0.5, &fallback).unwrap(), (320, 240));
        // 越界输入收敛；右/下边缘不出界。
        assert_eq!(click_focus_offset(1.2, -0.1, &header).unwrap(), (4031, 0));
        assert_eq!(click_focus_offset(1.0, 1.0, &fallback).unwrap(), (639, 479));
        // 两个尺寸都没有 → 明确报错（不猜测中心）。
        assert!(click_focus_offset(0.5, 0.5, &LiveViewHeader::default()).is_err());
    }

    #[test]
    fn nikon_change_af_area_sends_pixel_params_and_retries_busy() {
        struct AreaTransport {
            responses: Vec<u32>,
            commands: Vec<(u16, Vec<u32>)>,
        }
        impl PtpTransport for AreaTransport {
            fn read(&mut self, _: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
                unreachable!()
            }
            fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
                unreachable!()
            }
            fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
                self.commands.push((operation, params.to_vec()));
                Ok(self.responses.pop().unwrap_or(0x2001))
            }
        }
        // 两参数 = 像素坐标（u32，无数据相），次序 x, y。
        let mut transport = AreaTransport {
            responses: vec![0x2001],
            commands: Vec::new(),
        };
        nikon_change_af_area(&mut transport, 1008, 2268).unwrap();
        assert_eq!(transport.commands, vec![(NIKON_FOCUS_AREA, vec![1008, 2268])]);

        // DeviceBusy(0x2019) → 重试后成功。
        let mut transport = AreaTransport {
            responses: vec![0x2001, 0x2019],
            commands: Vec::new(),
        };
        nikon_change_af_area(&mut transport, 1, 2).unwrap();
        assert_eq!(transport.commands.len(), 2);

        // 持续忙 → 明确错误，不发其他指令。
        let mut transport = AreaTransport {
            responses: vec![0x2019; 5],
            commands: Vec::new(),
        };
        assert!(nikon_change_af_area(&mut transport, 1, 2).is_err());
        assert_eq!(transport.commands.len(), 5);

        // 其他拒绝码原样报错（如 NotLiveView 0xA00B）。
        let mut transport = AreaTransport {
            responses: vec![0xa00b],
            commands: Vec::new(),
        };
        assert!(nikon_change_af_area(&mut transport, 1, 2).is_err());
        assert_eq!(transport.commands.len(), 1);
    }

    #[test]
    fn nikon_autofocus_sends_the_planned_parameter() {
        struct AfTransport {
            response: u32,
            commands: Vec<(u16, Vec<u32>)>,
        }
        impl PtpTransport for AfTransport {
            fn read(&mut self, _: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
                unreachable!()
            }
            fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
                unreachable!()
            }
            fn command(&mut self, operation: u16, params: &[u32]) -> Result<u32, String> {
                self.commands.push((operation, params.to_vec()));
                Ok(self.response)
            }
        }
        let mut transport = AfTransport {
            response: 0x2001,
            commands: Vec::new(),
        };
        nikon_autofocus(&mut transport).unwrap();
        assert_eq!(transport.commands, vec![(NIKON_AUTOFOCUS, vec![1])]);

        // Camera refusal surfaces the response code.
        transport.response = 0xa004;
        assert!(nikon_autofocus(&mut transport).is_err());
    }
    #[test]
    fn truncation_and_invalid_types_do_not_panic() {
        let bytes = enum_u16(0x500f);
        for len in 0..bytes.len() {
            assert!(Property::parse(&bytes[..len]).is_err());
        }
        let mut invalid = bytes;
        invalid[2..4].copy_from_slice(&0x4004u16.to_le_bytes());
        assert!(Property::parse(&invalid).is_err());
    }
    #[test]
    fn nikon_shutter_labels_preserve_camera_fractions() {
        assert_eq!(value_label(0xd100, &Value::Unsigned(0x0001_0032)), "1/50s");
        assert_eq!(value_label(0xd100, &Value::Unsigned(0x0002_0001)), "2s");
        assert_eq!(value_label(0x5007, &Value::Unsigned(280)), "2.8");
    }
    #[test]
    fn preview_extracts_only_complete_jpeg() {
        assert_eq!(
            preview_jpeg(&[0, 1, 0xff, 0xd8, 0xff, 2, 0xff, 0xd9, 0]).unwrap(),
            vec![0xff, 0xd8, 0xff, 2, 0xff, 0xd9]
        );
        assert!(preview_jpeg(&[0xff, 0xd8, 0xff, 2]).is_err());
    }

    struct PreviewTransport {
        originally_running: bool,
        status_reads: usize,
        commands: Vec<u16>,
        /// 0x9203 返回载荷：None = 裸 JPEG（无可解析头部）。
        framed: bool,
    }
    impl PtpTransport for PreviewTransport {
        fn read(&mut self, operation: u16, _: &[u32], _: usize) -> Result<(u32, Vec<u8>), String> {
            if operation == GET_PROP_VALUE {
                self.status_reads += 1;
                return Ok((0x2001, vec![u8::from(self.originally_running)]));
            }
            assert_eq!(operation, NIKON_LIVE_IMAGE);
            if !self.framed {
                return Ok((0x2001, vec![0xff, 0xd8, 0xff, 2, 0xff, 0xd9]));
            }
            let mut frame = vec![0u8; 64];
            frame[0..2].copy_from_slice(&640i16.to_le_bytes());
            frame[2..4].copy_from_slice(&480i16.to_le_bytes());
            frame.extend_from_slice(&[0xff, 0xd8, 0xff, 2, 0xff, 0xd9]);
            Ok((0x2001, frame))
        }
        fn write(&mut self, _: u16, _: &[u32], _: &[u8]) -> Result<u32, String> {
            unreachable!()
        }
        fn command(&mut self, operation: u16, _: &[u32]) -> Result<u32, String> {
            self.commands.push(operation);
            Ok(0x2001)
        }
    }
    #[test]
    fn live_view_preserves_existing_state_and_does_not_read_settings_each_frame() {
        for running in [false, true] {
            let mut transport = PreviewTransport {
                originally_running: running,
                status_reads: 0,
                commands: Vec::new(),
                framed: false,
            };
            let mut view = NikonLiveView::default();
            for _ in 0..20 {
                assert!(!view.frame(&mut transport).unwrap().is_empty());
            }
            view.stop(&mut transport).unwrap();
            view.stop(&mut transport).unwrap();
            assert_eq!(transport.status_reads, 1);
            assert_eq!(
                transport.commands,
                if running {
                    vec![]
                } else {
                    vec![NIKON_LIVE_START, NIKON_LIVE_END]
                }
            );
        }
    }

    #[test]
    fn live_view_caches_the_latest_frame_header_for_focus_mapping() {
        // 有头部 → 每帧更新缓存（点击对焦按最新一帧坐标系换算）。
        let mut transport = PreviewTransport {
            originally_running: true,
            status_reads: 0,
            commands: Vec::new(),
            framed: true,
        };
        let mut view = NikonLiveView::default();
        assert!(view.last_header().is_none(), "还没取过帧时无头部");
        view.frame(&mut transport).unwrap();
        let header = view.last_header().expect("帧后应有头部");
        assert_eq!(header.size, Some((640, 480)));

        // 裸 JPEG（无头部布局的载荷）→ 头部保持 None，取景不失败。
        let mut bare = PreviewTransport {
            originally_running: true,
            status_reads: 0,
            commands: Vec::new(),
            framed: false,
        };
        let mut view = NikonLiveView::default();
        view.frame(&mut bare).unwrap();
        assert!(view.last_header().is_none());
    }
}
