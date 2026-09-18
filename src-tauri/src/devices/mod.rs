//! 设备源抽象（spec §5.1）：卷设备与 WPD 相机统一为 `DeviceSource` trait，
//! 导入引擎不感知设备类型。热插拔检测在 `hotplug`（WM_DEVICECHANGE），
//! 卷实现在 `volume`，WPD/MTP 实现在 `wpd`。

pub mod folder;
pub mod hotplug;
pub mod orchestrator;
pub mod present;
pub mod volume;
pub mod wpd;

use std::io::Read;

use chrono::{DateTime, Utc};

use crate::events::AssetKind;
pub use crate::events::SourceKind;

/// 设备上可导入的文件条目。
#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    /// 设备内唯一标识：卷设备为相对路径；WPD 为对象持久 ID
    pub id: String,
    /// 相对路径（统一 `/` 分隔，如 `DCIM/100CANON/IMG_0001.CR3`）
    pub rel_path: String,
    pub size: u64,
    pub mtime: DateTime<Utc>,
}

/// 设备源错误分类：UI 需要区分给出针对性提示。
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// 相机未切 PC 连接模式 / 手机锁定等：可提示用户操作设备
    #[error("device denied access (switch camera to PC connect mode / unlock phone)")]
    AccessDenied,
    /// 传输中拔线：任务应暂停而非失败
    #[error("device disconnected")]
    Disconnected,
    /// 该源不支持的操作（如 MTP 流不支持 Seek 时的尾部局部哈希）
    #[error("operation not supported by this source: {0}")]
    #[allow(dead_code)] // 错误契约的一部分（测试桩构造；lib 内暂无触发点）
    NotSupported(String),
    #[error("device error: {0}")]
    Other(String),
}

pub type DeviceResult<T> = Result<T, DeviceError>;

/// 设备源统一接口。
pub trait DeviceSource: Send + Sync {
    /// 稳定设备标识（卷盘符如 `E:`；WPD PnP 路径）
    fn id(&self) -> String;
    fn kind(&self) -> SourceKind;
    /// 展示名（卷标 / 相机友好名）
    fn name(&self) -> String;
    /// 枚举全部媒体文件（跳过系统目录与非媒体扩展名）。
    fn list(&self) -> DeviceResult<Vec<FileEntry>>;
    /// 读取文件头部（≤max 字节）用于魔数识别与 EXIF 探测。
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>>;
    /// 全文件流（复制用）。调用方负责读完或 drop。
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn Read + Send>>;
    /// 删除源文件（M2 move 模式：校验入册后删源）。
    /// 默认不支持（仅实现该能力的源可参与移动导入）。
    fn delete(&self, _id: &str) -> DeviceResult<()> {
        Err(DeviceError::NotSupported("delete".into()))
    }
}

// ---------------------------------------------------------------------------
// 媒体识别：扩展名白名单 + 魔数校验（spec §5.2 类型识别）
// ---------------------------------------------------------------------------

/// 照片扩展名（小写）。
pub const PHOTO_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "heic", "heif", "avif", "tif", "tiff", "bmp", "gif", "webp", "jxl",
];
/// RAW 扩展名（小写）。
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "r3d", "iiq", "pef", "srw", "x3f",
    "nev",
];
/// 视频扩展名（小写）。
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "mts", "m2ts", "wmv", "3gp", "avchd",
];

/// 全部媒体扩展名。
pub fn is_media_ext(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    PHOTO_EXTS.contains(&ext.as_str())
        || RAW_EXTS.contains(&ext.as_str())
        || VIDEO_EXTS.contains(&ext.as_str())
}

fn ext_kind(ext: &str) -> Option<AssetKind> {
    let ext = ext.to_ascii_lowercase();
    if PHOTO_EXTS.contains(&ext.as_str()) {
        Some(AssetKind::Photo)
    } else if RAW_EXTS.contains(&ext.as_str()) {
        Some(AssetKind::Raw)
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some(AssetKind::Video)
    } else {
        None
    }
}

/// 从魔数推断类型（无扩展名/未知扩展名时兜底）。
fn magic_kind(head: &[u8]) -> Option<AssetKind> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(AssetKind::Photo); // JPEG（含内嵌于 RAW 的预览头少见，主判扩展名）
    }
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        return Some(AssetKind::Photo);
    }
    if head.starts_with(b"II*\0") || head.starts_with(b"MM\0*") {
        // TIFF 系：多数 RAW（NEF/ARW/CR2/DNG/ORF...）也是 TIFF 头
        return Some(AssetKind::Raw);
    }
    if head.starts_with(b"FUJIFILM") {
        return Some(AssetKind::Raw); // RAF
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        return match brand {
            b"crx " | b"CRX " => Some(AssetKind::Raw), // CR3
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => Some(AssetKind::Photo),
            _ => Some(AssetKind::Video), // mp4/mov/qt 等 ISOBMFF
        };
    }
    if head.starts_with(b"RIFF") {
        if head.len() >= 12 && &head[8..12] == b"AVI " {
            return Some(AssetKind::Video);
        }
        if head.len() >= 12 && &head[8..12] == b"WEBP" {
            return Some(AssetKind::Photo);
        }
        return Some(AssetKind::Video);
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(AssetKind::Video); // Matroska/MKV/WebM
    }
    None
}

/// 扩展名 + 魔数双保险分类。
///
/// 规则：扩展名优先；若魔数可判且与扩展名结论冲突（如 .jpg 实为 mp4），
/// 判 `Other`（伪装文件，不导入）。无头数据（空 head）时信任扩展名。
pub fn classify(file_name: &str, head: &[u8]) -> AssetKind {
    let ext = file_name.rsplit('.').next().unwrap_or("");
    let by_ext = ext_kind(ext);
    if head.is_empty() {
        return by_ext.unwrap_or(AssetKind::Other);
    }
    let by_magic = magic_kind(head);
    match (by_ext, by_magic) {
        (Some(ext_kind_), Some(magic_kind_)) if ext_kind_ != magic_kind_ => {
            // 冲突容忍：TIFF 头的 RAW 与 Photo 互认（CR2/TIFF/DNG 同头、HEIC 各 brand 差异）
            let both_image = matches!(ext_kind_, AssetKind::Photo | AssetKind::Raw)
                && matches!(magic_kind_, AssetKind::Photo | AssetKind::Raw);
            if both_image {
                ext_kind_
            } else {
                AssetKind::Other
            }
        }
        (Some(kind), _) | (None, Some(kind)) => kind,
        (None, None) => AssetKind::Other,
    }
}

/// 相对路径归一化：`\` → `/`，去掉开头 `./`。
pub fn normalize_rel_path(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    while let Some(stripped) = p.strip_prefix("./") {
        p = stripped.to_string();
    }
    p.trim_start_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(bytes: &[u8]) -> Vec<u8> {
        bytes.to_vec()
    }

    #[test]
    fn classify_by_extension_without_head() {
        assert_eq!(classify("IMG_0001.JPG", &[]), AssetKind::Photo);
        assert_eq!(classify("DSC00001.ARW", &[]), AssetKind::Raw);
        assert_eq!(classify("MVI_0001.MP4", &[]), AssetKind::Video);
    }

    #[test]
    fn classify_by_magic_matrix() {
        assert_eq!(
            classify("a.jpg", &head(&[0xFF, 0xD8, 0xFF, 0xE0])),
            AssetKind::Photo
        );
        assert_eq!(classify("a.nef", &head(b"II*\0\x00\x00")), AssetKind::Raw);
        assert_eq!(
            classify("a.cr3", &head(b"\0\0\0\x18ftypcrx \0\0")),
            AssetKind::Raw
        );
        assert_eq!(
            classify("a.heic", &head(b"\0\0\0\x18ftypheic\0\0")),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.mp4", &head(b"\0\0\0\x20ftypisom\0\0")),
            AssetKind::Video
        );
        assert_eq!(
            classify("a.avi", &head(b"RIFF\x00\x00\x00\x00AVI ")),
            AssetKind::Video
        );
        assert_eq!(
            classify("a.mkv", &head(&[0x1A, 0x45, 0xDF, 0xA3])),
            AssetKind::Video
        );
        assert_eq!(
            classify("a.raf", &head(b"FUJIFILMCCD-RAW ")),
            AssetKind::Raw
        );
    }

    #[test]
    fn extension_magic_conflict_marks_other() {
        // 伪装：.jpg 实为 mp4
        assert_eq!(
            classify("fake.jpg", &head(b"\0\0\0\x20ftypisom\0\0")),
            AssetKind::Other
        );
        // 图片家族内部互认：.tif 是 TIFF 头 → Photo（按扩展名）
        assert_eq!(classify("a.tif", &head(b"II*\0")), AssetKind::Photo);
    }

    #[test]
    fn unknown_extension_and_unknown_magic_is_other() {
        assert_eq!(classify("a.xyz", &head(b"\x01\x02\x03")), AssetKind::Other);
        assert_eq!(classify("a.dat", &[]), AssetKind::Other);
    }

    #[test]
    fn media_ext_filter() {
        assert!(is_media_ext("CR3"));
        assert!(is_media_ext("jpg"));
        assert!(!is_media_ext("txt"));
        assert!(!is_media_ext("db"));
    }

    #[test]
    fn rel_path_normalization() {
        assert_eq!(
            normalize_rel_path(r"DCIM\100CANON\IMG.CR3"),
            "DCIM/100CANON/IMG.CR3"
        );
        assert_eq!(normalize_rel_path("./DCIM/x.jpg"), "DCIM/x.jpg");
        assert_eq!(normalize_rel_path("/root/a.jpg"), "root/a.jpg");
    }
}
