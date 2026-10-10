//! XMP 边车（M5 评分与 LR 互通）：读/改/建 `.xmp`，评分字段
//! `xmp:Rating`。写入侧接受 -1..=5：**-1 = 拒绝**（Adobe 业界约定的
//! 拒绝表示，LR 可识别；用户定案 2026-09-27「XMP 即时投影」）——DB 为
//! 真值，边车评分 = rejected ? -1 : rating(0-5)；读取侧不认 -1（拒绝是
//! 应用内状态，不回填 DB）。
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

/// LR 标准颜色标签名（首字母大写的 XMP 值形态；LR 原生读写这些 token）。
pub const COLOR_LABELS_XMP: &[&str] = &["Red", "Yellow", "Green", "Blue", "Purple"];

/// 应用内小写 token（DB 列存储形态）→ XMP 标准色名（首字母大写）。
/// LR 侧映射默认即标准色名（roadmap §3：不同软件映射可不同，v1 不做配置）。
pub fn label_to_xmp(token: &str) -> Option<&'static str> {
    let lower = token.to_ascii_lowercase();
    COLOR_LABELS_XMP
        .iter()
        .find(|name| name.to_ascii_lowercase() == lower)
        .copied()
}

/// XMP 色名 → 应用内小写 token（大小写不敏感认标准色名；未知值 None——
/// 非 LR 标准标签的 xmp:Label 值不回填，避免脏数据入列）。
pub fn label_from_xmp(value: &str) -> Option<&'static str> {
    label_to_xmp(value)
}

/// XMP 评分投影（用户定案 2026-09-27）：DB 真值 → 边车值。
/// `rejected ? -1 : rating`（星级保留在 DB，边车显示 -1 表示拒绝）。
/// 写入侧统一入口（评分 IPC 与库扫描的边车补写共用）。
pub fn projected_rating(rating: i64, rejected: bool) -> i8 {
    if rejected {
        -1
    } else {
        rating.clamp(0, 5) as i8
    }
}

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

/// 解析子元素形态颜色标签（`<xmp:Label>Red</xmp:Label>`；同 read_rating_element
/// 手法，按 local name "Label" 捕获）。
fn read_label_element(xmp_text: &str) -> Option<String> {
    let mut reader = Reader::from_str(xmp_text);
    reader.config_mut().trim_text(true);
    let mut capture = false;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                capture = e.name().local_name().as_ref() == "Label";
            }
            Ok(Event::Text(ref t)) if capture => {
                // BytesText: Deref<Target = str>（0.42）；LR 色名为纯词无转义
                let text = t.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
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

/// 读取边车颜色标签（原始值，大小写保留；归一由 [`label_from_xmp`] 做）。
/// 认 rdf:Description 的 `xmp:Label` 属性与 `<xmp:Label>` 子元素两种形态。
pub fn read_label(xmp_text: &str) -> Option<String> {
    let mut reader = Reader::from_str(xmp_text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                if e.name().local_name().as_ref() != "Description" {
                    continue;
                }
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == "xmp:Label" {
                        if let Ok(value) = attr.normalized_value(quick_xml::XmlVersion::default()) {
                            if !value.trim().is_empty() {
                                return Some(value.trim().to_string());
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    read_label_element(xmp_text)
}

/// 读取边车颜色标签的完整入口（属性 + 子元素两种形态，归一为小写 token）。
pub fn sidecar_label(xmp_text: &str) -> Option<&'static str> {
    read_label(xmp_text).and_then(|raw| label_from_xmp(&raw))
}

/// 读取边车关键字（`dc:subject` 的 `rdf:Bag` 列表，LR 原生形态）——读入
/// 方向（LR → Photographer，§三/§六「关键字从 XMP 边车读入」），与写方向
/// [`write_keywords`] 对称。按结构捕获（同 read_rating_element 手法，按
/// local name 捕获，命名空间声明可在任意祖先上）：进入 `subject` 子树后
/// 收集 `li` 元素文本（Bag/Seq/Alt 容器不敏感），实体反转义、trim、去空、
/// 去重（保序首见）。无 `dc:subject` / 结构损坏 / 自闭合空元素 → 空表。
pub fn sidecar_subject(xmp_text: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xmp_text);
    reader.config_mut().trim_text(true);
    let mut in_subject = false;
    let mut capture_li = false;
    let mut buf = Vec::new();
    let mut li_text = String::new();
    let mut keywords: Vec<String> = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                if e.name().local_name().as_ref() == "subject" {
                    in_subject = true;
                } else if in_subject && e.name().local_name().as_ref() == "li" {
                    capture_li = true;
                    li_text.clear();
                }
            }
            // 0.42 把内容里的实体引用拆成独立事件：li 文本须跨事件累积
            //（Text 与 GeneralRef 交替），收尾一次性反转义
            Ok(Event::Text(ref t)) if capture_li => li_text.push_str(t),
            Ok(Event::GeneralRef(ref r)) if capture_li => {
                li_text.push('&');
                li_text.push_str(r);
                li_text.push(';');
            }
            Ok(Event::End(ref e)) => {
                if e.name().local_name().as_ref() == "subject" {
                    in_subject = false;
                } else if e.name().local_name().as_ref() == "li" {
                    // BytesText 实体已还原（写方向转义过的 &<> 回来）
                    if let Ok(text) = quick_xml::escape::unescape(&li_text) {
                        let text = text.trim();
                        if !text.is_empty() && !keywords.iter().any(|k| k == text) {
                            keywords.push(text.to_string());
                        }
                    }
                    capture_li = false;
                    li_text.clear();
                }
            }
            Ok(Event::Eof) => break,
            Ok(_) => {}
            Err(_) => return Vec::new(), // 结构损坏：当作无关键字
        }
        buf.clear();
    }
    keywords
}

/// 最小边车模板（新建路径）：LR 可直接读的 xmp:Rating。
fn minimal_template(rating: i8) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Photographer">
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
/// `rating` 接受 -1..=5（-1 = 拒绝投影，见模块注释）；越界值原样写入，
/// 由调用方（IPC 层投影函数）保证域。
/// 三条路径，全部字节级保留未触内容：
/// 1. 已有 `xmp:Rating="N"` 属性 → 原位替换数字；
/// 2. 已有 `<xmp:Rating>N</xmp:Rating>` 子元素 → 原位替换数字；
/// 3. 都没有 → 在首个 `<rdf:Description` 开标签插入属性（缺 xmlns:xmp
///    声明一并补；无 rdf:Description 时退回整档最小模板——内容太少说明
///    不是有效 XMP，不冒险拼接）。
pub fn write_rating(existing: Option<&str>, rating: i8) -> String {
    existing
        .and_then(|text| write_field(text, "Rating", Some(&rating.to_string())))
        .unwrap_or_else(|| minimal_template(rating))
}

/// 同步评分到边车（读已有 → 改/建 → 原子写）。返回是否真的落盘。
/// 文件不存在 → 新建最小模板；已存在的其余内容逐字节保留。
pub fn sync_rating_to_sidecar(asset_path: &Path, rating: i8) -> Result<(), String> {
    sync_sidecar(asset_path, |existing| write_rating(existing, rating))
}

/// 在文本中把颜色标签写入/更新/清除为 `xmp:Label`（返回新文本；label None =
/// 清除——LR「无颜色」即无该属性，不是空值）。与 [`write_rating`] 同一套
/// 外科手术策略，全部字节级保留未触内容：
/// 1. 已有 `xmp:Label="Red"` 属性 → 原位替换 / 整属性摘除；
/// 2. 已有 `<xmp:Label>Red</xmp:Label>` 子元素 → 原位替换 / 整元素摘除；
/// 3. 都没有 → 写入时在首个 `<rdf:Description` 开标签插属性（缺 xmlns:xmp
///    声明一并补）；清除时无事可做原样返回。
pub fn write_label(existing: Option<&str>, label: Option<&str>) -> String {
    existing
        .and_then(|text| write_field(text, "Label", label))
        .unwrap_or_else(|| match label {
            Some(label) => minimal_template_label(label),
            None => minimal_template_noop(),
        })
}

/// 只改指定 XMP 字段，保留其他属性、元素和空白的原始字节。
/// 无可插入的 Description 返回 None，由字段入口选择自己的最小模板。
fn write_field(text: &str, field: &str, value: Option<&str>) -> Option<String> {
    let attribute = format!("xmp:{field}=\"");
    if let Some(pos) = text.find(&attribute) {
        let start = pos + attribute.len();
        if let Some(end) = text[start..].find('"').map(|n| start + n) {
            return Some(match value {
                Some(value) => replace_span(text, start, end, value),
                None => replace_span(text, eat_ws_before(text, pos), end + 1, ""),
            });
        }
    }
    let opening = format!("<xmp:{field}>");
    let closing = format!("</xmp:{field}>");
    if let Some(open) = text.find(&opening) {
        let start = open + opening.len();
        if let Some(end) = text[start..].find(&closing).map(|n| start + n) {
            return Some(match value {
                Some(value) => replace_span(text, start, end, value),
                None => replace_span(text, eat_ws_before(text, open), end + closing.len(), ""),
            });
        }
    }
    let Some(value) = value else {
        return Some(text.to_string());
    };
    let tag_start = text.find("<rdf:Description")?;
    let tag_end = tag_start + text[tag_start..].find('>')?;
    let mut insert = String::new();
    if !text[tag_start..tag_end].contains("xmlns:xmp=") {
        insert.push_str(&format!("\n    xmlns:xmp=\"{XMP_NS}\""));
    }
    insert.push_str(&format!("\n    xmp:{field}=\"{value}\""));
    Some(replace_span(text, tag_end, tag_end, &insert))
}

/// 摘除属性/元素时，把 span 起点前的连续空白（含换行）一并吃掉——保持
/// 摘除后 rdf:Description 属性列表的排版不残留孤立缩进行。
fn eat_ws_before(text: &str, mut start: usize) -> usize {
    while start > 0 {
        let prev = text[..start].chars().next_back().unwrap_or(' ');
        if prev.is_whitespace() {
            start -= prev.len_utf8();
        } else {
            break;
        }
    }
    start
}

/// 替换 [from, to) 区间（其余字节原样保留）。
fn replace_span(text: &str, from: usize, to: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(text.len() - (to - from) + replacement.len());
    out.push_str(&text[..from]);
    out.push_str(replacement);
    out.push_str(&text[to..]);
    out
}

/// 无档新建 + 写标签：最小模板（Rating 0 + Label）。
fn minimal_template_label(label: &str) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Photographer">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="{XMP_NS}"
    xmp:Rating="0"
    xmp:Label="{label}"/>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#
    )
}

/// 无档新建 + 清标签：无 Label 字段的最小模板（幂等，无意义写盘由调用方
/// ——IPC 层——在 label 无变化时避免）。
fn minimal_template_noop() -> String {
    minimal_template(0)
}

/// 同步颜色标签到边车（读已有 → 改/建/摘除 → 原子写）。
pub fn sync_label_to_sidecar(asset_path: &Path, label: Option<&str>) -> Result<(), String> {
    sync_sidecar(asset_path, |existing| write_label(existing, label))
}

// ---------------------------------------------------------------------------
// 关键字（dc:subject）与导出边车（M6 §六，Photographer → LR 互操作）
// ---------------------------------------------------------------------------

/// `dc:` 命名空间（关键字容器）。
const DC_NS: &str = "http://purl.org/dc/elements/1.1/";

/// XML 文本转义（关键字是用户输入，`&<>` 必须转义；与 edit::meta 同规则）。
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// 关键字块（LR 原生形态：`dc:subject` 子元素 + `rdf:Bag` 列表）。
/// 空关键字返回 None（清除语义——无字段即无关键字，不写空 Bag）。
fn keywords_block(keywords: &[String]) -> Option<String> {
    if keywords.is_empty() {
        return None;
    }
    let mut block = String::from("<dc:subject><rdf:Bag>");
    for keyword in keywords {
        block.push_str("<rdf:li>");
        block.push_str(&xml_escape(keyword));
        block.push_str("</rdf:li>");
    }
    block.push_str("</rdf:Bag></dc:subject>");
    Some(block)
}

/// 无档新建 + 写关键字：最小模板（Rating 0 + dc:subject）。
fn minimal_template_keywords(keywords: &[String]) -> String {
    format!(
        r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="Photographer">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:xmp="{XMP_NS}"
    xmlns:dc="{DC_NS}"
    xmp:Rating="0">
   {}
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#,
        keywords_block(keywords).unwrap_or_default()
    )
}

/// 在文本中把关键字写入/更新/清除为 `dc:subject` 子元素（返回新文本）。
/// 与 [`write_rating`]/[`write_label`] 同一套外科手术策略，字节级保留
/// 未触内容：
/// 1. 已有 `<dc:subject>…</dc:subject>`（或自闭合 `<dc:subject/>`）→
///    整块原位替换 / 摘除；
/// 2. 都没有 → 写入时在首个 `rdf:Description` 开标签后插子元素（自闭合
///    形态先展开为开闭对；缺 `xmlns:dc` 声明一并补）；清除时原样返回；
/// 3. 无 `rdf:Description` → None（由调用方选择最小模板）。
pub fn write_keywords(existing: Option<&str>, keywords: &[String]) -> String {
    existing
        .and_then(|text| splice_keywords(text, keywords))
        .unwrap_or_else(|| minimal_template_keywords(keywords))
}

/// [`write_keywords`] 的文本内核（无可插入锚点时 None）。
fn splice_keywords(text: &str, keywords: &[String]) -> Option<String> {
    let block = keywords_block(keywords);
    // ① 已有 dc:subject（开闭对或自闭合）：整块替换 / 摘除。
    // 元素名后必须是标签结束符，防误匹配 <dc:subjectx> 之类的撞名前缀。
    let existing_subject = text.find("<dc:subject").filter(|&open| {
        text[open + "<dc:subject".len()..]
            .chars()
            .next()
            .is_some_and(|c| c == '>' || c == '/' || c.is_whitespace())
    });
    if let Some(open) = existing_subject {
        let end = text[open..]
            .find("</dc:subject>")
            .map(|n| open + n + "</dc:subject>".len())
            .or_else(|| text[open..].find("/>").map(|n| open + n + "/>".len()))?;
        return Some(match block {
            Some(block) => replace_span(text, open, end, &block),
            None => replace_span(text, eat_ws_before(text, open), end, ""),
        });
    }
    // ② 插入首个 rdf:Description：整段开标签一次替换（自闭合先展开为
    //    开闭对，xmlns:dc 缺省补在属性列表尾；单次替换无坐标漂移）。
    let Some(block) = block else {
        return Some(text.to_string());
    };
    let tag_start = text.find("<rdf:Description")?;
    let tag_body_end = tag_start + text[tag_start..].find('>')?;
    let self_closed = text[..tag_body_end].ends_with('/');
    let opening = &text[tag_start..=tag_body_end];
    let mut new_opening = if self_closed {
        format!("{}>", &opening[..opening.len() - 2])
    } else {
        opening.to_string()
    };
    if !new_opening.contains("xmlns:dc=") {
        new_opening.insert_str(new_opening.len() - 1, &format!("\n    xmlns:dc=\"{DC_NS}\""));
    }
    let replacement = if self_closed {
        format!("{new_opening}\n   {block}\n  </rdf:Description>")
    } else {
        format!("{new_opening}\n   {block}")
    };
    Some(replace_span(text, tag_start, tag_body_end + 1, &replacement))
}

/// 导出边车组合写入（M6 §六「全量 XMP 边车」）：星级/颜色/关键字三字段
/// 一次落定。基底文本（源资产旁已有边车，含 LR 开发设置等）逐字段外科
/// 手术覆写——库内 DB 为真值；无基底时新建三字段齐全的最小模板。
pub fn write_export_fields(
    existing: Option<&str>,
    rating: i8,
    label: Option<&str>,
    keywords: &[String],
) -> String {
    let rated = write_rating(existing, rating);
    let labeled = write_label(Some(&rated), label);
    write_keywords(Some(&labeled), keywords)
}

/// 导出边车原子落盘（`.tmp` + rename；独立文件——绝不硬链接，LR 侧写回
/// 不得波及库内）。
pub fn write_export_sidecar(
    dst_body: &Path,
    base_text: Option<&str>,
    rating: i8,
    label: Option<&str>,
    keywords: &[String],
) -> Result<(), String> {
    let sidecar = sidecar_path(dst_body);
    let content = write_export_fields(base_text, rating, label, keywords);
    atomic_write_sidecar(&sidecar, &content)
}

/// 公用边车读改写流程；字段入口只提供保留字节的改写函数。
fn sync_sidecar(
    asset_path: &Path,
    update: impl FnOnce(Option<&str>) -> String,
) -> Result<(), String> {
    // 评分、颜色与拒绝状态由不同后台任务提交。串行化读改写，避免两个
    // 任务同时读取旧边车后互相覆盖对方的字段。
    static WRITE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _guard = WRITE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let sidecar = sidecar_path(asset_path);
    let existing = std::fs::read_to_string(&sidecar).ok();
    atomic_write_sidecar(&sidecar, &update(existing.as_deref()))
}

/// 边车原子落盘（.tmp + rename；rename 失败清残留 .tmp）。
fn atomic_write_sidecar(sidecar: &Path, content: &str) -> Result<(), String> {
    let tmp = sidecar.with_extension("xmp.tmp");
    std::fs::write(&tmp, content).map_err(|e| format!("写边车失败 {}: {e}", sidecar.display()))?;
    std::fs::rename(&tmp, sidecar).map_err(|e| {
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
    fn rating_and_label_updates_preserve_each_other_and_clear_only_label() {
        let cases = [
            (
                r#"<rdf:Description xmp:Rating="2" xmp:Label="Blue" crs:Tone="保留"/>"#,
                r#"<rdf:Description xmp:Rating="4" xmp:Label="Blue" crs:Tone="保留"/>"#,
                r#"<rdf:Description xmp:Rating="2" xmp:Label="Red" crs:Tone="保留"/>"#,
                r#"<rdf:Description xmp:Rating="2" crs:Tone="保留"/>"#,
            ),
            (
                "<rdf:Description><xmp:Rating>2</xmp:Rating>\n　<xmp:Label>Blue</xmp:Label><crs:Tone>保留</crs:Tone></rdf:Description>",
                "<rdf:Description><xmp:Rating>4</xmp:Rating>\n　<xmp:Label>Blue</xmp:Label><crs:Tone>保留</crs:Tone></rdf:Description>",
                "<rdf:Description><xmp:Rating>2</xmp:Rating>\n　<xmp:Label>Red</xmp:Label><crs:Tone>保留</crs:Tone></rdf:Description>",
                "<rdf:Description><xmp:Rating>2</xmp:Rating><crs:Tone>保留</crs:Tone></rdf:Description>",
            ),
        ];
        for (source, rated, labeled, cleared) in cases {
            assert_eq!(write_rating(Some(source), 4), rated);
            assert_eq!(write_label(Some(source), Some("Red")), labeled);
            assert_eq!(write_label(Some(source), None), cleared);
            assert_eq!(write_label(Some(cleared), None), cleared);
        }
        let incomplete = "<rdf:Description";
        assert_eq!(write_rating(Some(incomplete), 4), minimal_template(4));
        assert_eq!(
            write_label(Some(incomplete), Some("Red")),
            minimal_template_label("Red")
        );
        assert_eq!(write_label(Some(incomplete), None), incomplete);
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

        // -1（拒绝投影，用户定案）：写入路径 + 读取侧不认（拒绝是应用内
        // 状态，exif 通道不回填 DB）
        let rejected = write_rating(Some(lr), -1);
        assert!(rejected.contains(r#"xmp:Rating="-1""#));
        assert!(rejected.contains("crs:Sharpness"), "其余字节保留");
        assert_eq!(sidecar_rating(&rejected), None);

        // 拒绝 → 取消：-1 原位替换回星级
        let restored = write_rating(Some(&rejected), 4);
        assert!(restored.contains(r#"xmp:Rating="4""#));
        assert!(!restored.contains(r#"-1"#));
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

    // —— 关键字（dc:subject）与导出边车组合（M6 §六）——

    /// LR 写法：已有 dc:subject 整块替换；其余字节（crs: 开发设置）保留。
    #[test]
    fn write_keywords_replaces_existing_subject_and_preserves_bytes() {
        let lr = r#"<?xpacket begin=""?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/" xmp:Rating="4">
   <dc:subject><rdf:Bag><rdf:li>old</rdf:li></rdf:Bag></dc:subject>
   <crs:Look><rdf:Bag><rdf:li>Punch</rdf:li></rdf:Bag></crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;
        let out = write_keywords(Some(lr), &["旅行".into(), "a&b<c".into()]);
        assert!(out.contains("<dc:subject><rdf:Bag><rdf:li>旅行</rdf:li><rdf:li>a&amp;b&lt;c</rdf:li></rdf:Bag></dc:subject>"));
        assert!(!out.contains("<rdf:li>old</rdf:li>"));
        assert!(out.contains("<rdf:li>Punch</rdf:li>"), "crs 字节保留");
        assert_eq!(out.matches("xmlns:dc=").count(), 1, "不重复声明");
        // 清除：整块摘除
        let cleared = write_keywords(Some(&out), &[]);
        assert!(!cleared.contains("dc:subject"));
        assert!(cleared.contains("<rdf:li>Punch</rdf:li>"));
    }

    /// 无 dc:subject：插入首个 rdf:Description（自闭合形态先展开 + 补
    /// xmlns:dc 声明）；已有声明不重复。
    #[test]
    fn write_keywords_inserts_into_first_description() {
        let self_closed = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="2"/></rdf:RDF>"#;
        let out = write_keywords(Some(self_closed), &["red".into()]);
        assert!(out.contains("</rdf:Description>"), "自闭合展开为开闭对");
        assert!(out.contains(r#"xmp:Rating="2""#));
        assert!(out.contains("xmlns:dc=\"http://purl.org/dc/elements/1.1/\""));
        assert!(out.contains("<rdf:li>red</rdf:li>"));

        let with_dc = r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>hi</dc:title></rdf:Description>"#;
        let out = write_keywords(Some(with_dc), &["k".into()]);
        assert_eq!(out.matches("xmlns:dc=").count(), 1, "已有声明不重复");
        assert!(out.contains("<dc:title>hi</dc:title>"));
        assert!(out.contains("<dc:subject>"));

        // 无档 → 最小模板
        let fresh = write_keywords(None, &["x".into(), "y".into()]);
        assert!(fresh.contains("x:xmpmeta"));
        assert!(fresh.contains("<rdf:li>x</rdf:li>"));
        // 无锚点残文 → 最小模板兜底
        assert_eq!(
            write_keywords(Some("<rdf:Description"), &["x".into()]),
            minimal_template_keywords(&["x".to_string()])
        );
    }

    /// 导出边车组合：基底（LR 边车）覆写三字段 + 无档新建三字段齐全。
    #[test]
    fn write_export_fields_composes_rating_label_keywords() {
        let base = r#"<rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="1" xmp:Label="Blue"/></rdf:RDF>"#;
        let out = write_export_fields(Some(base), 5, Some("Red"), &["wedding".into()]);
        assert!(out.contains(r#"xmp:Rating="5""#));
        assert!(out.contains(r#"xmp:Label="Red""#));
        assert!(!out.contains("Blue"));
        assert!(out.contains("<rdf:li>wedding</rdf:li>"));

        let fresh = write_export_fields(None, -1, None, &[]);
        assert!(fresh.contains(r#"xmp:Rating="-1""#), "拒绝投影");
        assert!(!fresh.contains("xmp:Label"));
        assert!(!fresh.contains("dc:subject"), "空关键字不写字段");

        let fresh = write_export_fields(None, 3, Some("Green"), &["a&b".into()]);
        assert!(fresh.contains(r#"xmp:Rating="3""#));
        assert!(fresh.contains(r#"xmp:Label="Green""#));
        assert!(fresh.contains("<rdf:li>a&amp;b</rdf:li>"));
    }

    /// 导出边车落盘：目标旁生成同名 .xmp，内容可被读取侧 roundtrip。
    #[test]
    fn write_export_sidecar_lands_next_to_body() {
        let dir = tempfile::tempdir().unwrap();
        let dst = dir.path().join("DSC_0001.NEF");
        std::fs::write(&dst, b"nef").unwrap();
        write_export_sidecar(&dst, None, 4, Some("Yellow"), &["keep".into()]).unwrap();
        let sidecar = sidecar_path(&dst);
        let text = std::fs::read_to_string(&sidecar).unwrap();
        assert_eq!(sidecar_rating(&text), Some(4));
        // sidecar_label 返回 LR 标准色名形态（首字母大写，与写入值一致）
        assert_eq!(sidecar_label(&text), Some("Yellow"));
        assert!(text.contains("<rdf:li>keep</rdf:li>"));
    }

    // —— 关键字读入（dc:subject，LR → Photographer 方向，§三/§六）——

    /// LR 原生形态：转义反转义、trim/去空、去重（保序首见）；subject 外
    /// 的同级 Bag（crs:Look 等）不串扰。
    #[test]
    fn sidecar_subject_reads_lr_bag_with_unescape() {
        let lr = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
   <dc:subject><rdf:Bag><rdf:li>旅行</rdf:li><rdf:li>a&amp;b&lt;c</rdf:li><rdf:li>  </rdf:li><rdf:li>旅行</rdf:li></rdf:Bag></dc:subject>
   <crs:Look><rdf:Bag><rdf:li>Punch</rdf:li></rdf:Bag></crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
        assert_eq!(
            sidecar_subject(lr),
            vec!["旅行".to_string(), "a&b<c".to_string()],
            "转义反转义 + 去空 + 去重，crs 字段不串扰"
        );
    }

    /// 无 dc:subject / 自闭合空元素 / 结构损坏 → 空表（不误读后续内容）。
    #[test]
    fn sidecar_subject_absent_or_broken_is_empty() {
        assert!(sidecar_subject(r#"<rdf:Description rdf:about=""/>"#).is_empty());
        // 自闭合 <dc:subject/>：元素无内容，且不得把后续字段吞进关键字
        let self_closed = r#"<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:subject/><dc:title>hi</dc:title></rdf:Description>"#;
        assert!(sidecar_subject(self_closed).is_empty());
        assert!(sidecar_subject("not xml at all <<<").is_empty());
        // subject 子树外（闭后）的 li 不采集
        let after = r#"<rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:subject><rdf:Bag><rdf:li>k</rdf:li></rdf:Bag></dc:subject><dc:other><rdf:Bag><rdf:li>late</rdf:li></rdf:Bag></dc:other></rdf:Description>"#;
        assert_eq!(sidecar_subject(after), vec!["k".to_string()]);
    }

    /// 与写方向 roundtrip：write_keywords 产物读回等值（双向对称钉死）。
    #[test]
    fn sidecar_subject_roundtrips_write_keywords() {
        let base = r#"<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"/>"#;
        let keywords = vec!["wedding".to_string(), "桑&迪".to_string()];
        let out = write_keywords(Some(base), &keywords);
        assert_eq!(sidecar_subject(&out), keywords);
        // 清除后读回空
        let cleared = write_keywords(Some(&out), &[]);
        assert!(sidecar_subject(&cleared).is_empty());
    }
}
