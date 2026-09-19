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
    }
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

/// EXIF 日期格式 `"%Y:%m:%d %H:%M:%S"`，不含时区 → 按待定时间视作 UTC。
fn datetime_field(exif: &exif::Exif, tag: exif::Tag) -> Option<DateTime<Utc>> {
    let raw = ascii_value(exif, tag)?;
    NaiveDateTime::parse_from_str(&raw, "%Y:%m:%d %H:%M:%S")
        .ok()
        .map(|naive| naive.and_utc())
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
