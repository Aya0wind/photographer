//! 导出 JPEG 元数据改写（阶段 D）。
//!
//! 分层（契约：「修改本应用数据库 / 写入 XMP / 写入导出 JPEG」是三件事，
//! 本模块只做第三件——导出产物）：
//! - **源 EXIF 尽量保留**：从源文件（JPEG APP1 / TIFF 基 RAW 容器）读出
//!   全部字段，拍摄时间/相机/镜头/曝光参数原样带走；
//! - **removeGps=true**：丢弃整个 GPS IFD（kamadak-exif 重写时不带
//!   GPSInfoIFDPointer，GPS 块整体消失）；
//! - **copyright/author** → EXIF IFD0 Artist(0x013B) / Copyright(0x8298) +
//!   XMP `dc:rights` / `dc:creator`；
//! - **keywords** → XMP `dc:subject`。
//!
//! 容器操作用 img-parts 做 **JPEG 段级改写**：图像数据（熵编码流）逐字节
//! 原样保留，只替换/插入 APP1(EXIF) 与 APP1(XMP) 段——绝不重编码。
//! EXIF 内容层用 kamadak-exif Reader→Writer 重建（MakerNote 等超纲容器
//! 数据可能丢失；拍摄时间/相机/镜头/参数这类契约要求保住的字段不会）。

use std::io::Cursor;

use exif::experimental::Writer as ExifWriter;
use exif::{Context, Field, In, Tag, Value};
use img_parts::jpeg::{Jpeg, JpegSegment};
use img_parts::Bytes;

/// 元数据写入选项（已与导出 options 对齐的终值）。
#[derive(Debug, Clone, Default)]
pub struct MetaOptions {
    pub remove_gps: bool,
    pub copyright: Option<String>,
    pub author: Option<String>,
    pub keywords: Vec<String>,
}

impl MetaOptions {
    /// 无任何元数据动作（源 EXIF 也不动：无源可读时直接原样返回）。
    fn is_noop(&self) -> bool {
        !self.remove_gps
            && self.copyright.as_deref().unwrap_or("").is_empty()
            && self.author.as_deref().unwrap_or("").is_empty()
            && self.keywords.is_empty()
    }
}

/// 把元数据段合入已编码 JPEG：
/// 1. 从 `source_bytes`（源 JPEG/RAW）读 EXIF 字段（失败 = 空集）；
/// 2. removeGps → 滤 GPS；author/copyright → 设 IFD0 Artist/Copyright；
/// 3. 有任何字段 → 重建 APP1(EXIF)；有版权/作者/关键词 → 建 APP1(XMP)；
/// 4. img-parts 替换产物里既有的 EXIF/XMP APP1 段（SOI 后插入）。
pub fn apply_metadata(jpeg: Vec<u8>, source_bytes: Option<&[u8]>, opts: &MetaOptions) -> Vec<u8> {
    if opts.is_noop() && source_bytes.is_none() {
        return jpeg;
    }
    let mut fields: Vec<Field> = source_bytes
        .and_then(|bytes| read_exif_fields(bytes))
        .unwrap_or_default();
    if opts.remove_gps {
        // GPS 字段的甄别键是 Tag 的 Context（kamadak-exif 读出的子 IFD 字段
        // ifd_num 沿用父 IFD）；GPSInfoIFDPointer 同为 Gps 上下文一并丢弃，
        // Writer 重建时不再合成 GPS 块。
        fields.retain(|field| field.tag.context() != Context::Gps);
    }
    let author = opts
        .author
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(author) = author {
        set_ascii(&mut fields, Tag::Artist, In::PRIMARY, author);
    }
    let copyright = opts
        .copyright
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(copyright) = copyright {
        set_ascii(&mut fields, Tag::Copyright, In::PRIMARY, copyright);
    }

    let jpeg = Bytes::from(jpeg);
    let mut parsed = match Jpeg::from_bytes(jpeg.clone()) {
        Ok(parsed) => parsed,
        Err(_) => return Vec::from(jpeg), // 编码器产物异常（理论不可达）：不冒险动它
    };
    // 剥既有 EXIF/XMP APP1（稍后重写；其余段包括 JFIF/图像数据原样保留）
    parsed
        .segments_mut()
        .retain(|segment| !(segment.marker() == 0xE1 && is_exif_or_xmp_app1(segment.contents())));
    if !fields.is_empty() {
        if let Some(app1) = build_exif_app1(&fields) {
            parsed.segments_mut().insert(1, app1); // SOI/APP0 之后
        }
    }
    let needs_xmp = author.is_some() || copyright.is_some() || !opts.keywords.is_empty();
    if needs_xmp {
        parsed
            .segments_mut()
            .insert(1, build_xmp_app1(author, copyright, &opts.keywords));
    }
    let mut out = Vec::new();
    parsed
        .encoder()
        .write_to(&mut Cursor::new(&mut out))
        .ok()
        .filter(|_| !out.is_empty())
        .map(|_| out)
        .unwrap_or_else(|| Vec::from(jpeg))
}

/// 源容器（JPEG / TIFF 基 RAW）→ EXIF 字段集。CR3（ISO BMFF）等不支持的
/// 容器返回 None（导出仍成功，只是没 EXIF 可带——契约允许「尽量保留」）。
fn read_exif_fields(bytes: &[u8]) -> Option<Vec<Field>> {
    exif::Reader::new()
        .continue_on_error(true)
        .read_from_container(&mut Cursor::new(bytes))
        .map(|exif| exif.fields().cloned().collect())
        .ok()
}

/// 设/替换一个 ASCII 字段（同 tag 旧行移除；EXIF ASCII 以 NUL 结尾）。
fn set_ascii(fields: &mut Vec<Field>, tag: Tag, ifd_num: In, value: &str) {
    fields.retain(|field| !(field.tag == tag && field.ifd_num == ifd_num));
    fields.push(Field {
        tag,
        ifd_num,
        value: Value::Ascii(vec![value.as_bytes().to_vec()]),
    });
}

/// APP1(EXIF) 段：`Exif\0\0` + TIFF 流（小端）。
fn build_exif_app1(fields: &[Field]) -> Option<JpegSegment> {
    let mut tiff = Vec::new();
    let mut writer = ExifWriter::new();
    for field in fields {
        writer.push_field(field);
    }
    writer.write(&mut Cursor::new(&mut tiff), false).ok()?;
    let mut contents = Vec::with_capacity(6 + tiff.len());
    contents.extend_from_slice(b"Exif\0\0");
    contents.extend_from_slice(&tiff);
    Some(JpegSegment::new_with_contents(0xE1, Bytes::from(contents)))
}

/// APP1(XMP) 段：标准 xpacket RDF（dc:rights / dc:creator / dc:subject）。
fn build_xmp_app1(
    author: Option<&str>,
    copyright: Option<&str>,
    keywords: &[String],
) -> JpegSegment {
    let mut xml = String::with_capacity(512);
    xml.push_str("<?xpacket begin=\"﻿\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    xml.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    xml.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    xml.push_str(
        "  <rdf:Description rdf:about=\"\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n",
    );
    if let Some(copyright) = copyright {
        xml.push_str("   <dc:rights><rdf:Alt><rdf:li xml:lang=\"x-default\">");
        xml.push_str(&xml_escape(copyright));
        xml.push_str("</rdf:li></rdf:Alt></dc:rights>\n");
    }
    if let Some(author) = author {
        xml.push_str("   <dc:creator><rdf:Seq><rdf:li>");
        xml.push_str(&xml_escape(author));
        xml.push_str("</rdf:li></rdf:Seq></dc:creator>\n");
    }
    if !keywords.is_empty() {
        xml.push_str("   <dc:subject><rdf:Bag>");
        for keyword in keywords {
            xml.push_str("<rdf:li>");
            xml.push_str(&xml_escape(keyword));
            xml.push_str("</rdf:li>");
        }
        xml.push_str("</rdf:Bag></dc:subject>\n");
    }
    xml.push_str("  </rdf:Description>\n");
    xml.push_str(" </rdf:RDF>\n");
    xml.push_str("</x:xmpmeta>\n");
    xml.push_str("<?xpacket end=\"w\"?>");

    let mut contents = Vec::with_capacity(28 + xml.len());
    contents.extend_from_slice(b"http://ns.adobe.com/xap/1.0/\0");
    contents.extend_from_slice(xml.as_bytes());
    JpegSegment::new_with_contents(0xE1, Bytes::from(contents))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// APP1 载荷是否为 EXIF（`Exif\0`）或 XMP（标准命名空间 URI）。
fn is_exif_or_xmp_app1(contents: &Bytes) -> bool {
    let head = &contents[..contents.len().min(29)];
    head.starts_with(b"Exif\0") || head.starts_with(b"http://ns.adobe.com/xap/1.0/")
}
