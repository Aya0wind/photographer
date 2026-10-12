//! Metadata-only IFD merge; compressed strips and their offsets remain intact.
//! The upstream codec owns pixel encoding. This adapter carries the app's
//! prepared EXIF fields where the codec currently emits only ICC/XMP/text.
use std::collections::{BTreeMap, HashSet};
fn directory_end(b: &[u8], dir: usize) -> Result<(usize, usize), String> {
    let count = u16_at(b, dir)? as usize;
    let end = dir.checked_add(2 + count * 12).ok_or("TIFF 目录过大")?;
    u32_at(b, end)?;
    Ok((count, end))
}
fn u16_at(b: &[u8], p: usize) -> Result<u16, String> {
    Ok(u16::from_le_bytes(
        b.get(p..p.checked_add(2).ok_or("TIFF 元数据越界")?)
            .ok_or("TIFF 元数据越界")?
            .try_into()
            .unwrap(),
    ))
}
fn u32_at(b: &[u8], p: usize) -> Result<u32, String> {
    Ok(u32::from_le_bytes(
        b.get(p..p.checked_add(4).ok_or("TIFF 元数据越界")?)
            .ok_or("TIFF 元数据越界")?
            .try_into()
            .unwrap(),
    ))
}
pub(super) fn embed(mut output: Vec<u8>, metadata: &[u8]) -> Result<Vec<u8>, String> {
    if output.get(..2) != Some(b"II") || metadata.get(..2) != Some(b"II") {
        return Err("当前 TIFF 元数据仅支持小端输出".into());
    }
    if u16_at(metadata, 2)? != 42 {
        return Err("源 EXIF 格式无效".into());
    }
    if u16_at(&output, 2)? == 43 {
        return embed_big(output, metadata);
    }
    if u16_at(&output, 2)? != 42 {
        return Err("TIFF 文件头无效".into());
    }
    let first = u32_at(&output, 4)? as usize;
    let (count, end) = directory_end(&output, first)?;
    let next = u32_at(&output, end)?;
    let mut entries = BTreeMap::<u16, [u8; 12]>::new();
    for n in 0..count {
        let at = first + 2 + n * 12;
        entries.insert(
            u16_at(&output, at)?,
            output
                .get(at..at + 12)
                .ok_or("TIFF 目录越界")?
                .try_into()
                .unwrap(),
        );
    }
    while output.len() % 4 != 0 {
        output.push(0);
    }
    let base = u32::try_from(output.len()).map_err(|_| "TIFF 元数据超过 4 GB")?;
    let mut extra = metadata.to_vec();
    let root = u32_at(&extra, 4)? as usize;
    let mut pending = vec![root];
    let mut visited = HashSet::new();
    while let Some(dir) = pending.pop() {
        if !visited.insert(dir) {
            return Err("TIFF 元数据目录循环".into());
        }
        if visited.len() > 16 {
            return Err("TIFF 元数据目录过多".into());
        }
        let (count, tail) = directory_end(&extra, dir)?;
        for n in 0..count {
            let at = dir + 2 + n * 12;
            let tag = u16_at(&extra, at)?;
            let kind = u16_at(&extra, at + 2)?;
            let length = u32_at(&extra, at + 4)? as usize;
            let unit = match kind {
                1 | 2 | 6 | 7 => 1,
                3 | 8 => 2,
                4 | 9 | 11 | 13 => 4,
                5 | 10 | 12 => 8,
                _ => return Err("TIFF 元数据类型不受支持".into()),
            };
            let size = length.checked_mul(unit).ok_or("TIFF 元数据过大")?;
            let pointer = [34665, 34853, 40965].contains(&tag);
            if pointer && (kind != 4 || length != 1) {
                return Err("TIFF 元数据目录指针无效".into());
            }
            if pointer || size > 4 {
                let old = u32_at(&extra, at + 8)?;
                if pointer {
                    if old != 0 {
                        pending.push(old as usize);
                    }
                } else if old < 8
                    || (old as usize)
                        .checked_add(size)
                        .is_none_or(|end| end > extra.len())
                {
                    return Err("TIFF 元数据数据越界".into());
                }
                if old != 0 {
                    let relocated = base.checked_add(old).ok_or("TIFF 元数据超过 4 GB")?;
                    extra
                        .get_mut(at + 8..at + 12)
                        .ok_or("TIFF 元数据越界")?
                        .copy_from_slice(&relocated.to_le_bytes());
                }
            }
        }
        let next = u32_at(&extra, tail)?;
        if next != 0 {
            pending.push(next as usize);
            extra[tail..tail + 4].copy_from_slice(
                &base
                    .checked_add(next)
                    .ok_or("TIFF 元数据超过 4 GB")?
                    .to_le_bytes(),
            );
        }
    }
    let count = u16_at(&extra, root)? as usize;
    for n in 0..count {
        let at = root + 2 + n * 12;
        let tag = u16_at(&extra, at)?;
        // Structural/pixel tags belong to the codec, never the source EXIF.
        if !structural(tag) {
            entries.insert(
                tag,
                extra
                    .get(at..at + 12)
                    .ok_or("TIFF 元数据越界")?
                    .try_into()
                    .unwrap(),
            );
        }
    }
    output.extend(extra);
    while output.len() % 4 != 0 {
        output.push(0);
    }
    let directory = u32::try_from(output.len()).map_err(|_| "TIFF 元数据超过 4 GB")?;
    let count = u16::try_from(entries.len()).map_err(|_| "TIFF 元数据过多")?;
    output.extend(count.to_le_bytes());
    for entry in entries.values() {
        output.extend(entry);
    }
    output.extend(next.to_le_bytes());
    output[4..8].copy_from_slice(&directory.to_le_bytes());
    Ok(output)
}

fn u64_at(b: &[u8], p: usize) -> Result<u64, String> {
    Ok(u64::from_le_bytes(
        b.get(p..p.checked_add(8).ok_or("TIFF 元数据越界")?)
            .ok_or("TIFF 元数据越界")?
            .try_into()
            .unwrap(),
    ))
}
fn index(offset: u64) -> Result<usize, String> {
    usize::try_from(offset).map_err(|_| "TIFF 偏移超出地址范围".into())
}
fn align_big(output: &mut Vec<u8>) {
    while output.len() % 8 != 0 {
        output.push(0);
    }
}
fn write_big_directory(output: &mut Vec<u8>, entries: &BTreeMap<u16, [u8; 20]>, next: u64) -> u64 {
    align_big(output);
    let offset = output.len() as u64;
    output.extend((entries.len() as u64).to_le_bytes());
    for entry in entries.values() {
        output.extend(entry);
    }
    output.extend(next.to_le_bytes());
    offset
}
fn structural(tag: u16) -> bool {
    [
        256, 257, 258, 259, 262, 273, 277, 278, 279, 284, 317, 322, 323, 324, 325, 330, 338, 339,
    ]
    .contains(&tag)
}
/// Convert the prepared classic EXIF directories to BigTIFF entries. Values
/// fitting eight bytes become inline; child directories use 64-bit IFD offsets.
/// Source offsets never get added to a 32-bit base, so an output over 4 GB works.
fn big_metadata_entries(
    output: &mut Vec<u8>,
    metadata: &[u8],
    dir: usize,
    visited: &mut HashSet<usize>,
) -> Result<(BTreeMap<u16, [u8; 20]>, u64), String> {
    if dir < 8 || !visited.insert(dir) || visited.len() > 16 {
        return Err("TIFF 元数据目录无效或循环".into());
    }
    let (count, tail) = directory_end(metadata, dir)?;
    let mut entries = BTreeMap::new();
    for n in 0..count {
        let at = dir + 2 + n * 12;
        let tag = u16_at(metadata, at)?;
        let mut kind = u16_at(metadata, at + 2)?;
        let length = u32_at(metadata, at + 4)?;
        let unit = match kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 | 13 => 4,
            5 | 10 | 12 => 8,
            _ => return Err("TIFF 元数据类型不受支持".into()),
        };
        let size = (length as usize)
            .checked_mul(unit)
            .ok_or("TIFF 元数据过大")?;
        let mut value = [0u8; 8];
        if [34665, 34853, 40965].contains(&tag) {
            if kind != 4 || length != 1 {
                return Err("TIFF 元数据目录指针无效".into());
            }
            kind = 18;
            let child = u32_at(metadata, at + 8)?;
            if child != 0 {
                let (child_entries, next) =
                    big_metadata_entries(output, metadata, child as usize, visited)?;
                value.copy_from_slice(
                    &write_big_directory(output, &child_entries, next).to_le_bytes(),
                );
            }
        } else {
            let start = if size <= 4 {
                at + 8
            } else {
                let start = u32_at(metadata, at + 8)? as usize;
                if start < 8 {
                    return Err("TIFF 元数据数据越界".into());
                }
                start
            };
            let bytes = metadata
                .get(start..start.checked_add(size).ok_or("TIFF 元数据数据越界")?)
                .ok_or("TIFF 元数据数据越界")?;
            if size <= 8 {
                value[..size].copy_from_slice(bytes);
            } else {
                align_big(output);
                value.copy_from_slice(&(output.len() as u64).to_le_bytes());
                output.extend(bytes);
            }
        }
        let mut entry = [0u8; 20];
        entry[..2].copy_from_slice(&tag.to_le_bytes());
        entry[2..4].copy_from_slice(&kind.to_le_bytes());
        entry[4..12].copy_from_slice(&(length as u64).to_le_bytes());
        entry[12..].copy_from_slice(&value);
        entries.insert(tag, entry);
    }
    let next = u32_at(metadata, tail)?;
    let next = if next == 0 {
        0
    } else {
        let (entries, next) = big_metadata_entries(output, metadata, next as usize, visited)?;
        write_big_directory(output, &entries, next)
    };
    Ok((entries, next))
}
fn embed_big(mut output: Vec<u8>, metadata: &[u8]) -> Result<Vec<u8>, String> {
    if u16_at(&output, 4)? != 8 || u16_at(&output, 6)? != 0 {
        return Err("BigTIFF 偏移格式无效".into());
    }
    let first = index(u64_at(&output, 8)?)?;
    let count = index(u64_at(&output, first)?)?;
    let end = first
        .checked_add(8)
        .and_then(|start| {
            count
                .checked_mul(20)
                .and_then(|size| start.checked_add(size))
        })
        .ok_or("BigTIFF 目录过大")?;
    let next = u64_at(&output, end)?;
    let mut entries = BTreeMap::new();
    for n in 0..count {
        let at = first + 8 + n * 20;
        entries.insert(
            u16_at(&output, at)?,
            output[at..at + 20].try_into().unwrap(),
        );
    }
    let root = u32_at(metadata, 4)? as usize;
    let (extra, _) = big_metadata_entries(&mut output, metadata, root, &mut HashSet::new())?;
    for (tag, entry) in extra {
        if !structural(tag) {
            entries.insert(tag, entry);
        }
    }
    let directory = write_big_directory(&mut output, &entries, next);
    output[8..16].copy_from_slice(&directory.to_le_bytes());
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bigtiff_keeps_pixels_and_independent_decoder_reads_exif_and_gps() {
        use photocraft_engine::doc::{Color, ColorMode, Document, SampleType, Size};
        let doc = Document::with_background(
            "Big",
            Size::new(8, 6),
            ColorMode::Rgb,
            SampleType::U16,
            Color::rgba(0.2, 0.5, 0.8, 1.0),
        );
        let mut options = photocraft_io::ExportOptions::default();
        options.encode.tiff_bigtiff = true;
        let original = photocraft_io::export(&doc, "tif", &options).unwrap().bytes;
        assert_eq!(u16_at(&original, 2).unwrap(), 43);
        let fields = [
            exif::Field {
                tag: exif::Tag::ExposureTime,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Rational(vec![exif::Rational { num: 1, denom: 250 }]),
            },
            exif::Field {
                tag: exif::Tag::Model,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"Camera model longer than inline".to_vec()]),
            },
            exif::Field {
                tag: exif::Tag::GPSLatitudeRef,
                ifd_num: exif::In::PRIMARY,
                value: exif::Value::Ascii(vec![b"N".to_vec()]),
            },
        ];
        let mut writer = exif::experimental::Writer::new();
        for field in &fields {
            writer.push_field(field);
        }
        let mut source = std::io::Cursor::new(Vec::new());
        writer.write(&mut source, true).unwrap();
        let opts = super::super::meta::MetaOptions {
            author: Some("Artist".into()),
            ..Default::default()
        };
        let (metadata, _) =
            super::super::meta::flat_metadata(Some(source.get_ref()), &opts, 8, 6).unwrap();
        let merged = embed(original.clone(), &metadata).unwrap();
        assert_eq!(&merged[16..original.len()], &original[16..]);
        let mut decoder = tiff::decoder::Decoder::new(std::io::Cursor::new(&merged)).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (8, 6));
        assert_eq!(
            decoder
                .get_tag_ascii_string(tiff::tags::Tag::Artist)
                .unwrap(),
            "Artist"
        );
        assert_eq!(
            decoder
                .get_tag_ascii_string(tiff::tags::Tag::Model)
                .unwrap(),
            "Camera model longer than inline"
        );
        let pointer = decoder
            .get_tag(tiff::tags::Tag::ExifDirectory)
            .unwrap()
            .into_ifd_pointer()
            .unwrap();
        let dir = decoder.read_directory(pointer).unwrap();
        assert_eq!(
            decoder
                .read_directory_tags(&dir)
                .get_tag(tiff::tags::Tag::Unknown(33434))
                .unwrap(),
            tiff::decoder::ifd::Value::Rational(1, 250)
        );
        assert_eq!(
            decoder
                .read_directory_tags(&dir)
                .get_tag_u32(tiff::tags::Tag::Unknown(40962))
                .unwrap(),
            8
        );
        let pointer = decoder
            .get_tag(tiff::tags::Tag::GpsDirectory)
            .unwrap()
            .into_ifd_pointer()
            .unwrap();
        let dir = decoder.read_directory(pointer).unwrap();
        assert_eq!(
            decoder
                .read_directory_tags(&dir)
                .get_tag_ascii_string(tiff::tags::Tag::Unknown(1))
                .unwrap(),
            "N"
        );
        // The generic image format sniffer does not recognize BigTIFF's 43
        // header. Use the independent TIFF decoder explicitly for pixel proof.
        let pixels = match decoder.read_image().unwrap() {
            tiff::decoder::DecodingResult::U16(values) => values,
            other => panic!("unexpected pixels {other:?}"),
        };
        let before = match tiff::decoder::Decoder::new(std::io::Cursor::new(&original))
            .unwrap()
            .read_image()
            .unwrap()
        {
            tiff::decoder::DecodingResult::U16(values) => values,
            other => panic!("unexpected pixels {other:?}"),
        };
        assert_eq!(pixels, before);
    }
    #[test]
    fn bigtiff_rejects_invalid_offset_width_truncated_directories_and_metadata_cycles() {
        let mut big = vec![b'I', b'I', 43, 0, 8, 0, 0, 0];
        big.extend(16u64.to_le_bytes());
        big.extend(0u64.to_le_bytes());
        big.extend(0u64.to_le_bytes());
        let mut invalid = big.clone();
        invalid[4] = 4;
        assert!(embed(invalid, &empty()).is_err());
        let mut invalid = big.clone();
        invalid[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(embed(invalid, &empty()).is_err());
        assert!(embed(big, &field(34665, 4, 1, 8)).is_err());
    }
    fn empty() -> Vec<u8> {
        vec![b'I', b'I', 42, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    }
    fn field(tag: u16, kind: u16, count: u32, value: u32) -> Vec<u8> {
        let mut bytes = empty();
        bytes.truncate(8);
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(tag.to_le_bytes());
        bytes.extend(kind.to_le_bytes());
        bytes.extend(count.to_le_bytes());
        bytes.extend(value.to_le_bytes());
        bytes.extend(0u32.to_le_bytes());
        bytes
    }
    #[test]
    fn rejects_truncated_cycles_bad_pointer_types_and_out_of_bounds_payloads() {
        let mut truncated = empty();
        truncated[8..10].copy_from_slice(&65535u16.to_le_bytes());
        for invalid in [
            truncated,
            field(34665, 4, 1, 8),
            field(34665, 3, 1, 8),
            field(272, 2, 10, 0),
            field(272, 2, 10, u32::MAX),
        ] {
            assert!(embed(empty(), &invalid).is_err());
        }
        let mut output = empty();
        output[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(embed(output, &empty()).is_err());
    }
    #[test]
    fn preserves_encoded_pixels_and_reads_metadata_beyond_initial_read_window() {
        let opts = super::super::meta::MetaOptions {
            remove_gps: false,
            author: Some("Tail Artist".into()),
            copyright: None,
            keywords: vec![],
        };
        let (metadata, _) = super::super::meta::flat_metadata(None, &opts, 1, 1).unwrap();
        let mut output = empty();
        output.resize(3 * 1024 * 1024, 0xAB);
        let prefix = output[8..].to_vec();
        let merged = embed(output, &metadata).unwrap();
        assert_eq!(&merged[8..8 + prefix.len()], prefix);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tail.tif");
        std::fs::write(&path, merged).unwrap();
        let fields = super::super::meta::source_exif(&path).unwrap();
        let parsed = exif::Reader::new()
            .read_from_container(&mut std::io::Cursor::new(fields))
            .unwrap();
        match &parsed
            .get_field(exif::Tag::Artist, exif::In::PRIMARY)
            .unwrap()
            .value
        {
            exif::Value::Ascii(values) => assert_eq!(values, &vec![b"Tail Artist".to_vec()]),
            other => panic!("unexpected artist {other:?}"),
        };
    }
}
