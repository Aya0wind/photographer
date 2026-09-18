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
use walkdir::WalkDir;

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
    // ExifIFD 指针越界（指向文件末尾之外）
    let mut broken = good.clone();
    let ptr_value_at = 8 + 2 + 12 * 2 + 8; // IFD0 内第 3 条（ExifIFD 指针）的值偏移
    broken[ptr_value_at..ptr_value_at + 4].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
    assert_eq!(parse(&broken), MetaLite::default());
}

/// 真实文件冒烟（只读遍历 `Y:\照片`，找第一个 .jpg 读头部 1MB）：
/// 断言解析不 panic；camera 多半 Some（打印观察，不作硬断言）。
#[test]
#[ignore = "依赖本机 Y:\\照片 真实照片，需 --ignored 手动运行"]
fn real_jpeg_head_smoke() {
    let root = Path::new(r"Y:\照片");
    if !root.is_dir() {
        eprintln!("skip: {} 不存在", root.display());
        return;
    }
    let found = WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .find(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("jpg"))
        });
    let Some(entry) = found else {
        eprintln!("skip: {} 下未找到 .jpg", root.display());
        return;
    };
    let mut head = Vec::new();
    fs::File::open(entry.path())
        .expect("failed to open photo")
        .take(1024 * 1024)
        .read_to_end(&mut head)
        .expect("failed to read head");
    let meta = parse(&head); // 不 panic 即通过
    eprintln!(
        "{} -> captured_at={:?} camera={:?}",
        entry.path().display(),
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
