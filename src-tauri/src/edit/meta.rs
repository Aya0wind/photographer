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
#[allow(dead_code)] // Compatibility entry point used by metadata integration tests.
pub fn apply_metadata(jpeg: Vec<u8>, source_bytes: Option<&[u8]>, opts: &MetaOptions) -> Vec<u8> {
    apply_metadata_inner(jpeg,source_bytes,opts,None,true)
}
#[allow(dead_code)] // 集成测试入口（生产走 color 版；srgb=true 快捷形态）。
pub fn apply_metadata_for_image(jpeg:Vec<u8>,source_bytes:Option<&[u8]>,opts:&MetaOptions,width:u32,height:u32)->Vec<u8>{
    apply_metadata_inner(jpeg,source_bytes,opts,Some((width,height)),true)
}
pub(super) fn apply_metadata_for_color_image(jpeg:Vec<u8>,source_bytes:Option<&[u8]>,opts:&MetaOptions,width:u32,height:u32,srgb:bool)->Vec<u8>{
    apply_metadata_inner(jpeg,source_bytes,opts,Some((width,height)),srgb)
}
fn apply_metadata_inner(jpeg:Vec<u8>,source_bytes:Option<&[u8]>,opts:&MetaOptions,size:Option<(u32,u32)>,srgb:bool)->Vec<u8>{
    if opts.is_noop() && source_bytes.is_none() && size.is_none() {
        return jpeg;
    }
    let fields=prepared_fields(source_bytes,opts,size,srgb);
    let author=opts.author.as_deref().map(str::trim).filter(|s|!s.is_empty());
    let copyright=opts.copyright.as_deref().map(str::trim).filter(|s|!s.is_empty());

    let jpeg = Bytes::from(jpeg);
    let mut parsed = match Jpeg::from_bytes(jpeg.clone()) {
        Ok(parsed) => parsed,
        Err(_) => return Vec::from(jpeg),
    };
    parsed.segments_mut().retain(|segment| !(segment.marker() == 0xE1 && is_exif_or_xmp_app1(segment.contents())));
    if !fields.is_empty(){if let Some(app1)=build_exif_app1(&fields){parsed.segments_mut().insert(1,app1);}}
    if author.is_some()||copyright.is_some()||!opts.keywords.is_empty(){parsed.segments_mut().insert(1,build_xmp_app1(author,copyright,&opts.keywords));}
    let mut out=Vec::new();parsed.encoder().write_to(&mut Cursor::new(&mut out)).ok().filter(|_|!out.is_empty()).map(|_|out).unwrap_or_else(||Vec::from(jpeg))
}
fn prepared_fields(source_bytes:Option<&[u8]>,opts:&MetaOptions,size:Option<(u32,u32)>,srgb:bool)->Vec<Field>{
    let mut fields: Vec<Field> = source_bytes
        .and_then(|bytes| read_exif_fields(bytes))
        .unwrap_or_default();
    if let Some((width,height))=size {
        // Pixels have already been oriented and transformed. Old preview IFDs
        // and their offsets cannot describe this new encoded image.
        fields.retain(|field|field.ifd_num==In::PRIMARY);
        fields.retain(|field|field.tag!=Tag::ColorSpace);fields.push(Field{tag:Tag::ColorSpace,ifd_num:In::PRIMARY,value:Value::Short(vec![if srgb{1}else{65535}])});
        for (tag,value) in [(Tag::Orientation,Value::Short(vec![1])),(Tag::ImageWidth,Value::Long(vec![width])),(Tag::ImageLength,Value::Long(vec![height])),(Tag::PixelXDimension,Value::Long(vec![width])),(Tag::PixelYDimension,Value::Long(vec![height]))]{
            fields.retain(|field|field.tag!=tag);fields.push(Field{tag,ifd_num:In::PRIMARY,value});
        }
    }
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

    fields
}
/// Native codecs accept TIFF EXIF and plain XML rather than JPEG APP1 segments.
#[allow(dead_code)] // 旧签名仅 tiff_metadata 集成测试引用（生产走 with_color_space）。
pub(super) fn flat_metadata(source:Option<&[u8]>,opts:&MetaOptions,width:u32,height:u32)->Result<(Vec<u8>,Option<String>),String>{
    flat_metadata_with_color_space(source,opts,width,height,true)
}
pub(super) fn flat_metadata_with_color_space(source:Option<&[u8]>,opts:&MetaOptions,width:u32,height:u32,srgb:bool)->Result<(Vec<u8>,Option<String>),String>{
    let fields=prepared_fields(source,opts,Some((width,height)),srgb);let mut bytes=Vec::new();let mut writer=ExifWriter::new();for field in &fields{writer.push_field(field);}writer.write(&mut Cursor::new(&mut bytes),true).map_err(|e|e.to_string())?;
    let author=opts.author.as_deref().map(str::trim).filter(|s|!s.is_empty());let copyright=opts.copyright.as_deref().map(str::trim).filter(|s|!s.is_empty());
    let xmp=if author.is_some()||copyright.is_some()||!opts.keywords.is_empty(){let segment=build_xmp_app1(author,copyright,&opts.keywords);Some(String::from_utf8(segment.contents()[b"http://ns.adobe.com/xap/1.0/\0".len()..].to_vec()).map_err(|e|e.to_string())?)}else{None};
    Ok((bytes,xmp))
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
/// TIFF/PNG metadata may live beyond the first few MB. Seek to metadata via
/// the upstream EXIF reader rather than loading the complete image into RAM.
pub(super) fn source_exif(path:&std::path::Path)->Option<Vec<u8>>{
    let file=std::fs::File::open(path).ok()?;let exif=exif::Reader::new().continue_on_error(true).read_from_container(&mut std::io::BufReader::new(file)).ok()?;
    let mut writer=ExifWriter::new();for field in exif.fields(){writer.push_field(field);}let mut bytes=Cursor::new(Vec::new());writer.write(&mut bytes,true).ok()?;Some(bytes.into_inner())
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
#[cfg(test)]
mod output_metadata_tests {
    use super::*;
    #[test]
    fn output_metadata_uses_oriented_dimensions_and_discards_old_thumbnail_fields(){
        let image=image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(6,4,image::Rgb([80,120,160])));let mut encoded=Cursor::new(Vec::new());image.write_to(&mut encoded,image::ImageFormat::Jpeg).unwrap();let jpeg=encoded.into_inner();
        let fields=vec![Field{tag:Tag::Orientation,ifd_num:In::PRIMARY,value:Value::Short(vec![6])},Field{tag:Tag::PixelXDimension,ifd_num:In::PRIMARY,value:Value::Long(vec![6000])},Field{tag:Tag::PixelYDimension,ifd_num:In::PRIMARY,value:Value::Long(vec![4000])},Field{tag:Tag::Model,ifd_num:In::PRIMARY,value:Value::Ascii(vec![b"Test camera".to_vec()])},Field{tag:Tag::Orientation,ifd_num:In(1),value:Value::Short(vec![3])}];
        let mut source=Jpeg::from_bytes(Bytes::from(jpeg.clone())).unwrap();source.segments_mut().insert(1,build_exif_app1(&fields).unwrap());let mut source_bytes=Vec::new();source.encoder().write_to(&mut source_bytes).unwrap();
        let output=apply_metadata_for_image(jpeg.clone(),Some(&source_bytes),&MetaOptions::default(),6,4);let parsed=exif::Reader::new().read_from_container(&mut Cursor::new(&output)).unwrap();
        for (tag,expected) in [(Tag::Orientation,1),(Tag::ImageWidth,6),(Tag::ImageLength,4),(Tag::PixelXDimension,6),(Tag::PixelYDimension,4)]{assert_eq!(parsed.get_field(tag,In::PRIMARY).unwrap().value.get_uint(0),Some(expected));}
        assert!(parsed.get_field(Tag::Model,In::PRIMARY).is_some());assert!(parsed.fields().all(|field|field.ifd_num==In::PRIMARY));
        let without_metadata=|bytes:Vec<u8>|Jpeg::from_bytes(Bytes::from(bytes)).unwrap().segments().iter().filter(|segment|segment.marker()!=0xE1).map(|segment|(segment.marker(),segment.contents().clone())).collect::<Vec<_>>();assert_eq!(without_metadata(jpeg),without_metadata(output));
    }
}
