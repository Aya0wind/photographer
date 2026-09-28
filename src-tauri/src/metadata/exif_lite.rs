//! M1 EXIF-lite（spec §5.3 / T6），M3.5 扩展拍摄参数：captured_at / 相机 /
//! 宽高 / ISO / 光圈 / 快门 / 焦距 / 镜头。容器解析交给 kamadak-exif
//! （JPEG APP1、TIFF 系 II*/MM*——即多数 NEF/ARW/CR2/DNG/ORF——另有
//! HEIF/PNG/WebP 附赠支持），解析容错（continue_on_error）：head 截断导致
//! 个别 IFD 值越界只丢该字段，主 IFD 链其余字段照常返回。
//! 宽高取主图帧，分层（gen-5，2026-09-21 尼康 NEF 修复）：
//! EXIF PixelX/YDimension(0xA002/0xA003) > TIFF 主 IFD 链 + SubIFD(0x014A)
//! 主图尺寸（NewSubfileType=0 优先）> JPEG（SOI 起始）SOF0..SOF3 帧段。
//! 内嵌 preview JPEG（RAW 容器里的缩略图层）一律不作为尺寸源——尼康 NEF
//! 的 IFD0 是缩略图 IFD 且 EXIF 不写 PixelX/YDimension，旧策略回退到内嵌
//! 缩略图 SOF 曾把 Z5 的 6064×4040 本体显示成 640×424（2026-09-21）。
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
    let exif = read_exif(head);
    let dims = pixel_dimensions(head, exif.as_ref());
    let Some(exif) = exif else {
        // EXIF 容器失败（纯 JPEG 无 APP1 / 损坏）：宽高仍可从主图帧提取
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
        iso: uint_field(&exif, 0x8827).filter(|v| *v > 0).map(u32::from),
        f_number: rational_field(&exif, 0x829D).and_then(format_f_number),
        exposure_time: rational_field(&exif, 0x829A).and_then(format_exposure_time),
        focal_length: rational_field(&exif, 0x920A).and_then(format_focal_length),
        lens: ascii_value(&exif, exif::Tag::LensModel).and_then(equipment_text),
        deep: deep_exif(&exif),
    }
}

/// EXIF 容器解析（容错模式）：continue_on_error 使 head 截断导致的单字段
/// 越界（如 RAW 的 MakerNote/条带数据落在截取窗之外）只丢该字段；严格
/// 模式下任一字段截断会废掉整个解析（EXIF 全 None）。部分结果经
/// `distill_partial_result` 蒸馏，硬错误（非 EXIF 容器等）照旧 None。
fn read_exif(head: &[u8]) -> Option<exif::Exif> {
    exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut Cursor::new(head))
        .or_else(|e| e.distill_partial_result(|_| {}))
        .ok()
}

/// 宽高分层（gen-5）：EXIF PixelX/YDimension（拍摄设备真实输出尺寸，
/// ARW 真机 61MP 修复沿用）> 容器主图帧——TIFF 基 RAW 走主 IFD 链 +
/// SubIFD 主图（绝不读内嵌 preview JPEG：那是缩略图层，NEF 真机曾把
/// 6064×4040 本体显示成 640×424）；真 JPEG（SOI 起始）走 SOF 帧段；
/// 其余容器（CR3 的 ISO BMFF / HEIC / PNG / WebP）无主图帧支持——
/// 宁缺毋错，绝不把内嵌预览尺寸冒充本体。
fn pixel_dimensions(head: &[u8], exif: Option<&exif::Exif>) -> Option<(u32, u32)> {
    let exif_dims = exif.and_then(exif_pixel_dimensions);
    if is_tiff_container(head) {
        exif_dims.or_else(|| tiff_raw_dimensions(head))
    } else {
        exif_dims.or_else(|| jpeg_sof_dimensions(head))
    }
}

/// TIFF 容器判定（II*/MM* 魔数）：NEF/ARW/CR2/DNG/ORF/RW2 等 TIFF 基 RAW。
fn is_tiff_container(head: &[u8]) -> bool {
    head.starts_with(b"II\x2a\x00") || head.starts_with(b"MM\x00\x2a")
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
    let exif = read_exif(head)?;
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
    if num == 0 || den == 0 {
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
    (!text.is_empty() && text != "0" && text != "-0").then_some(text)
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
// 宽高：JPEG SOF 帧段（仅真 JPEG 容器）+ TIFF 主 IFD 链/SubIFD（TIFF 基 RAW）
// ---------------------------------------------------------------------------

/// JPEG（SOI 起始）的 SOF0..SOF3 帧段尺寸。非 JPEG 容器直接 None——
/// RAW/CR3 等容器内嵌的 JPEG 是预览/缩略图层，扫到也不代表本体尺寸
/// （NEF 真机坑：6064×4040 本体被写成 640×424 缩略图尺寸，2026-09-21）。
/// 结构非法返回 None。绝不 panic。
fn jpeg_sof_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    if !head.starts_with(&[0xFF, 0xD8]) {
        return None; // 非 JPEG 容器：内嵌 JPEG 属预览层，不冒充本体帧
    }
    walk_jpeg_markers(head, 2)
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

/// TIFF 按端序读 16 位无符号整数（越界 None）。
fn tiff_u16(head: &[u8], i: usize, le: bool) -> Option<u16> {
    let b = [*head.get(i)?, *head.get(i + 1)?];
    Some(if le {
        u16::from_le_bytes(b)
    } else {
        u16::from_be_bytes(b)
    })
}

/// TIFF 按端序读 32 位无符号整数（越界 None）。
fn tiff_u32(head: &[u8], i: usize, le: bool) -> Option<u32> {
    let b = [
        *head.get(i)?,
        *head.get(i + 1)?,
        *head.get(i + 2)?,
        *head.get(i + 3)?,
    ];
    Some(if le {
        u32::from_le_bytes(b)
    } else {
        u32::from_be_bytes(b)
    })
}

/// IFD 尺寸候选：(是否主图, w, h)。主图 = NewSubfileType(0x00FE) 缺失或 0
/// ——TIFF 基 RAW 的 IFD0 常是缩略图层（NEF/ARW 的 NewSubfileType=1 甚至
/// 不带尺寸 tag），全尺寸住在 SubIFD(0x014A) 的主图 IFD 里。
type IfdDims = (bool, u32, u32);

/// TIFF 基 RAW 的主图尺寸兜底（gen-5）：IFD0 → next-IFD 链（同 kamadak
/// 上限 8 层）→ 各级 SubIFD(0x014A)，收集全部 (NewSubfileType, w, h)
/// 候选后取**主图（NewSubfileType=0）中面积最大**者，无主图再退全体最大
/// ——RAW 的全尺寸 IFD 永远是最大的一层，缩略图/预览 IFD 天然靠后。
/// 真机样本（2026-09-21）：Z5 NEF 的 IFD0 无尺寸、EXIF 无 0xA002，全尺寸
/// 6064×4040 在 SubIFD#1（NewSubfileType=0）；LR 导出 DNG 的 IFD0 是
/// 256×171 缩略图，全尺寸在 SubIFD#0；ARW 走 EXIF 0xA002 优先，不到此层。
/// 容器非法/无候选返回 None；环/垃圾偏移防御（visited 去重 + 数量上限）。
fn tiff_raw_dimensions(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 8 {
        return None;
    }
    let le = match &head[0..4] {
        b"II\x2a\x00" => true,
        b"MM\x00\x2a" => false,
        _ => return None,
    };
    /// 单个 IFD 的尺寸值（SHORT/LONG 均内联；SHORT 按 TIFF 规范左对齐
    /// 存于 4 字节值字段的前 2 字节）。
    fn dim_value(head: &[u8], entry: usize, le: bool) -> Option<u32> {
        match tiff_u16(head, entry + 2, le)? {
            3 => tiff_u16(head, entry + 8, le).map(u32::from), // SHORT
            4 => tiff_u32(head, entry + 8, le),                // LONG
            _ => None,
        }
    }
    let ifd0 = tiff_u32(head, 4, le)? as usize;
    let mut queue = vec![ifd0];
    let mut visited: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut candidates: Vec<IfdDims> = Vec::new();
    while let Some(off) = queue.pop() {
        if off < 8 || off + 2 > head.len() || !visited.insert(off) || visited.len() > 32 {
            continue; // 越界/重复/超量：环与垃圾防御
        }
        // 条目数上限：真机 IFD 最多几十条，超界即垃圾
        let Some(count) = tiff_u16(head, off, le) else {
            continue;
        };
        if count > 512 {
            continue;
        }
        let mut width = None;
        let mut height = None;
        let mut subfile: Option<u32> = None;
        for n in 0..count as usize {
            let entry = off + 2 + n * 12;
            if entry + 12 > head.len() {
                break; // 截断：已收集的前缀仍可用
            }
            let Some(tag) = tiff_u16(head, entry, le) else {
                break;
            };
            match tag {
                0x0100 => width = dim_value(head, entry, le),
                0x0101 => height = dim_value(head, entry, le),
                0x00FE => subfile = tiff_u32(head, entry + 8, le),
                // SubIFD 指针（LONG/IFD 型）：count=1 内联，多数组存偏移
                0x014A => {
                    let (typ, cnt) = (
                        tiff_u16(head, entry + 2, le).unwrap_or(0),
                        tiff_u32(head, entry + 4, le).unwrap_or(0) as usize,
                    );
                    if matches!(typ, 4 | 13) && cnt > 0 {
                        if cnt == 1 {
                            if let Some(p) = tiff_u32(head, entry + 8, le) {
                                queue.push(p as usize);
                            }
                        } else if let Some(base) = tiff_u32(head, entry + 8, le) {
                            for k in 0..cnt.min(16) {
                                if let Some(p) = tiff_u32(head, base as usize + k * 4, le) {
                                    queue.push(p as usize);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if let (Some(w), Some(h)) = (width, height) {
            candidates.push((subfile.unwrap_or(0) == 0, w, h));
        }
        // next-IFD 链（与 kamadak 一致：主链即 IFD0/IFD1/…）
        let next_at = off + 2 + count as usize * 12;
        if next_at + 4 <= head.len() {
            if let Some(next) = tiff_u32(head, next_at, le).filter(|n| *n != 0) {
                queue.push(next as usize);
            }
        }
    }
    // 尺寸合法域过滤（1..=10 万像素），主图优先、面积最大者胜出
    candidates
        .iter()
        .filter(|(_, w, h)| *w > 0 && *h > 0 && *w <= 100_000 && *h <= 100_000)
        .max_by_key(|&&(primary, w, h)| (primary, u64::from(w) * u64::from(h)))
        .map(|&(_, w, h)| (w, h))
}

/// camera = Make + " " + Model，缺失一侧则用另一侧，多余空格折叠为一个。
fn camera_string(exif: &exif::Exif) -> Option<String> {
    let combined = match (
        ascii_value(exif, exif::Tag::Make).and_then(equipment_text),
        ascii_value(exif, exif::Tag::Model).and_then(equipment_text),
    ) {
        (Some(make), Some(model)) => format!("{make} {model}"),
        (Some(make), None) => make,
        (None, Some(model)) => model,
        (None, None) => return None,
    };
    let collapsed = combined.split_whitespace().collect::<Vec<_>>().join(" ");
    (!collapsed.is_empty()).then_some(collapsed)
}

/// EXIF writers use these labels for missing equipment, rather than actual names.
pub const MISSING_EQUIPMENT_TEXT: &[&str] = &[
    "",
    "-",
    "--",
    "---",
    "----",
    "—",
    "–",
    "n/a",
    "na",
    "unknown",
    "none",
    "null",
    "not available",
];

fn equipment_text(text: String) -> Option<String> {
    let text = text.trim();
    (!MISSING_EQUIPMENT_TEXT.contains(&text.to_ascii_lowercase().as_str()))
        .then(|| text.to_string())
}
