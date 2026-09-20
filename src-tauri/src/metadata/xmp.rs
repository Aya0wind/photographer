//! XMP 边车（M5 评分与 LR 互通）：读/改/建 `.xmp`，评分字段
//! `xmp:Rating`（0-5，LR 原生读写的就是它）。
//!
//! ## 选型（2026-09-20 定案）
//! - 读取：quick-xml 流式解析（属性与子元素两种形态都认，另兼容
//!   `MicrosoftPhoto:Rating` 1-99 旧标映射）。
//! - 改写：**字符串外科手术**——只替换/插入 Rating 一处，其余字节原样
//!   保留。不整档反序列化再序列化：Adobe XMP Toolkit 绑定（xmp_toolkit）
//!   要 C++/cmake 重构建，纯 Rust XMP 库（gufo-xmp/xmpkit）尚早期、整档
//!   重写有丢 LR 未知字段的危险；对「改一个字段」的需求，字节级保留是
//!   唯一可证明无损的方案。
//! - 新建：最小模板（x:xmpmeta + rdf:Description + xmp:Rating）。
//! - 落盘：原子写（`.tmp` + rename）。
//!
//! 失败语义：本模块返回 Result，调用方（ipc::rating）异步派发、失败经
//! AppError 事件上报，绝不阻塞评分入库。

use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::Reader;

/// xmp:Rating 命名空间前缀（新建模板/属性插入用）。
const XMP_NS: &str = "http://ns.adobe.com/xap/1.0/";

/// 边车路径：同目录同名换扩展名（`DSC_0176.NEF` → `DSC_0176.xmp`；
/// 无扩展名直接补 `.xmp`）。
pub fn sidecar_path(asset_path: &Path) -> PathBuf {
    let file_name = asset_path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stem = match file_name.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => file_name,
    };
    asset_path.with_file_name(format!("{stem}.xmp"))
}

/// MicrosoftPhoto:Rating（1/25/50/75/99）→ 0-5 星映射。
fn ms_rating_to_stars(value: u32) -> Option<u8> {
    match value {
        1 => Some(1),
        25 => Some(2),
        50 => Some(3),
        75 => Some(4),
        99 => Some(5),
        _ => None,
    }
}

/// 解析数字文本为 0-5 星（越界/非数字 None）。
fn parse_stars(text: &str) -> Option<u8> {
    let value: u32 = text.trim().parse().ok()?;
    (value <= 5).then_some(value as u8)
}

/// 读取边车评分（0-5）。认三种形态：rdf:Description 的 `xmp:Rating` 属性、
/// `<xmp:Rating>N</xmp:Rating>` 子元素、`MicrosoftPhoto:Rating` 属性
/// （1-99 旧标）。子元素形态由 [`read_rating_element`] 配套（完整入口
/// [`sidecar_rating`]）；本函数只扫 rdf:Description 开标签上的属性。
pub fn read_rating(xmp_text: &str) -> Option<u8> {
    let mut reader = Reader::from_str(xmp_text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if e.name().local_name().as_ref() != "Description" {
                    continue;
                }
                for attr in e.attributes().flatten() {
                    let key = attr.key.as_ref();
                    let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::default()) else {
                        continue;
                    };
                    match key {
                        "xmp:Rating" => {
                            if let Some(stars) = parse_stars(&value) {
                                return Some(stars);
                            }
                        }
                        "MicrosoftPhoto:Rating" => {
                            if let Ok(raw) = value.trim().parse::<u32>() {
                                if let Some(stars) = ms_rating_to_stars(raw) {
                                    return Some(stars);
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return None, // 结构损坏：当作无评分
        }
    }
    None
}

/// 解析子元素形态评分（`<xmp:Rating>4</xmp:Rating>`）：quick-xml 逐事件
/// 配对 Start/End + 缓冲文本（命名空间声明可在任意祖先上，不要求元素
/// 自带 xmlns——按 local name "Rating" 捕获即可，边车里撞名概率可忽略）。
fn read_rating_element(xmp_text: &str) -> Option<u8> {
    let mut reader = Reader::from_str(xmp_text);
    reader.config_mut().trim_text(true);
    let mut capture = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                capture = e.name().local_name().as_ref() == "Rating";
            }
            Ok(Event::Text(ref t)) if capture => {
                // BytesText: Deref<Target = str>（0.42）；评分为纯数字无转义
                if let Some(stars) = parse_stars(t) {
                    return Some(stars);
                }
            }
            Ok(Event::End(_)) => capture = false,
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
        buf.clear();
    }
    None
}

/// 读取边车评分的完整入口（属性 + 子元素两种形态）。
pub fn sidecar_rating(xmp_text: &str) -> Option<u8> {
    read_rating(xmp_text).or_else(|| read_rating_element(xmp_text))
}

/// 最小边车模板（新建路径）：LR 可直接读的 xmp:Rating。
fn minimal_template(rating: u8) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Smart Photo">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="{XMP_NS}"
    xmp:Rating="{rating}"/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

/// 在文本中把评分写入/更新为 `xmp:Rating`（返回新文本；输入 None = 新建）。
/// 三条路径，全部字节级保留未触内容：
/// 1. 已有 `xmp:Rating="N"` 属性 → 原位替换数字；
/// 2. 已有 `<xmp:Rating>N</xmp:Rating>` 子元素 → 原位替换数字；
/// 3. 都没有 → 在首个 `<rdf:Description` 开标签插入属性（缺 xmlns:xmp
///    声明一并补；无 rdf:Description 时退回整档最小模板——内容太少说明
///    不是有效 XMP，不冒险拼接）。
pub fn write_rating(existing: Option<&str>, rating: u8) -> String {
    let Some(text) = existing else {
        return minimal_template(rating);
    };
    // 路径 1：属性形态原位替换（首个命中）
    if let Some(pos) = text.find("xmp:Rating=\"") {
        let start = pos + "xmp:Rating=\"".len();
        let end = text[start..].find('"').map(|n| start + n);
        if let Some(end) = end {
            let mut out = String::with_capacity(text.len() + 8);
            out.push_str(&text[..start]);
            out.push_str(&rating.to_string());
            out.push_str(&text[end..]);
            return out;
        }
    }
    // 路径 2：子元素形态原位替换（<xmp:Rating>N</xmp:Rating>，数字可为空）
    if let Some(open) = text.find("<xmp:Rating>") {
        let content_start = open + "<xmp:Rating>".len();
        if let Some(close) = text[content_start..].find("</xmp:Rating>") {
            let mut out = String::with_capacity(text.len() + 8);
            out.push_str(&text[..content_start]);
            out.push_str(&rating.to_string());
            out.push_str(&text[content_start + close..]);
            return out;
        }
    }
    // 路径 3：插入属性到首个 rdf:Description
    if let Some(tag_start) = text.find("<rdf:Description") {
        let tag_end = text[tag_start..].find('>').map(|n| tag_start + n);
        let Some(tag_end) = tag_end else {
            return minimal_template(rating);
        };
        let tag = &text[tag_start..tag_end];
        let needs_ns = !tag.contains("xmlns:xmp=");
        let mut insert = String::new();
        if needs_ns {
            insert.push_str(&format!("\n    xmlns:xmp=\"{XMP_NS}\""));
        }
        insert.push_str(&format!("\n    xmp:Rating=\"{rating}\""));
        let mut out = String::with_capacity(text.len() + insert.len());
        out.push_str(&text[..tag_end]);
        out.push_str(&insert);
        out.push_str(&text[tag_end..]);
        return out;
    }
    minimal_template(rating)
}

/// 同步评分到边车（读已有 → 改/建 → 原子写）。返回是否真的落盘。
/// 文件不存在 → 新建最小模板；已存在的其余内容逐字节保留。
pub fn sync_rating_to_sidecar(asset_path: &Path, rating: u8) -> Result<(), String> {
    let sidecar = sidecar_path(asset_path);
    let existing = std::fs::read_to_string(&sidecar).ok();
    let updated = write_rating(existing.as_deref(), rating);
    let tmp = sidecar.with_extension("xmp.tmp");
    std::fs::write(&tmp, updated).map_err(|e| format!("写边车失败 {}: {e}", sidecar.display()))?;
    std::fs::rename(&tmp, &sidecar).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("边车落盘失败 {}: {e}", sidecar.display())
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidecar_naming() {
        assert_eq!(
            sidecar_path(Path::new(r"D:\lib\DSC_0176.NEF")),
            PathBuf::from(r"D:\lib\DSC_0176.xmp")
        );
        assert_eq!(
            sidecar_path(Path::new(r"D:\lib\noext")),
            PathBuf::from(r"D:\lib\noext.xmp")
        );
    }

    #[test]
    fn read_rating_attribute_and_element_and_ms() {
        // 属性形态（LR 写法）
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4" xmp:Label="Red"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(sidecar_rating(lr), Some(4));
        // 子元素形态
        let elem = r#"<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"><xmp:Rating>2</xmp:Rating></rdf:Description>"#;
        assert_eq!(sidecar_rating(elem), Some(2));
        // MicrosoftPhoto 旧标
        let ms = r#"<rdf:Description rdf:about="" xmlns:MicrosoftPhoto="urn:schemas-microsoft-com:photo" MicrosoftPhoto:Rating="75"/>"#;
        assert_eq!(sidecar_rating(ms), Some(4));
        // 无评分 / 垃圾
        assert_eq!(sidecar_rating(r#"<rdf:Description rdf:about=""/>"#), None);
        assert_eq!(sidecar_rating("not xml at all"), None);
        // 越界评分不认
        assert_eq!(sidecar_rating(r#"<rdf:Description xmp:Rating="9"/>"#), None);
    }

    #[test]
    fn write_rating_preserves_unrelated_bytes() {
        // 已有属性：原位替换，其余逐字节保留
        let lr = r#"<?xpacket begin=""?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="http://ns.adobe.com/xap/1.0/"
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
    xmp:Rating="2" crs:Sharpness="25" crs:Custom="keep-me">
   <crs:Look>
    <rdf:Bag><rdf:li>Punch</rdf:li></rdf:Bag>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;
        let out = write_rating(Some(lr), 5);
        assert!(out.contains(r#"xmp:Rating="5""#));
        assert!(!out.contains(r#"xmp:Rating="2""#));
        assert!(out.contains(r#"crs:Sharpness="25""#));
        assert!(out.contains("Punch"));
        assert!(out.contains("<?xpacket end=\"w\"?>"));
        assert_eq!(sidecar_rating(&out), Some(5));

        // 无 Rating 但有 Description：插入属性 + 补命名空间
        let no_rating = r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>hi</dc:title></rdf:Description>"#;
        let out = write_rating(Some(no_rating), 3);
        assert!(out.contains(r#"xmp:Rating="3""#));
        assert!(out.contains("xmlns:xmp="));
        assert!(out.contains("<dc:title>hi</dc:title>"));
        assert_eq!(sidecar_rating(&out), Some(3));

        // 已有命名空间：只插属性不重复声明
        let with_ns = r#"<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/"/>"#;
        let out = write_rating(Some(with_ns), 1);
        assert_eq!(out.matches("xmlns:xmp=").count(), 1);
        assert!(out.contains(r#"xmp:Rating="1""#));

        // 子元素形态：原位替换
        let elem = r#"<rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/"><xmp:Rating>1</xmp:Rating><xmp:Label>Blue</xmp:Label></rdf:Description>"#;
        let out = write_rating(Some(elem), 4);
        assert!(out.contains("<xmp:Rating>4</xmp:Rating>"));
        assert!(out.contains("<xmp:Label>Blue</xmp:Label>"));

        // 无档 → 最小模板 roundtrip
        let fresh = write_rating(None, 4);
        assert!(fresh.contains("x:xmpmeta"));
        assert_eq!(sidecar_rating(&fresh), Some(4));

        // 0 星（清除评分）也正确写
        let zero = write_rating(Some(lr), 0);
        assert!(zero.contains(r#"xmp:Rating="0""#));
        assert_eq!(sidecar_rating(&zero), Some(0));
    }

    #[test]
    fn sync_creates_and_updates_file() {
        let dir = tempfile::tempdir().unwrap();
        let asset = dir.path().join("IMG_0001.NEF");
        std::fs::write(&asset, b"nef").unwrap();
        sync_rating_to_sidecar(&asset, 3).unwrap();
        let sidecar = sidecar_path(&asset);
        let text = std::fs::read_to_string(&sidecar).unwrap();
        assert_eq!(sidecar_rating(&text), Some(3));
        // 第二次改写保留（模拟 LR 侧追加字段后我们再改评分）
        let lr_style = text.replace("xmp:Rating=\"3\"", "xmp:Rating=\"3\" crs:Extra=\"keep\"");
        std::fs::write(&sidecar, lr_style).unwrap();
        sync_rating_to_sidecar(&asset, 5).unwrap();
        let text = std::fs::read_to_string(&sidecar).unwrap();
        assert!(text.contains(r#"xmp:Rating="5""#));
        assert!(text.contains("crs:Extra=\"keep\""));
    }
}
