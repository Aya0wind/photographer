//! M1 EXIF-lite（spec §5.3 / T6），M3.5 扩展拍摄参数：captured_at / 相机 /
//! 宽高 / ISO / 光圈 / 快门 / 焦距 / 镜头。容器解析交给 kamadak-exif
//! （JPEG APP1、TIFF 系 II*/MM*——即多数 NEF/ARW/CR2/DNG/ORF——另有
//! HEIF/PNG/WebP 附赠支持）；宽高优先 JPEG SOF0..SOF3 帧段（含 RAW 内嵌
//! 预览的 SOI 扫描），TIFF 形态回退 IFD0 ImageWidth/ImageLength。
//! 任何失败（垃圾/空/截断）静默返回全 None——绝不 Err、绝不 panic。

use std::io::Cursor;

use chrono::{DateTime, NaiveDateTime, Utc};

/// EXIF-lite 提取结果：全部字段可缺失。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MetaLite {
    /// DateTimeOriginal(0x9003) 优先，回退 DateTime(0x0132)，均为 UTC。
    pub captured_at: Option<DateTime<Utc>>,
    /// EXIF Make + Model（多余空白折叠）。
    pub camera: Option<String>,
    /// 像素宽（SOF 或 TIFF ImageWidth 0x0100）。
    pub width: Option<u32>,
    /// 像素高（SOF 或 TIFF ImageLength 0x0101）。
    pub height: Option<u32>,
    /// 感光度 ISO(0x8827)。
    pub iso: Option<u32>,
    /// 光圈 FNumber(0x829D)，展示态字符串（如 "2.8"、"4"）。
    pub f_number: Option<String>,
    /// 快门 ExposureTime(0x829A)，展示态字符串（如 "1/250"、"0.4"、"2"）。
    pub exposure_time: Option<String>,
    /// 焦距 FocalLength(0x920A)，展示态字符串（mm，如 "85"）。
    pub focal_length: Option<String>,
    /// 镜头型号 LensModel(0xA434)。
    pub lens: Option<String>,
    /// 深提取字段（0008 列，gen-2 回填链）。
    pub deep: DeepExif,
}

/// EXIF 深提取字段（assets 0008 列同构；全部可缺失）。
/// token 映射（写库契约，筛选/展示两侧共用）：
/// - flash：EXIF Flash(0x9209) bit0（是否闪光）→ "fired" / "no_flash"
///   （强制开/自动未亮等细分折叠，筛选用 `LIKE '%fired%'` / `'no_flash%'`）
/// - metering_mode：average/center_weighted/spot/multi_spot/pattern/partial/other
/// - white_balance：auto/manual
/// - exposure_program：manual/aperture_priority/shutter_priority/creative/
///   action/portrait/landscape
/// - orientation：EXIF 1-8（1=正常；5-8 含 90° 旋转）
/// - gps：十进制度（deg + min/60 + sec/3600；南纬/西经取负）
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DeepExif {
    pub orientation: Option<i64>,
    pub flash: Option<String>,
    pub metering_mode: Option<String>,
    pub white_balance: Option<String>,
    pub exposure_program: Option<String>,
    pub software: Option<String>,
    pub artist: Option<String>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

/// 从文件头部字节（建议 ≥1MB）解析 EXIF-lite。永不返回 Err，也绝不 panic。
pub fn parse(head: &[u8]) -> MetaLite {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(head))
        .ok();
    let dims = jpeg_sof_dimensions(head).or_else(|| tiff_ifd0_dimensions(head));
    let Some(exif) = exif else {
        // EXIF 容器失败（纯 JPEG 无 APP1 / 损坏）：宽高仍可从 SOF 提取
        return MetaLite {
            width: dims.map(|d| d.0),
            height: dims.map(|d| d.1),
            ..MetaLite::default()
        };
    };
    // 宽高优先级：EXIF 像素维度（0xA002/0xA003，拍摄设备的真实输出尺寸）
    // > SOF/TIFF——ARW 的 IFD0 ImageWidth 是内嵌缩略图尺寸（真机 61MP ARW
    // 被写成 160×120，2026-09-20）
    let dims = exif_pixel_dimensions(&exif).or(dims);
    MetaLite {
        captured_at: datetime_field(&exif, exif::Tag::DateTimeOriginal)
            .or_else(|| datetime_field(&exif, exif::Tag::DateTime)),
        camera: camera_string(&exif),
        width: dims.map(|d| d.0),
        height: dims.map(|d| d.1),
        iso: uint_field(&exif, 0x8827).map(|v| v as u32),
        f_number: rational_field(&exif, 0x829D).and_then(format_f_number),
        exposure_time: rational_field(&exif, 0x829A).and_then(format_exposure_time),
        focal_length: rational_field(&exif, 0x920A).and_then(format_focal_length),
        lens: ascii_value(&exif, exif::Tag::LensModel).filter(|s| !s.is_empty()),
        deep: deep_exif(&exif),
    }
}

/// 深提取字段集合（0008 列）：方向 / 闪光 / 测光 / 白平衡 / 曝光程序 /
/// 软件 / 作者 / GPS。JPEG（APP1）与 RAW（TIFF IFD0 + GPS IFD）都经
/// kamadak-exif 统一解析，字段在 In::PRIMARY 平面查询。
fn deep_exif(exif: &exif::Exif) -> DeepExif {
    DeepExif {
        orientation: orientation_field(exif),
        flash: flash_token(exif),
        metering_mode: token_by_code(
            uint_field(exif, 0x9207).map(u32::from),
            &[
                (1, "average"),
                (2, "center_weighted"),
                (3, "spot"),
                (4, "multi_spot"),
                (5, "pattern"),
                (6, "partial"),
                (255, "other"),
            ],
        ),
        white_balance: token_by_code(
            uint_field(exif, 0xA403).map(u32::from),
            &[(0, "auto"), (1, "manual")],
        ),
        exposure_program: token_by_code(
            uint_field(exif, 0x8822).map(u32::from),
            &[
                (1, "manual"),
                (2, "aperture_priority"),
                (3, "shutter_priority"),
                (4, "creative"),
                (5, "action"),
                (6, "portrait"),
                (7, "landscape"),
            ],
        ),
        software: ascii_value(exif, exif::Tag::Software).filter(|s| !s.is_empty()),
        artist: ascii_value(exif, exif::Tag::Artist).filter(|s| !s.is_empty()),
        gps_lat: gps_coordinate(
            exif,
            exif::Tag::GPSLatitude,
            exif::Tag::GPSLatitudeRef,
            false,
        ),
        gps_lon: gps_coordinate(
            exif,
            exif::Tag::GPSLongitude,
            exif::Tag::GPSLongitudeRef,
            true,
        ),
    }
}

/// 拍摄方向 Orientation(0x0112，IFD0/TIFF 上下文)，EXIF 1-8；缺失/非法 None。
fn orientation_field(exif: &exif::Exif) -> Option<i64> {
    let value = &exif
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
        .value;
    let raw = match value {
        exif::Value::Short(list) => list.first().copied().map(u32::from),
        exif::Value::Long(list) => list.first().copied(),
        _ => None,
    }?;
    (1..=8).contains(&raw).then_some(raw as i64)
}

/// 闪光 token：Flash(0x9209) bit0 = 是否闪光（任一 fired 复合值都算
/// "fired"；0x00/强制关(0x10)/自动未亮(0x18) 等 → "no_flash"）。
fn flash_token(exif: &exif::Exif) -> Option<String> {
    let value = &exif
        .get_field(exif::Tag(exif::Context::Exif, 0x9209), exif::In::PRIMARY)?
        .value;
    let raw = match value {
        exif::Value::Short(list) => list.first().copied().map(u32::from),
        exif::Value::Long(list) => list.first().copied(),
        _ => None,
    }?;
    Some(if raw & 1 == 1 { "fired" } else { "no_flash" }.to_string())
}

/// 数值枚举 → token（0 = 未定义 → None；未知码 → None）。
fn token_by_code(code: Option<u32>, map: &[(u32, &str)]) -> Option<String> {
    let code = code?;
    map.iter()
        .find(|(c, _)| *c == code)
        .map(|(_, token)| token.to_string())
}

/// GPS 坐标（十进制度）：GPSLatitude/GPSLongitude 1-3 个 RATIONAL
///（deg [min] [sec]）+ Ref（N/S 或 E/W）。值不完整/缺 Ref → None；
/// 南纬（S）/西经（W）取负。
fn gps_coordinate(
    exif: &exif::Exif,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
    _is_lon: bool,
) -> Option<f64> {
    let dms = match &exif.get_field(coord_tag, exif::In::PRIMARY)?.value {
        exif::Value::Rational(list) if !list.is_empty() && list.len() <= 3 => {
            list.iter().map(|r| r.to_f64()).collect::<Vec<f64>>()
        }
        _ => return None,
    };
    let reference = ascii_value(exif, ref_tag)?;
    let negative = match reference.trim_start_matches('\u{0}') {
        "S" | "W" => true,
        "N" | "E" => false,
        _ => return None,
    };
    let mut degrees = dms.first().copied()?;
    if let Some(minutes) = dms.get(1) {
        degrees += minutes / 60.0;
    }
    if let Some(seconds) = dms.get(2) {
        degrees += seconds / 3600.0;
    }
    Some(if negative { -degrees } else { degrees })
}

/// 读取 ASCII 字段并去除首尾空白（kamadak-exif 解析时已剥离尾部 NUL）。
/// 子 IFD（Exif）字段沿用父 IFD 编号，故统一用 `In::PRIMARY` 查询。
fn ascii_value(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    match &exif.get_field(tag, exif::In::PRIMARY)?.value {
        exif::Value::Ascii(list) => list
            .first()
            .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string()),
        _ => None,
    }
}

/// EXIF 像素维度 PixelXDimension(0xA002)/PixelYDimension(0xA003)。
fn exif_pixel_dimensions(exif: &exif::Exif) -> Option<(u32, u32)> {
    let w = uint_field(exif, 0xA002)?;
    let h = uint_field(exif, 0xA003)?;
    (w > 0 && h > 0).then_some((w as u32, h as u32))
}

/// EXIF 日期格式 `"%Y:%m:%d %H:%M:%S"`，不含时区 → 按待定时间视作 UTC。
fn datetime_field(exif: &exif::Exif, tag: exif::Tag) -> Option<DateTime<Utc>> {
    let raw = ascii_value(exif, tag)?;
    NaiveDateTime::parse_from_str(&raw, "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
}

/// 拍摄方向 Orientation(0x0112，IFD0/TIFF 上下文)，EXIF 1-8；缺失/非法 None。
/// RAW（TIFF 容器）与 JPEG（APP1）都经 kamadak 解析——真机的方向语义在
/// 容器 IFD0 上，内嵌预览 JPEG 自带的 EXIF 不代表拍摄方向。
#[doc(hidden)]
#[allow(dead_code)] // exif_lite_test 单文件编译场景下无调用方
pub fn parse_orientation(head: &[u8]) -> Option<u32> {
    let exif = exif::Reader::new()
        .read_from_container(&mut Cursor::new(head))
        .ok()?;
    let value = &exif
        .get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
        .value;
    match value {
        exif::Value::Short(list) => list.first().copied().map(u32::from),
        exif::Value::Long(list) => list.first().copied(),
        _ => None,
    }
}

/// 无符号整数字段（SHORT/LONG 均可；相机按 tag 指定类型存储）。
fn uint_field(exif: &exif::Exif, tag: u16) -> Option<u16> {
    let value = &exif
        .get_field(exif::Tag(exif::Context::Exif, tag), exif::In::PRIMARY)?
        .value;
    match value {
        exif::Value::Short(list) => list.first().copied(),
        exif::Value::Long(list) => list.first().map(|v| *v as u16),
        exif::Value::Byte(list) => list.first().map(|v| u16::from(*v)),
        _ => None,
    }
}

/// 无符号有理数字段（RATIONAL，num/den）。
fn rational_field(exif: &exif::Exif, tag: u16) -> Option<(u32, u32)> {
    match &exif
        .get_field(exif::Tag(exif::Context::Exif, tag), exif::In::PRIMARY)?
        .value
    {
        exif::Value::Rational(list) => list.first().map(|r| (r.num, r.denom)),
        _ => None,
    }
}

/// 小数展示：保留至多 4 位并去尾零（28/10 → "2.8"；40/10 → "4"）。
fn trim_decimal(num: u32, den: u32) -> Option<String> {
    if den == 0 {
        return None;
    }
    let value = num as f64 / den as f64;
    let mut text = format!("{value:.4}");
    while text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    (!text.is_empty() && text != "-0").then_some(text)
}

/// f/ 值：纯小数（"2.8"）。
fn format_f_number(r: (u32, u32)) -> Option<String> {
    trim_decimal(r.0, r.1)
}

/// 快门：分子为 1 用分数形式（"1/250"），其余小数（"0.4"、"2"）。
fn format_exposure_time(r: (u32, u32)) -> Option<String> {
    let (num, den) = r;
    if num == 0 || den == 0 {
        return None;
    }
    if num == 1 && den > 1 {
        return Some(format!("1/{den}"));
    }
    trim_decimal(num, den)
}

/// 焦距 mm：纯小数（"85"、"8.5"）。
fn format_focal_length(r: (u32, u32)) -> Option<String> {
    trim_decimal(r.0, r.1)
}

// ---------------------------------------------------------------------------
// 宽高：JPEG SOF 帧段扫描（含 RAW 内嵌预览）+ TIFF IFD0 尺寸回退
// ---------------------------------------------------------------------------

/// 扫描缓冲内所有 SOI（0xFFD8），对每个候选走 marker 链找 SOF0..SOF3 帧头；
/// JPEG 第一个 SOI 即主图，RAW（TIFF 容器）则命中内嵌预览的 SOI。
/// 任一步骤越界/结构非法即换下一候选；全失败返回 None。绝不 panic。
fn jpeg_sof_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    let mut from = 0usize;
    while let Some(rel) = head[from..].windows(2).position(|w| w == [0xFF, 0xD8]) {
        let soi = from + rel;
        if let Some(dims) = walk_jpeg_markers(head, soi + 2) {
            return Some(dims);
        }
        from = soi + 2;
        if from + 4 > head.len() {
            return None;
        }
    }
    None
}

/// 从 `i` 起走 JPEG marker 链：SOF0..SOF3（跳过 C4/C8/CC 非帧段）解析
/// (width, height)；SOS（FFDA）后是熵编码不再有 SOF；无负载段
///（SOI/EOI/RST/TEM）跨过；其余段按 16 位长度跳过。最多 256 段防环。
fn walk_jpeg_markers(head: &[u8], mut i: usize) -> Option<(u32, u32)> {
    for _ in 0..256 {
        // 0xFF 填充字节（对齐填充）
        while i < head.len() && head[i] == 0xFF && head.get(i + 1) == Some(&0xFF) {
            i += 1;
        }
        if i + 4 > head.len() || head[i] != 0xFF {
            return None;
        }
        let marker = head[i + 1];
        match marker {
            0x01 | 0xD0..=0xD7 => i += 2, // TEM/RST：无负载
            0xD8..=0xDA => return None,   // SOI 重叠/EOI/SOS：无 SOF
            0xC0..=0xCF => {
                if matches!(marker, 0xC4 | 0xC8 | 0xCC) {
                    i += 2 + seg_len(head, i)?; // DHT/JPG/DAC：非帧段
                    continue;
                }
                let len = seg_len(head, i)?;
                if len < 7 || i + 2 + len > head.len() {
                    return None;
                }
                let height = u16::from_be_bytes([head[i + 5], head[i + 6]]);
                let width = u16::from_be_bytes([head[i + 7], head[i + 8]]);
                return Some((u32::from(width), u32::from(height)));
            }
            _ => i += 2 + seg_len(head, i)?, // APPn/COM/其余：跳负载
        }
    }
    None
}

/// 段长度（i 指向 0xFF；长度字段在 marker 后，含自身 2 字节）。
fn seg_len(head: &[u8], i: usize) -> Option<usize> {
    let hi = *head.get(i + 2)?;
    let lo = *head.get(i + 3)?;
    Some(usize::from(u16::from_be_bytes([hi, lo])))
}

/// TIFF 形态（RAW 容器）回退：IFD0 的 ImageWidth(0x0100)/ImageLength(0x0101)
/// （SHORT/LONG）。容器非法/缺 tag 返回 None。
fn tiff_ifd0_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 8 {
        return None;
    }
    let le = match &head[0..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = [head.get(i)?, head.get(i + 1)?];
        let b = [*b[0], *b[1]];
        Some(if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = [
            head.get(i)?,
            head.get(i + 1)?,
            head.get(i + 2)?,
            head.get(i + 3)?,
        ];
        let b = [*b[0], *b[1], *b[2], *b[3]];
        Some(if le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let ifd0 = u32_at(4)? as usize;
    let count = u16_at(ifd0)? as usize;
    let mut width = None;
    let mut height = None;
    for n in 0..count {
        let entry = ifd0 + 2 + n * 12;
        let tag = u16_at(entry)?;
        let typ = u16_at(entry + 2)?;
        // 值 ≤4 字节内联（SHORT/LONG count=1 皆内联）
        let value = u32_at(entry + 8)?;
        let value = match (tag, typ) {
            (0x0100, 3) | (0x0101, 3) => {
                // SHORT 内联：按端序取低/高端 2 字节
                let b = value.to_le_bytes();
                Some(u32::from(if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[2], b[3]])
                }))
            }
            (0x0100, 4) | (0x0101, 4) => Some(value),
            _ => None,
        };
        match tag {
            0x0100 => width = value,
            0x0101 => height = value,
            _ => {}
        }
        if width.is_some() && height.is_some() {
            break;
        }
    }
    Some((width?, height?))
}

/// camera = Make + " " + Model，缺失一侧则用另一侧，多余空格折叠为一个。
fn camera_string(exif: &exif::Exif) -> Option<String> {
    let combined = match (
        ascii_value(exif, exif::Tag::Make),
        ascii_value(exif, exif::Tag::Model),
    ) {
        (Some(make), Some(model)) => format!("{make} {model}"),
        (Some(make), None) => make,
        (None, Some(model)) => model,
        (None, None) => return None,
    };
    let collapsed = combined.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}
