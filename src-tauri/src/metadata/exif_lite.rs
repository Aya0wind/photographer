//! M1 EXIF-lite（spec §5.3 / T6）：仅提取拍摄时间与相机型号，供导入流水线
//! 计算目标路径。容器解析交给 kamadak-exif（JPEG APP1、TIFF 系 II*/MM*——
//! 即多数 NEF/ARW/CR2/DNG/ORF——另有 HEIF/PNG/WebP 附赠支持）。
//! 任何失败（垃圾/空/截断）静默返回全 None——绝不 Err、绝不 panic。

// lib 侧 `mod metadata;` 为私有模块且导入流水线（T7）尚未接线，库内暂无调用方

use std::io::Cursor;

use chrono::{DateTime, NaiveDateTime, Utc};

/// EXIF-lite 提取结果：全部字段可缺失。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MetaLite {
    /// DateTimeOriginal(0x9003) 优先，回退 DateTime(0x0132)，均为 UTC。
    pub captured_at: Option<DateTime<Utc>>,
    /// EXIF Make + Model（多余空白折叠）。
    pub camera: Option<String>,
}

/// 从文件头部字节（建议 ≥1MB）解析 EXIF-lite。永不返回 Err，也绝不 panic。
pub fn parse(head: &[u8]) -> MetaLite {
    let exif = match exif::Reader::new().read_from_container(&mut Cursor::new(head)) {
        Ok(exif) => exif,
        Err(_) => return MetaLite::default(),
    };
    MetaLite {
        captured_at: datetime_field(&exif, exif::Tag::DateTimeOriginal)
            .or_else(|| datetime_field(&exif, exif::Tag::DateTime)),
        camera: camera_string(&exif),
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
