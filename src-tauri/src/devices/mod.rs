//! 设备源抽象（spec §5.1）：卷设备与 WPD 相机统一为 `DeviceSource` trait，
//! 导入引擎不感知设备类型。热插拔检测在 `hotplug`（WM_DEVICECHANGE），
//! 卷实现在 `volume`，WPD/MTP 实现在 `wpd`。

pub mod diagnostics;
pub mod folder;
pub mod hotplug;
pub mod lifecycle;
pub mod orchestrator;
pub mod present;
#[cfg(any(windows, test))]
pub mod timed_worker;
pub mod volume;
#[cfg(windows)]
#[path = "../platform/windows/wpd.rs"]
pub mod wpd;
#[cfg(target_os = "macos")]
#[path = "../platform/macos/wpd.rs"]
pub mod wpd;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/wpd.rs"]
pub mod wpd;
#[cfg(target_os = "android")]
#[path = "../platform/android/wpd.rs"]
pub mod wpd;
#[cfg(not(any(
    windows,
    target_os = "macos",
    target_os = "linux",
    target_os = "android"
)))]
#[path = "../platform/unsupported/wpd.rs"]
pub mod wpd;
mod wpd_helpers;

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
pub type FileBatchCallback = std::sync::Arc<dyn Fn(Vec<FileEntry>) + Send + Sync>;

/// 设备源统一接口。
pub trait DeviceSource: Send + Sync {
    /// 稳定设备标识（卷盘符如 `E:`；WPD PnP 路径）
    fn id(&self) -> String;
    fn kind(&self) -> SourceKind;
    /// 展示名（卷标 / 相机友好名）
    fn name(&self) -> String;
    /// 枚举全部媒体文件（跳过系统目录与非媒体扩展名）。
    fn list(&self) -> DeviceResult<Vec<FileEntry>>;
    fn list_with_progress(&self, on_batch: FileBatchCallback) -> DeviceResult<Vec<FileEntry>> {
        let files = self.list()?;
        on_batch(files.clone());
        Ok(files)
    }
    /// 读取文件头部（≤max 字节）用于魔数识别与 EXIF 探测。
    fn open_head(&self, id: &str, max: u64) -> DeviceResult<Vec<u8>>;
    /// 全文件流（复制用）。调用方负责读完或 drop。
    fn stream(&self, id: &str) -> DeviceResult<Box<dyn Read + Send>>;
    /// 源文件的本地绝对路径（同卷 rename 快道用）。非本地源（WPD/MTP）
    /// 返回 None → 引擎走流式复制。
    fn local_path(&self, _id: &str) -> Option<std::path::PathBuf> {
        None
    }
    /// 设备提供的缩略资源；不支持时返回 None，不能为预览传输整个原文件。
    fn thumbnail(&self, _id: &str) -> DeviceResult<Option<Vec<u8>>> {
        Ok(None)
    }
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
    "ico", "cur",
];
/// RAW 扩展名（小写）。
pub const RAW_EXTS: &[&str] = &[
    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "r3d", "iiq", "pef", "srw", "x3f",
    "nev",
];
/// 已知非图片扩展名：即便内容伪装成 JPEG，也不能进入导入任务。
const NON_IMAGE_EXTS: &[&str] = &[
    "mp4", "mov", "avi", "mkv", "mts", "m2ts", "wmv", "3gp", "avchd", "webm",
];
/// 可导入的图片扩展名。
pub fn is_media_ext(ext: &str) -> bool {
    let ext = ext.to_ascii_lowercase();
    PHOTO_EXTS.contains(&ext.as_str()) || RAW_EXTS.contains(&ext.as_str())
}

fn ext_kind(ext: &str) -> Option<AssetKind> {
    let ext = ext.to_ascii_lowercase();
    if PHOTO_EXTS.contains(&ext.as_str()) {
        Some(AssetKind::Photo)
    } else if RAW_EXTS.contains(&ext.as_str()) {
        Some(AssetKind::Raw)
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
    if head.starts_with(&[0x00, 0x00, 0x01, 0x00]) || head.starts_with(&[0x00, 0x00, 0x02, 0x00]) {
        return Some(AssetKind::Photo); // ICO / CUR（00 00 01 00 / 00 00 02 00）
    }
    if head.starts_with(&[0xFF, 0x0A]) {
        return Some(AssetKind::Photo); // JXL 裸码流（JPEG 起始为 FF D8，不冲突）
    }
    if head.starts_with(&[
        0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
    ]) {
        return Some(AssetKind::Photo); // JXL 容器签名
    }
    if head.len() >= 12 && &head[4..8] == b"ftyp" {
        let brand = &head[8..12];
        return match brand {
            b"crx " | b"CRX " => Some(AssetKind::Raw), // CR3
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => Some(AssetKind::Photo),
            b"avif" | b"avis" => Some(AssetKind::Photo), // AVIF 静图 / 序列
            _ => Some(AssetKind::Other),                 // 非图片 ISOBMFF 容器
        };
    }
    if head.starts_with(b"RIFF") {
        if head.len() >= 12 && &head[8..12] == b"WEBP" {
            return Some(AssetKind::Photo);
        }
        return Some(AssetKind::Other);
    }
    if head.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        return Some(AssetKind::Other);
    }
    None
}

/// 扩展名 + 魔数双保险分类。
///
/// 规则：仅允许图片扩展名；魔数与扩展名冲突时判 `Other`。
pub fn classify(file_name: &str, head: &[u8]) -> AssetKind {
    let ext = file_name.rsplit('.').next().unwrap_or("");
    let by_ext = ext_kind(ext);
    if NON_IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return AssetKind::Other;
    }
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
        (Some(kind), _) => kind,
        (None, Some(kind)) => kind,
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

/// 设备 id 规范化（注册表 key 的单一事实源，2026-09-18 相机链路修复）：
/// 同一 WPD 设备在启动枚举（WPD GetDevices 产出小写）与热插 DBT/WPD
/// 到达（dbcc_name 常为大写）出现**大小写不同的 id**——Windows 设备路径
/// 大小写不敏感，PnP 形态统一转 ASCII 小写后，注册/查找/移除全链路命中
/// 同一 key（重复到达幂等、移除不漏）。
///
/// 仅归一 PnP 设备路径（`\\?\usb#...` / `\\?\swd#...` 等非文件系统形态）；
/// **文件系统路径必须原样**：卷盘符（"E:"）、`FOLDER:` 源（NTFS 大小写
/// 敏感，路径含 `#` 也不可小写化）、`\\?\C:\` verbatim 盘符与 `\\?\UNC\`
/// 网络路径均直接返回。
pub fn normalize_device_id(id: &str) -> String {
    if is_pnp_device_path(id) {
        id.to_ascii_lowercase()
    } else {
        id.to_string()
    }
}

/// PnP 设备路径判定：`\\?\` 前缀且后随非文件系统形态
///（排除 `\\?\C:\` 盘符与 `\\?\UNC\` 两类 verbatim 文件系统路径）。
fn is_pnp_device_path(id: &str) -> bool {
    let Some(rest) = id.strip_prefix(r"\\?\") else {
        return false;
    };
    let mut chars = rest.chars();
    if let Some(first) = chars.next() {
        // `\\?\C:\...`：verbatim 盘符路径，大小写敏感
        if first.is_ascii_alphabetic() && chars.next() == Some(':') {
            return false;
        }
    }
    // `\\?\UNC\server\share`：verbatim 网络路径
    if rest.len() >= 4 && rest[..4].eq_ignore_ascii_case("UNC\\") {
        return false;
    }
    true
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
        assert_eq!(classify("MVI_0001.MP4", &[]), AssetKind::Other);
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
            classify("a.heic", &head(b"\0\0\0\x18ftypmif1\0\0")),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.avif", &head(b"\0\0\0\x20ftypavif\0\0")),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.jxl", &head(&[0xFF, 0x0A, 0x78, 0x02])),
            AssetKind::Photo
        );
        assert_eq!(
            classify(
                "a.jxl",
                &head(&[0x00, 0x00, 0x00, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A])
            ),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.ico", &head(&[0x00, 0x00, 0x01, 0x00])),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.cur", &head(&[0x00, 0x00, 0x02, 0x00])),
            AssetKind::Photo
        );
        assert_eq!(
            classify("a.mp4", &head(b"\0\0\0\x20ftypisom\0\0")),
            AssetKind::Other
        );
        assert_eq!(
            classify("a.avi", &head(b"RIFF\x00\x00\x00\x00AVI ")),
            AssetKind::Other
        );
        assert_eq!(
            classify("a.mkv", &head(&[0x1A, 0x45, 0xDF, 0xA3])),
            AssetKind::Other
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
        assert_eq!(
            classify("fake.mp4", &head(&[0xFF, 0xD8, 0xFF, 0xE0])),
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
