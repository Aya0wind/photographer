//! EXIF-lite 解析测试（spec §5.3 / M1 T6）。
//!
//! lib.rs 中 `mod metadata;` 为私有模块（不可改），此处经 `#[path]` 将源文件
//! 直接编译进测试 crate。fixture 为测试内手写 builder 构造的最小合法
//! TIFF+EXIF 字节（II*\0 + IFD0 Make/Model[/DateTime] + ExifIFD 指针 +
//! DateTimeOriginal），外加 JPEG SOI+APP1 包装变体。

use std::fs;
use std::io::Read;
use std::path::Path;

use chrono::NaiveDateTime;

#[path = "../src/metadata/exif_lite.rs"]
mod exif_lite;

use exif_lite::{parse, MetaLite};

/// ASCII 字符串值（含结尾 NUL，count 含 NUL）。
fn ascii(data: &str) -> Vec<u8> {
    let mut v = data.as_bytes().to_vec();
    v.push(0);
    v
}

/// 追加一个 ASCII 类型 IFD 条目。TIFF 规范：值 ≤4 字节内联存于值字段（左对齐
/// 零填充），否则值字段为数据偏移。
fn push_ascii_entry(buf: &mut Vec<u8>, tag: u16, data_with_nul: &[u8], offset: usize) {
    buf.extend_from_slice(&tag.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes()); // 类型 ASCII
    buf.extend_from_slice(&(data_with_nul.len() as u32).to_le_bytes());
    if data_with_nul.len() <= 4 {
        let mut inline = [0u8; 4];
        inline[..data_with_nul.len()].copy_from_slice(data_with_nul);
        buf.extend_from_slice(&inline);
    } else {
        buf.extend_from_slice(&(offset as u32).to_le_bytes());
    }
}

/// 构造最小合法 TIFF+EXIF（小端 II*\0）：
/// 头(8) + IFD0[Make(0x010F), Model(0x0110), 可选 DateTime(0x0132), 可选
/// ExifIFD 指针(0x8769)] + 字符串数据区 + 可选 ExifIFD[DateTimeOriginal(0x9003)]。
fn build_tiff(make: &str, model: &str, dto: Option<&str>, dt: Option<&str>) -> Vec<u8> {
    let make_b = ascii(make);
    let model_b = ascii(model);
    let dt_b = dt.map(ascii);
    let dto_b = dto.map(ascii);
    let has_exif = dto_b.is_some();

    let n0: usize = 2 + dt_b.is_some() as usize + has_exif as usize;
    let make_off = 8 + 2 + 12 * n0 + 4; // IFD0 紧随 8 字节头
    let model_off = make_off + make_b.len();
    let dt_off = model_off + model_b.len();
    let mut end = dt_off + dt_b.as_ref().map_or(0, Vec::len);
    let (exif_ifd_off, dto_off) = if has_exif {
        let ifd_off = end;
        end += 2 + 12 + 4; // 条目数 + 单条目 + 下一 IFD 指针
        (ifd_off, end)
    } else {
        (0, end)
    };

    let mut buf = Vec::with_capacity(end + dto_b.as_ref().map_or(0, Vec::len));
    buf.extend_from_slice(b"II\x2a\x00");
    buf.extend_from_slice(&8u32.to_le_bytes()); // IFD0 偏移
    buf.extend_from_slice(&(n0 as u16).to_le_bytes());
    // 条目按 tag 升序：0x010F < 0x0110 < 0x0132 < 0x8769
    push_ascii_entry(&mut buf, 0x010F, &make_b, make_off);
    push_ascii_entry(&mut buf, 0x0110, &model_b, model_off);
    if let Some(b) = &dt_b {
        push_ascii_entry(&mut buf, 0x0132, b, dt_off);
    }
    if has_exif {
        buf.extend_from_slice(&0x8769u16.to_le_bytes());
        buf.extend_from_slice(&4u16.to_le_bytes()); // 类型 LONG
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&(exif_ifd_off as u32).to_le_bytes());
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // 下一 IFD = 0
    buf.extend_from_slice(&make_b);
    buf.extend_from_slice(&model_b);
    if let Some(b) = &dt_b {
        buf.extend_from_slice(b);
    }
    if has_exif {
        buf.extend_from_slice(&1u16.to_le_bytes());
        push_ascii_entry(&mut buf, 0x9003, dto_b.as_ref().unwrap(), dto_off);
        buf.extend_from_slice(&0u32.to_le_bytes());
        buf.extend_from_slice(dto_b.as_ref().unwrap());
    }
    assert_eq!(buf.len(), end + dto_b.as_ref().map_or(0, Vec::len));
    buf
}

/// 把 TIFF 字节包进 JPEG 容器：SOI + APP1("Exif\0\0" + TIFF)。段长为大端。
fn wrap_jpeg(tiff: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(tiff.len() + 12);
    buf.extend_from_slice(&[0xFF, 0xD8]); // SOI
    buf.extend_from_slice(&[0xFF, 0xE1]); // APP1
    buf.extend_from_slice(&((2 + 6 + tiff.len()) as u16).to_be_bytes());
    buf.extend_from_slice(b"Exif\x00\x00");
    buf.extend_from_slice(tiff);
    buf
}

fn naive(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
}

#[test]
fn parses_tiff_datetime_original_and_camera() {
    let bytes = build_tiff("TestCam", "Model X", Some("2026:01:02 03:04:05"), None);
    let meta = parse(&bytes);
    assert_eq!(meta.captured_at, Some(naive(2026, 1, 2, 3, 4, 5)));
    assert_eq!(meta.camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn parses_jpeg_app1_container() {
    let tiff = build_tiff("TestCam", "Model X", Some("2024:12:31 23:59:59"), None);
    let meta = parse(&wrap_jpeg(&tiff));
    assert_eq!(meta.captured_at, Some(naive(2024, 12, 31, 23, 59, 59)));
    assert_eq!(meta.camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn collapses_camera_whitespace_and_trims() {
    let bytes = build_tiff(
        "  TestCam  ",
        " Model X ",
        Some("2026:01:02 03:04:05"),
        None,
    );
    assert_eq!(parse(&bytes).camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn falls_back_to_datetime_tag_without_exif_ifd() {
    let bytes = build_tiff("TestCam", "Model X", None, Some("2019:12:31 23:59:59"));
    let meta = parse(&bytes);
    assert_eq!(meta.captured_at, Some(naive(2019, 12, 31, 23, 59, 59)));
    assert_eq!(meta.camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn camera_partial_make_or_model() {
    let bytes = build_tiff("TestCam", "", Some("2026:01:02 03:04:05"), None);
    assert_eq!(parse(&bytes).camera.as_deref(), Some("TestCam"));
}

#[test]
fn bad_datetime_format_yields_none_camera_still_parsed() {
    // DateTimeOriginal 值格式非法 → captured_at=None，但 camera 正常
    let bytes = build_tiff("TestCam", "Model X", Some("not a date"), None);
    let meta = parse(&bytes);
    assert_eq!(meta.captured_at, None);
    assert_eq!(meta.camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn garbage_empty_truncated_input_yields_all_none() {
    assert_eq!(parse(&[]), MetaLite::default());
    assert_eq!(
        parse(b"definitely not an image, just text"),
        MetaLite::default()
    );
    // 随机字节（含恰好出现的幻数也无 TIFF 结构）
    assert_eq!(
        parse(&(0..=255u8).collect::<Vec<u8>>()),
        MetaLite::default()
    );
    // 合法 TIFF 的截断头部
    let good = build_tiff("A", "B", Some("2026:01:02 03:04:05"), None);
    assert_eq!(parse(&good[..10]), MetaLite::default());
    // ExifIFD 指针越界（指向文件末尾之外）：gen-5 容错解析下单字段越界
    // 只丢该子树——IFD0 的 Make/Model 照常返回（旧严格模式全 None）
    let mut broken = good.clone();
    let ptr_value_at = 8 + 2 + 12 * 2 + 8; // IFD0 内第 3 条（ExifIFD 指针）的值偏移
    broken[ptr_value_at..ptr_value_at + 4].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
    let meta = parse(&broken);
    assert_eq!(meta.camera.as_deref(), Some("A B"));
    assert_eq!(meta.captured_at, None); // EXIF 子树丢失：DateTimeOriginal 不可达
}

/// gen-5 容错韧性：TIFF 条目值落在截取窗之外（RAW 的条带数据/MakerNote
/// 超 1MB head 场景）只丢该字段，其余照常解析（严格模式会全 None——
/// 尼康/适马等大 MakerNote 的 RAW 最易触发）。
#[test]
fn truncated_out_of_window_field_does_not_kill_exif() {
    // 手工小 TIFF：IFD0[Make(值在缓冲内), 巨型条目(值偏移 4000 超出缓冲)]
    let mut buf = Vec::new();
    buf.extend_from_slice(b"II\x2a\x00");
    buf.extend_from_slice(&8u32.to_le_bytes()); // IFD0 偏移
    buf.extend_from_slice(&2u16.to_le_bytes()); // 2 条目
                                                // 条目 1：Make(0x010F) ASCII "Nikon\0" → 值偏移 38（IFD0 止于 38）
    buf.extend_from_slice(&0x010Fu16.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&6u32.to_le_bytes());
    buf.extend_from_slice(&38u32.to_le_bytes());
    // 条目 2：0x0201 BYTE×8000 → 值偏移 4000（缓冲只有 44 字节）
    buf.extend_from_slice(&0x0201u16.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());
    buf.extend_from_slice(&8000u32.to_le_bytes());
    buf.extend_from_slice(&4000u32.to_le_bytes());
    buf.extend_from_slice(&0u32.to_le_bytes()); // 下一 IFD = 0
    buf.extend_from_slice(b"Nikon\0"); // 38..44
    assert_eq!(buf.len(), 44);
    let meta = parse(&buf);
    assert_eq!(meta.camera.as_deref(), Some("Nikon"), "越界字段只丢自己");
}

/// 真实文件冒烟（直连已知样例路径，不做目录遍历——真实数据测试禁止全量扫描，
/// 只读头部 1MB 渐进解析；样例缺失时直接跳过）。
#[test]
#[ignore = "依赖本机 Y:\\照片 真实照片，需 --ignored 手动运行"]
fn real_jpeg_head_smoke() {
    // 已知样例（2026-09-18 实测存在）；如换机后失效，更新为任一已知 jpg 全路径即可
    const SAMPLE: &str = r"Y:\照片\20250607团建\DSC_0176.JPG";
    let path = Path::new(SAMPLE);
    if !path.is_file() {
        eprintln!("skip: 样例不存在 {}", SAMPLE);
        return;
    }
    let mut head = Vec::new();
    fs::File::open(path)
        .expect("failed to open photo")
        .take(1024 * 1024)
        .read_to_end(&mut head)
        .expect("failed to read head");
    let meta = parse(&head); // 不 panic 即通过
    eprintln!(
        "{} -> captured_at={:?} camera={:?}",
        path.display(),
        meta.captured_at.map(|t| t.to_rfc3339()),
        meta.camera
    );
    // 可观察性断言：真实相机 JPG 至少应解析出二者之一
    assert!(meta.captured_at.is_some() || meta.camera.is_some());
}

/// 编译期锚点：确保 chrono 解析格式与实现约定一致（"%Y:%m:%d %H:%M:%S"）。
#[test]
fn chrono_exif_format_reference() {
    let t = NaiveDateTime::parse_from_str("2026:01:02 03:04:05", "%Y:%m:%d %H:%M:%S").unwrap();
    assert_eq!(t.and_utc(), naive(2026, 1, 2, 3, 4, 5));
}

// ---------------------------------------------------------------------------
// 拍摄参数扩展（M3.5）：FNumber/ExposureTime/ISO/FocalLength/LensModel + 宽高
// ---------------------------------------------------------------------------

/// 通用 IFD 条目（payload = 实际数据字节；≤4 内联，否则进数据区）。
struct Ent {
    tag: u16,
    typ: u16,
    payload: Vec<u8>,
}

fn ent(tag: u16, typ: u16, payload: Vec<u8>) -> Ent {
    Ent { tag, typ, payload }
}

fn ascii_val(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

/// 各类型单元素字节数（TIFF 类型 1..12）。
fn unit_len(typ: u16) -> usize {
    match typ {
        3 | 8 => 2,
        4 | 9 | 11 => 4,
        5 | 10 | 12 => 8,
        _ => 1, // 1/2/6/7 及未知
    }
}

/// 小端 IFD 段编码：entries 升序；返回 (ifd 字节, 数据区字节)。
/// count = payload.len() / unit_len(typ)（ASCII 按字节，数值类型按元素）。
fn encode_ifd(entries: &[Ent], data_base: usize) -> (Vec<u8>, Vec<u8>) {
    let mut ifd = Vec::new();
    let mut data = Vec::new();
    ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for e in entries {
        let count = (e.payload.len() / unit_len(e.typ)).max(1);
        ifd.extend_from_slice(&e.tag.to_le_bytes());
        ifd.extend_from_slice(&e.typ.to_le_bytes());
        ifd.extend_from_slice(&(count as u32).to_le_bytes());
        if e.payload.len() <= 4 {
            let mut inline = [0u8; 4];
            inline[..e.payload.len()].copy_from_slice(&e.payload);
            ifd.extend_from_slice(&inline);
        } else {
            ifd.extend_from_slice(&((data_base + data.len()) as u32).to_le_bytes());
            data.extend_from_slice(&e.payload);
        }
    }
    ifd.extend_from_slice(&0u32.to_le_bytes()); // 下一 IFD = 0
    (ifd, data)
}

/// 全功能 TIFF 构造：IFD0[Make, Model, (ImageWidth 0x0100), (ImageLength
/// 0x0101), ExifIFD 指针] + ExifIFD[ExposureTime, FNumber, ISO,
/// DateTimeOriginal, FocalLength, LensModel]。
#[allow(clippy::too_many_arguments)]
fn build_full_tiff(
    make: Option<&str>,
    model: Option<&str>,
    dims: Option<(u32, u32)>,
    exposure: Option<(u32, u32)>,
    fnumber: Option<(u32, u32)>,
    iso: Option<u16>,
    dto: Option<&str>,
    focal: Option<(u32, u32)>,
    lens: Option<&str>,
) -> Vec<u8> {
    let mut ifd0 = Vec::new();
    if let Some(m) = make {
        ifd0.push(ent(0x010F, 2, ascii_val(m)));
    }
    if let Some(m) = model {
        ifd0.push(ent(0x0110, 2, ascii_val(m)));
    }
    if let Some((w, h)) = dims {
        ifd0.push(ent(0x0100, 4, w.to_le_bytes().to_vec()));
        ifd0.push(ent(0x0101, 4, h.to_le_bytes().to_vec()));
    }
    let mut exif_ents = Vec::new();
    if let Some((n, d)) = exposure {
        let mut p = Vec::new();
        p.extend_from_slice(&n.to_le_bytes());
        p.extend_from_slice(&d.to_le_bytes());
        exif_ents.push(ent(0x829A, 5, p));
    }
    if let Some((n, d)) = fnumber {
        let mut p = Vec::new();
        p.extend_from_slice(&n.to_le_bytes());
        p.extend_from_slice(&d.to_le_bytes());
        exif_ents.push(ent(0x829D, 5, p));
    }
    if let Some(v) = iso {
        exif_ents.push(ent(0x8827, 3, v.to_le_bytes().to_vec()));
    }
    if let Some(s) = dto {
        exif_ents.push(ent(0x9003, 2, ascii_val(s)));
    }
    if let Some((n, d)) = focal {
        let mut p = Vec::new();
        p.extend_from_slice(&n.to_le_bytes());
        p.extend_from_slice(&d.to_le_bytes());
        exif_ents.push(ent(0x920A, 5, p));
    }
    if let Some(s) = lens {
        exif_ents.push(ent(0xA434, 2, ascii_val(s)));
    }
    exif_ents.sort_by_key(|e| e.tag);

    let n0 = ifd0.len() + 1; // + ExifIFD 指针
    let ifd0_size = 2 + 12 * n0 + 4;
    let exif_off = 8 + ifd0_size;
    let exif_size = 2 + 12 * exif_ents.len() + 4;
    let ifd0_data_base = exif_off + exif_size;

    // ExifIFD 指针作为普通条目并入 IFD0（encode_ifd 输出含自身计数头）
    ifd0.push(ent(0x8769, 4, (exif_off as u32).to_le_bytes().to_vec()));
    ifd0.sort_by_key(|e| e.tag);

    let (ifd0_bytes, ifd0_data) = encode_ifd(&ifd0, ifd0_data_base);
    let exif_data_base = ifd0_data_base + ifd0_data.len();
    let (exif_bytes, exif_data) = encode_ifd(&exif_ents, exif_data_base);

    let mut buf = Vec::new();
    buf.extend_from_slice(b"II\x2a\x00");
    buf.extend_from_slice(&8u32.to_le_bytes());
    buf.extend_from_slice(&ifd0_bytes);
    buf.extend_from_slice(&exif_bytes);
    buf.extend_from_slice(&ifd0_data);
    buf.extend_from_slice(&exif_data);
    buf
}

/// 最小 JPEG 扫描段（ncomp=3 的 SOF0 + EOI）。
fn sof0_segment(width: u16, height: u16) -> Vec<u8> {
    let mut seg = Vec::new();
    seg.extend_from_slice(&[0xFF, 0xC0]);
    seg.extend_from_slice(&17u16.to_be_bytes()); // len = 8 + 3*3
    seg.push(8); // precision
    seg.extend_from_slice(&height.to_be_bytes());
    seg.extend_from_slice(&width.to_be_bytes());
    seg.push(3);
    seg.extend_from_slice(&[0; 9]);
    seg
}

fn wrap_jpeg_with_sof(tiff: &[u8], sof: Option<(u16, u16)>) -> Vec<u8> {
    let mut buf = wrap_jpeg(tiff);
    if let Some((w, h)) = sof {
        buf.extend_from_slice(&sof0_segment(w, h));
    }
    buf.extend_from_slice(&[0xFF, 0xD9]); // EOI
    buf
}

#[test]
fn parses_exposure_iso_fnumber_focal_lens_and_dimensions() {
    let tiff = build_full_tiff(
        Some("Sony"),
        Some("A7R5"),
        None,
        Some((1, 250)),
        Some((28, 10)),
        Some(1600),
        Some("2026:06:28 15:30:00"),
        Some((85, 1)),
        Some("FE 85mm F1.8"),
    );
    let bytes = wrap_jpeg_with_sof(&tiff, Some((6048, 8064)));
    let meta = parse(&bytes);
    eprintln!("DEBUG meta = {:?}", meta);
    assert_eq!(meta.width, Some(6048));
    assert_eq!(meta.height, Some(8064));
    assert_eq!(meta.iso, Some(1600));
    assert_eq!(meta.f_number.as_deref(), Some("2.8"));
    assert_eq!(meta.exposure_time.as_deref(), Some("1/250"));
    assert_eq!(meta.focal_length.as_deref(), Some("85"));
    assert_eq!(meta.lens.as_deref(), Some("FE 85mm F1.8"));
    // 原有字段不回归
    assert_eq!(meta.captured_at, Some(naive(2026, 6, 28, 15, 30, 0)));
    assert_eq!(meta.camera.as_deref(), Some("Sony A7R5"));
}

#[test]
fn rational_formatting_decimal_and_fraction() {
    // 0.4s（4/10）小数；f/4（40/10）整值去尾零；85.0mm 同
    let tiff = build_full_tiff(
        Some("C"),
        Some("M"),
        None,
        Some((4, 10)),
        Some((40, 10)),
        Some(100),
        None,
        Some((850, 10)),
        None,
    );
    let meta = parse(&wrap_jpeg_with_sof(&tiff, None));
    assert_eq!(meta.exposure_time.as_deref(), Some("0.4"));
    assert_eq!(meta.f_number.as_deref(), Some("4"));
    assert_eq!(meta.focal_length.as_deref(), Some("85"));
    assert_eq!(meta.lens, None);
}

#[test]
fn missing_shooting_fields_tolerated() {
    // 只有 Make/Model：新字段全 None（PartialEq 对 default 的扩展字段）
    let tiff = build_full_tiff(
        Some("TestCam"),
        Some("Model X"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let meta = parse(&wrap_jpeg_with_sof(&tiff, None));
    assert_eq!(meta.width, None);
    assert_eq!(meta.height, None);
    assert_eq!(meta.iso, None);
    assert_eq!(meta.f_number, None);
    assert_eq!(meta.exposure_time, None);
    assert_eq!(meta.focal_length, None);
    assert_eq!(meta.lens, None);
    assert_eq!(meta.camera.as_deref(), Some("TestCam Model X"));
}

#[test]
fn dimensions_from_sof2_progressive_and_from_tiff_tags() {
    // SOF2（渐进式，0xC2）
    let tiff = build_full_tiff(
        Some("C"),
        Some("M"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let mut bytes = wrap_jpeg(tiff.as_slice());
    bytes.extend_from_slice(&[0xFF, 0xC2]);
    bytes.extend_from_slice(&17u16.to_be_bytes());
    bytes.push(8);
    bytes.extend_from_slice(&1080u16.to_be_bytes());
    bytes.extend_from_slice(&1920u16.to_be_bytes());
    bytes.push(3);
    bytes.extend_from_slice(&[0; 9]);
    let meta = parse(&bytes);
    assert_eq!(meta.width, Some(1920));
    assert_eq!(meta.height, Some(1080));

    // TIFF 形态（RAW 容器）：无 SOF，从 IFD0 ImageWidth/ImageLength 取
    let raw = build_full_tiff(
        Some("Nikon"),
        Some("Z8"),
        Some((8256, 5504)),
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let meta = parse(&raw);
    assert_eq!(meta.width, Some(8256));
    assert_eq!(meta.height, Some(5504));
}

/// gen-5 契约：TIFF 基 RAW 的内嵌 preview JPEG 不再作为尺寸源——
/// 旧策略扫 SOI 把缩略图尺寸当本体（NEF 真机 6064×4040 → 640×424）。
#[test]
fn raw_tiff_ignores_embedded_preview_dimensions() {
    // RAW TIFF 头（无任何尺寸层：IFD0 无尺寸、无 EXIF 像素维度、无 SubIFD）
    // + 头部后段的内嵌预览 JPEG（SOI…SOF0 1616×1080…EOI）→ 宁缺毋错
    let mut raw = build_full_tiff(None, None, None, None, None, None, None, None, None);
    raw.extend_from_slice(&[0u8; 512]); // TIFF 数据与预览间填充
    let mut preview = Vec::new();
    preview.extend_from_slice(&[0xFF, 0xD8]);
    preview.extend_from_slice(&sof0_segment(1616, 1080));
    preview.extend_from_slice(&[0xFF, 0xD9]);
    raw.extend_from_slice(&preview);
    let meta = parse(&raw);
    assert_eq!(meta.width, None, "预览 JPEG 的 SOF 不得冒充本体尺寸");
    assert_eq!(meta.height, None);
}

/// 构造 NEF 形态的最小 TIFF（gen-5 真机结构复刻，小端）：
/// IFD0[NewSubfileType=1（缩略图层，无尺寸 tag）， ExifIFD→拍摄参数
/// （ISO；exif_pixel=Some 时另带 0xA002/0xA003）， SubIFD(0x014A)→全尺寸
/// 主图 IFD（NewSubfileType=0 + 6064×4040）] + 尾部内嵌缩略图 JPEG(640×424)。
/// 真机 DSC_0176.NEF（Z5）实测同构：IFD0=缩略图、EXIF 无 0xA002、
/// 全尺寸在 SubIFD#1、IFD0 内嵌 640×424 JPEG（2026-09-21）。
fn build_nef_like(exif_pixel: Option<(u32, u32)>) -> Vec<u8> {
    let mut exif_ents = vec![ent(0x8827, 3, 7200u16.to_le_bytes().to_vec())];
    if let Some((w, h)) = exif_pixel {
        exif_ents.push(ent(0xA002, 4, w.to_le_bytes().to_vec()));
        exif_ents.push(ent(0xA003, 4, h.to_le_bytes().to_vec()));
    }
    exif_ents.sort_by_key(|e| e.tag);
    // 布局：头(8) IFD0(2+3*12+4=42) ExifIFD(2+12n+4) SubIFD(2+3*12+4=42)
    let ifd0_off = 8;
    let ifd0_size = 2 + 12 * 3 + 4;
    let exif_off = ifd0_off + ifd0_size;
    let exif_size = 2 + 12 * exif_ents.len() + 4;
    let sub_off = exif_off + exif_size;

    let mut buf = Vec::new();
    buf.extend_from_slice(b"II\x2a\x00");
    buf.extend_from_slice(&(ifd0_off as u32).to_le_bytes());
    // IFD0 三条目（tag 升序 0x00FE < 0x014A < 0x8769），值全内联
    buf.extend_from_slice(&3u16.to_le_bytes());
    for (tag, value) in [
        (0x00FEu16, 1u32),            // NewSubfileType=1：缩略图层
        (0x014Au16, sub_off as u32),  // SubIFD 指针 → 全尺寸主图 IFD
        (0x8769u16, exif_off as u32), // ExifIFD 指针
    ] {
        buf.extend_from_slice(&tag.to_le_bytes());
        buf.extend_from_slice(&4u16.to_le_bytes()); // LONG
        buf.extend_from_slice(&1u32.to_le_bytes());
        buf.extend_from_slice(&value.to_le_bytes());
    }
    buf.extend_from_slice(&0u32.to_le_bytes()); // 下一 IFD = 0
    debug_assert_eq!(buf.len(), exif_off);
    // ExifIFD：ISO + 可选像素维度
    let (exif_bytes, _) = encode_ifd(&exif_ents, 0); // 值全内联，数据区空
    buf.extend_from_slice(&exif_bytes);
    debug_assert_eq!(buf.len(), sub_off);
    // SubIFD：主图（NewSubfileType=0）+ 全尺寸
    let sub_ents = vec![
        ent(0x00FE, 4, 0u32.to_le_bytes().to_vec()),
        ent(0x0100, 4, 6064u32.to_le_bytes().to_vec()),
        ent(0x0101, 4, 4040u32.to_le_bytes().to_vec()),
    ];
    let (sub_bytes, _) = encode_ifd(&sub_ents, 0);
    buf.extend_from_slice(&sub_bytes);
    // 尾部内嵌缩略图 JPEG（SOI + SOF0 640×424 + EOI）
    buf.extend_from_slice(&[0xFF, 0xD8]);
    buf.extend_from_slice(&sof0_segment(640, 424));
    buf.extend_from_slice(&[0xFF, 0xD9]);
    buf
}

/// NEF 主路径：EXIF 无 0xA002 → 全尺寸取 SubIFD 主图 IFD（6064×4040），
/// 缩略图 JPEG(640×424) 与缩略图层 IFD0（NewSubfileType=1）都被跳过。
#[test]
fn nef_dims_from_subifd_primary_not_thumbnail_layer() {
    let meta = parse(&build_nef_like(None));
    assert_eq!(meta.width, Some(6064));
    assert_eq!(meta.height, Some(4040));
    assert_eq!(meta.iso, Some(7200), "EXIF 子 IFD 拍摄参数照常（本就正确）");
    assert_ne!(meta.width, Some(640), "绝不回退到内嵌缩略图尺寸");
}

/// ARW 主路径（gen-4 不回归）：EXIF 0xA002/0xA003 存在时优先于 SubIFD。
#[test]
fn exif_pixel_dims_take_priority_over_subifd() {
    let meta = parse(&build_nef_like(Some((6016, 4016))));
    assert_eq!(meta.width, Some(6016));
    assert_eq!(meta.height, Some(4016));
    assert_eq!(meta.iso, Some(7200));
}

/// CR3（ISO BMFF 容器，ftyp brand "crx "）：无 EXIF 解析支持
/// （kamadak 只认 HEIF 系 mif1/msf1 brand）——契约是**不误读**：
/// 内嵌 JPEG 预览的尺寸绝不冒充本体，宽高/相机全 None；待真机样本
/// 接入后再开 CR3 专用提取路径。
#[test]
fn cr3_isobmff_preview_not_misread_as_body() {
    let mut head = Vec::new();
    // 最小 ftyp box：size 0x18 + "ftyp" + major "crx " + minor 0 + compat "crx "
    head.extend_from_slice(&24u32.to_be_bytes());
    head.extend_from_slice(b"ftyp");
    head.extend_from_slice(b"crx ");
    head.extend_from_slice(&0u32.to_be_bytes());
    head.extend_from_slice(b"crx ");
    // 内嵌 JPEG 预览（SOI + SOF0 640×424 + EOI）
    head.extend_from_slice(&[0xFF, 0xD8]);
    head.extend_from_slice(&sof0_segment(640, 424));
    head.extend_from_slice(&[0xFF, 0xD9]);
    let meta = parse(&head);
    assert_eq!(meta.width, None, "CR3 内嵌预览尺寸不得冒充本体");
    assert_eq!(meta.height, None);
    assert_eq!(meta.camera, None);
    assert_eq!(meta.iso, None);
}

/// 真实文件冒烟（直连已知样例路径，不做目录遍历——真实数据测试禁止全量
/// 扫描，只读头部 1MB 渐进解析；样例缺失时直接跳过）。
#[test]
#[ignore = "依赖本机 Y:\\照片 真实照片，需 --ignored 手动运行"]
fn real_raw_head_smoke() {
    // (样例, 格式说明)；如换机后失效，更新为任一已知 RAW 全路径即可
    const SAMPLES: &[(&str, &str)] = &[
        (r"Y:\照片\20250607团建\DSC_0176.NEF", "尼康 Z5"),
        (r"Y:\照片\测试\DSC00024..ARW", "索尼"),
        (
            r"Y:\照片\20250607团建\DSC_0224-已增强-降噪.dng",
            "LR 导出 DNG",
        ),
    ];
    for &(sample, vendor) in SAMPLES {
        let path = Path::new(sample);
        if !path.is_file() {
            eprintln!("skip: 样例不存在 {sample}");
            continue;
        }
        let mut head = Vec::new();
        fs::File::open(path)
            .expect("failed to open photo")
            .take(1024 * 1024)
            .read_to_end(&mut head)
            .expect("failed to read head");
        let meta = parse(&head); // 不 panic 即通过
        eprintln!(
            "{sample} ({vendor}) -> w={:?} h={:?} iso={:?} focal={:?} camera={:?}",
            meta.width, meta.height, meta.iso, meta.focal_length, meta.camera
        );
        // 核心契约：RAW 分辨率必须是本体级（≥2000px），绝不是缩略图级
        // （NEF 旧 bug：6064×4040 本体显示成 640×424）
        let w = meta.width.expect("RAW 应解析出本体宽度");
        let h = meta.height.expect("RAW 应解析出本体高度");
        assert!(
            w >= 2000 && h >= 2000,
            "{vendor} 分辨率疑似缩略图层: {w}x{h}"
        );
        // 拍摄参数层（EXIF 子 IFD）至少有 ISO 或焦段
        assert!(
            meta.iso.is_some() || meta.focal_length.is_some(),
            "{vendor} 拍摄参数缺失"
        );
    }
}

#[test]
fn bad_truncated_sof_yields_none_not_panic() {
    let tiff = build_full_tiff(
        Some("C"),
        Some("M"),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    );
    let mut bytes = wrap_jpeg_with_sof(&tiff, Some((100, 200)));
    // SOF 段被截断（声明长度超出缓冲）
    let cut = bytes.len() - 4;
    bytes.truncate(cut);
    let meta = parse(&bytes);
    assert_eq!(meta.width, None);
    assert_eq!(meta.height, None);
}
