//! 元数据提取（spec §5.3）：M1 T6 实现 EXIF-lite（DateTimeOriginal + 相机型号），
//! 完整提取（rawler/ffprobe）在 M3。

pub mod exif_lite;
pub mod xmp;
