//! 缩略图服务（M3 管线提前的核心块，2026-09-18 真机反馈：前端原图全量
//! 解码 30-60MB/张的 A7R5 JPG 导致卡顿）。
//!
//! 契约：`thumb_get_by_path(path, size)` 返回**缓存文件绝对路径**（前端经 asset
//! 协议加载）；无法生成（RAW/视频/读取失败/超时）返回 null。
//!
//! - 可解码集：image crate 支持的位图格式（JPG/PNG/WEBP/BMP/GIF/TIFF）；
//!   RAW（NEF/ARW/CR3…）与视频 v1 返回 null（RAW 内嵌预览提取留给 M3
//!   rawler 集成）。
//! - 缓存：`dbDir/thumbs/<档位>/<xxh64(路径小写)>-<mtime unixsecs>.jpg`
//!   （路径小写哈希：Windows 路径大小写不敏感；键含 mtime → 源变化自然
//!   miss）。命中直接返回，不重解码。
//! - 并发：同 path+size in-flight 合并（OnceLock 阻塞共享首次结果）；
//!   全局解码并发上限 2（CPU 密集）；解码超时 10s 放弃（超时后解码线程
//!   自行收尾并释放许可，不留坏缓存——落盘走 tmp+rename 原子写）。
//! - 尺寸档位 256/512（v1 前端用 256），请求值就近归档。
//! - 本模块不依赖 AppState（核心 `thumb_file(db_dir, path, size)` 可独立
//!   集成测试）；IPC 壳在 ipc::thumb。
//!
//! 已知取舍：v1 不做 EXIF 方向矫正、不主动清理陈旧缓存（键含 mtime，
//! 旧文件仅占盘不误命中）。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use xxhash_rust::xxh64::Xxh64;

use crate::metadata::exif_lite;

/// 可解码扩展名（小写；image crate 位图格式集）。
pub const DECODABLE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];
/// RAW 扩展名（内嵌 JPEG 预览提取；提取失败回退前端占位）。
pub const RAW_EXTS: &[&str] = &[
    "nef", "arw", "cr2", "cr3", "orf", "rw2", "dng", "raf", "pef", "srw",
];
/// 支持的缩略图档位（2048 供查看器原图回退链中继；缓存无上限 v1 接受，
/// M8 做 LRU——2048 档单张约 1-2MB）。
pub const SIZE_TIERS: &[u16] = &[256, 512, 2048];
/// 解码超时（61MP 解码 1-2s 可接受；>10s 视为失败放弃）。
const DECODE_TIMEOUT: Duration = Duration::from_secs(10);
const RAW_FULL_DECODE_TIMEOUT: Duration = Duration::from_secs(60);
/// 后台解码并发上限（用户定案 2026-09-19：索引任务吃满硬件）：
/// 物理核心数直通（turbojpeg SIMD 缩放解码近线性扩展；与导入/索引任务
/// 共享本池——导入优先/主图兜底优先由池的先到先得天然实现，不超发）。
#[doc(hidden)]
#[allow(dead_code)]
pub fn permit_count_for_test(cores: usize) -> u32 {
    permit_count(cores)
}

fn permit_count(cores: usize) -> u32 {
    cores.max(1) as u32
}

fn desired_permits() -> u32 {
    permit_count(
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4),
    )
}

/// turbojpeg 快路径成功解码计数（测试断言/运维观测）。
pub static TURBO_DECODES: AtomicUsize = AtomicUsize::new(0);

/// 真实解码次数（并发合并验证 / 运维统计；命中缓存不计数）。
pub static DECODE_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 请求尺寸就近归一到支持档位（|差|最小，平手取小档保守）。
pub fn snap_size(size: u16) -> u16 {
    let mut best = SIZE_TIERS[0];
    let mut best_diff = i32::abs(best as i32 - size as i32);
    for &tier in &SIZE_TIERS[1..] {
        let diff = i32::abs(tier as i32 - size as i32);
        if diff < best_diff {
            best = tier;
            best_diff = diff;
        }
    }
    best
}

/// 进程级服务态：in-flight 合并表 + 解码许可。
struct ThumbSvc {
    /// 缓存路径 → 首次结果（同 key 并发调用阻塞共享同一次生成）。
    inflight: Mutex<HashMap<PathBuf, Arc<OnceLock<Option<PathBuf>>>>>,
    permits: Arc<Permits>,
}

impl ThumbSvc {
    fn acquire(&self) -> PermitGuard {
        self.permits.acquire()
    }

    fn cell_for(&self, cache: &Path) -> Arc<OnceLock<Option<PathBuf>>> {
        let mut map = self
            .inflight
            .lock()
            .expect("thumbs inflight mutex poisoned");
        map.entry(cache.to_path_buf())
            .or_insert_with(|| Arc::new(OnceLock::new()))
            .clone()
    }

    /// 生成完成后移除表项（后续走缓存快路径，防 map 无界增长）。
    fn remove(&self, cache: &Path) {
        self.inflight
            .lock()
            .expect("thumbs inflight mutex poisoned")
            .remove(cache);
    }
}

fn service() -> &'static ThumbSvc {
    static SERVICE: OnceLock<ThumbSvc> = OnceLock::new();
    SERVICE.get_or_init(|| ThumbSvc {
        inflight: Mutex::new(HashMap::new()),
        permits: Arc::new(Permits::new(desired_permits())),
    })
}

/// 取（或生成）缩略图缓存文件路径。无法生成返回 None。
/// `db_dir` = 库 dbDir（缓存根）；源文件只读不动。
pub fn thumb_file(db_dir: &Path, src: &Path, size: u16) -> Option<String> {
    let size = snap_size(size);
    let ext = src.extension()?.to_str()?.to_ascii_lowercase();
    if !DECODABLE_EXTS.contains(&ext.as_str()) && !is_raw_ext(&ext) {
        return None; // 视频等永久不支持
    }
    if is_raw_ext(&ext) {
        // RAW：内嵌预览提取路径（同一缓存规则/同一超时与并发许可）
        let meta = fs::metadata(src).ok()?;
        if !meta.is_file() {
            return None;
        }
        let mtime = meta.modified().ok()?;
        let cache = cache_path(db_dir, src, size, mtime);
        if cache.exists() {
            return Some(cache.to_string_lossy().into_owned());
        }
        let svc = service();
        let cell = svc.cell_for(&cache);
        let result = cell.get_or_init(|| generate(&cache, src, size));
        svc.remove(&cache);
        return result.as_ref().map(|p| p.to_string_lossy().into_owned());
    }
    let meta = fs::metadata(src).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let cache = cache_path(db_dir, src, size, mtime);
    if cache.exists() {
        return Some(cache.to_string_lossy().into_owned()); // 命中，不重解码
    }

    // 同 key 并发合并：首个调用者生成，其余阻塞共享结果
    let svc = service();
    let cell = svc.cell_for(&cache);
    let result = cell.get_or_init(|| generate(&cache, src, size));
    svc.remove(&cache);
    result.as_ref().map(|p| p.to_string_lossy().into_owned())
}

/// 源是否可出缩略图（位图直解或 RAW 内嵌预览提取；入队前的廉价否决）。
pub fn is_decodable(src: &Path) -> bool {
    src.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        let e = e.to_ascii_lowercase();
        DECODABLE_EXTS.contains(&e.as_str()) || RAW_EXTS.contains(&e.as_str())
    })
}

/// 扩展名是否为 RAW（内嵌预览路径）。
fn is_raw_ext(ext: &str) -> bool {
    RAW_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// 缓存命中探测（不生成、不阻塞）：命中返回缓存文件绝对路径，未命中/
/// 不可解码/源缺失返回 None。按需管线（asset_thumb_get(asset_id)）的快路径。
pub fn cached(db_dir: &Path, src: &Path, size: u16) -> Option<String> {
    let size = snap_size(size);
    if !is_decodable(src) {
        return None;
    }
    let meta = fs::metadata(src).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let cache = cache_path(db_dir, src, size, mtime);
    cache.exists().then(|| cache.to_string_lossy().into_owned())
}

/// 缓存路径：`dbDir/thumbs/<档位>/<xxh64(路径小写)>-<mtime secs>.jpg`。
fn cache_path(db_dir: &Path, src: &Path, size: u16, mtime: SystemTime) -> PathBuf {
    let mut xxh = Xxh64::new(0);
    xxh.update(src.to_string_lossy().to_lowercase().as_bytes());
    let secs = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let tier = src
        .extension()
        .and_then(|e| e.to_str())
        .filter(|ext| size == 2048 && is_raw_ext(ext))
        .map(|_| "2048-raw-full-v1".to_string())
        .unwrap_or_else(|| size.to_string());
    db_dir
        .join("thumbs")
        .join(tier)
        .join(format!("{:016x}-{secs}.jpg", xxh.digest()))
}

/// 生成一枚缩略图：并发许可 + 解码线程 + 超时放弃。
fn generate(cache: &Path, src: &Path, size: u16) -> Option<PathBuf> {
    let permit = service().acquire();
    DECODE_COUNT.fetch_add(1, Ordering::SeqCst);

    let (tx, rx) = mpsc::channel();
    let src_display = src.display().to_string();
    let full_raw = src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| size >= 2048 && is_raw_ext(ext));
    let timeout = if full_raw {
        RAW_FULL_DECODE_TIMEOUT
    } else {
        DECODE_TIMEOUT
    };
    let src = src.to_path_buf();
    let cache = cache.to_path_buf();
    // 解码线程持有许可：超时放弃后它自行收尾并释放许可（不占死名额）。
    // 61MP 解码 1-2s；超时=异常文件/挂死，绝不留半份缓存。
    let decoder = std::thread::Builder::new()
        .name("thumb-decode".into())
        .spawn(move || {
            let _permit = permit;
            // RAW 全量显影单路执行：单张高像素 RAW 的 16-bit 中间缓冲很大，
            // 多张同时去马赛克会造成内存尖峰；单任务内部仍由 rawler 并行。
            let _raw_guard = full_raw.then(|| {
                static RAW_FULL_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
                RAW_FULL_LOCK
                    .get_or_init(|| Mutex::new(()))
                    .lock()
                    .expect("raw full decode mutex poisoned")
            });
            let out = decode_and_encode(&src, size).and_then(|jpeg| {
                write_atomic(&cache, &jpeg).ok()?;
                Some(cache.clone())
            });
            let _ = tx.send(out.clone());
            out
        })
        .expect("spawn thumb decoder");
    drop(decoder); // 不 join：超时后任其收尾

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => {
            eprintln!("缩略图解码超时（>{timeout:?}），放弃: {src_display}");
            None
        }
    }
}

/// 解码 → 拟合缩放 → JPEG q80。
/// JPEG 走 turbojpeg 缩放解码快路径（TJSCALED：DCT 域直出小图，61MP
/// 全量解码 1-3s → ~百 ms 级），失败回退 image crate 全量解码（turbojpeg
/// 拒收的怪 JPEG 兜底）；其余格式（PNG/GIF/BMP/TIFF/WEBP）走 image crate。
fn decode_and_encode(src: &Path, size: u16) -> Option<Vec<u8>> {
    let ext = src.extension()?.to_str()?.to_ascii_lowercase();
    // EXIF Orientation 生成时一次性转正（缓存里存的就是正的，前端零改动）
    let orientation = orientation_from_file(src).unwrap_or(1);
    let thumb: image::RgbImage = if is_raw_ext(&ext) {
        if size >= 2048 {
            // 查看器高清档：完整解码、去马赛克和色彩显影；失败时仍回退相机
            // 内嵌 JPEG，保证不因少数未支持机型失去预览。
            develop_raw_resize(src, size).or_else(|| {
                let preview = raw_preview_jpeg(src)?;
                Some(
                    jpeg_scaled_bytes(&preview, size)
                        .unwrap_or_else(|| full_decode_bytes_resize(&preview, size)),
                )
            })?
        } else {
            // 低清档优先内嵌 JPEG，快速给出首帧。
            let preview = raw_preview_jpeg(src)?;
            jpeg_scaled_bytes(&preview, size)
                .unwrap_or_else(|| full_decode_bytes_resize(&preview, size))
        }
    } else if matches!(ext.as_str(), "jpg" | "jpeg") {
        jpeg_scaled(src, size).unwrap_or_else(|| full_decode_resize(src, size))
    } else {
        full_decode_resize(src, size)
    };
    let thumb = apply_orientation(thumb, orientation);
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80);
    thumb.write_with_encoder(encoder).ok()?;
    Some(jpeg)
}

/// RAW 全量显影为屏幕预览。输出在编码前缩到查看器档位，避免把数千万像素的
/// 16-bit 中间图继续留在缓存链中；失败由调用方回退内嵌 JPEG。
fn develop_raw_resize(src: &Path, size: u16) -> Option<image::RgbImage> {
    use rawler::imgop::develop::RawDevelop;

    let raw = rawler::decode_file(src).ok()?;
    let developed = RawDevelop::default().develop_intermediate(&raw).ok()?;
    let image = developed.to_dynamic_image()?;
    Some(image.thumbnail(size as u32, size as u32).to_rgb8())
}

/// 从文件头解析 EXIF Orientation（JPEG APP1 / RAW TIFF IFD0 同源）。
fn orientation_from_file(src: &Path) -> Option<u32> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(1024 * 1024);
    fs::File::open(src)
        .ok()?
        .take(1024 * 1024)
        .read_to_end(&mut buf)
        .ok()?;
    exif_lite::parse_orientation(&buf)
}

/// 按 EXIF Orientation 1-8 转正像素（5/7 为转置组合，6/8 宽高互换）。
fn apply_orientation(img: image::RgbImage, orientation: u32) -> image::RgbImage {
    use image::imageops;
    match orientation {
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => imageops::flip_horizontal(&imageops::rotate90(&img)),
        6 => imageops::rotate90(&img),
        7 => imageops::flip_vertical(&imageops::rotate90(&img)),
        8 => imageops::rotate270(&img),
        _ => img,
    }
}

/// image crate 全量解码 + thumbnail 拟合（慢路径/非 JPEG）。解码失败返回
/// 空图（0x0）使后续编码失败 → 整体 None。
fn full_decode_resize(src: &Path, size: u16) -> image::RgbImage {
    match image::ImageReader::open(src)
        .ok()
        .and_then(|r| r.decode().ok())
    {
        Some(img) => img.thumbnail(u32::from(size), u32::from(size)).to_rgb8(),
        None => image::RgbImage::new(0, 0),
    }
}

/// turbojpeg 缩放解码：按源尺寸选最小满足目标的缩放档（1/8..1/1，
/// DCT 域直出），再 thumbnail 精确拟合到目标尺寸。
fn jpeg_scaled(src: &Path, target: u16) -> Option<image::RgbImage> {
    let data = fs::read(src).ok()?;
    jpeg_scaled_bytes(&data, target)
}

fn jpeg_scaled_bytes(data: &[u8], target: u16) -> Option<image::RgbImage> {
    let header = turbojpeg::read_header(data).ok()?;
    let factor =
        turbojpeg::ScalingFactor::new(pick_jpeg_scale(header.width, header.height, target), 8);
    let width = factor.scale(header.width);
    let height = factor.scale(header.height);
    let mut decompressor = turbojpeg::Decompressor::new().ok()?;
    decompressor.set_scaling_factor(factor).ok()?;
    let mut pixels = vec![0u8; width * height * turbojpeg::PixelFormat::RGB.size()];
    let mut image = turbojpeg::Image {
        pixels: &mut pixels[..],
        width,
        pitch: width * turbojpeg::PixelFormat::RGB.size(),
        height,
        format: turbojpeg::PixelFormat::RGB,
    };
    decompressor.decompress(data, image.as_deref_mut()).ok()?;
    let scaled = image::RgbImage::from_raw(width as u32, height as u32, pixels)?;
    TURBO_DECODES.fetch_add(1, Ordering::SeqCst);
    Some(
        image::DynamicImage::ImageRgb8(scaled)
            .thumbnail(u32::from(target), u32::from(target))
            .to_rgb8(),
    )
}

/// 测试钩子：thumbs 目录下的 .tmp 残留（原子写失败/中断的半成品）。
#[doc(hidden)]
#[allow(dead_code)]
pub fn find_tmp_residue(db_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = fs::read_dir(db_dir.join("thumbs")) {
        for tier in entries.flatten() {
            if let Ok(files) = fs::read_dir(tier.path()) {
                out.extend(
                    files
                        .flatten()
                        .map(|f| f.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "tmp")),
                );
            }
        }
    }
    out
}

/// 基准/测试钩子：turbojpeg 缩放解码到 RgbImage（bench 用例引用）。
#[doc(hidden)]
#[allow(dead_code)]
pub fn bench_scaled_rgb(src: &Path, target: u16) -> Option<image::RgbImage> {
    jpeg_scaled(src, target)
}

/// 依源尺寸选 turbojpeg 缩放档分子（num/8，num 越小图越小）：
/// 从 1/8 起找**最小**档位使缩放后两边仍 ≥ target（后续精确拟合有足够
/// 像素）；源小于目标时返回 8（原尺寸，避免上采样糊）。
/// 例：9504px→256 档 → 1/8（1188px）；1000px→256 → 3/8（375px）；
/// 200px→256 → 8（原尺寸）。
pub fn pick_jpeg_scale(width: usize, height: usize, target: u16) -> usize {
    let target = target as usize;
    for num in 1..=8usize {
        let sw = width * num / 8;
        let sh = height * num / 8;
        if sw >= target && sh >= target {
            return num;
        }
    }
    8
}

/// 原子落盘：tmp 写满后 rename（失败不留坏缓存）。
fn write_atomic(cache: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = cache.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = cache.with_extension("jpg.tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, cache)
}

/// 全量解码字节源（RAW 预览 JPEG 的慢路径兜底）。
fn full_decode_bytes_resize(data: &[u8], size: u16) -> image::RgbImage {
    match image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.decode().ok())
    {
        Some(img) => img.thumbnail(u32::from(size), u32::from(size)).to_rgb8(),
        None => image::RgbImage::new(0, 0),
    }
}

/// RAW 内嵌预览提取上限（预览 JPEG 通常 <5MB；留足余量）。
const RAW_SCAN_LIMIT: u64 = 256 * 1024 * 1024;

/// 在 RAW 文件字节里定位第一段「结构完整」的内嵌 JPEG：
/// SOI(FFD8FF) → 段链走到 SOS → 熵编码后首个 FFD9 即 EOI（熵数据中
/// FF00 转义保证 FFD9 不会出现在内部）。找到 SOF（尺寸>0）才算有效。
/// 找不到/结构坏返回 None（前端占位兜底）。
pub fn raw_preview_jpeg(src: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(src).ok()?;
    if meta.len() > RAW_SCAN_LIMIT {
        return None;
    }
    let data = fs::read(src).ok()?;
    let mut from = 0usize;
    while let Some(rel) = data[from..]
        .windows(3)
        .position(|w| w == [0xFF, 0xD8, 0xFF])
    {
        let soi = from + rel;
        if let Some(end) = jpeg_segment_end(&data[soi..]) {
            return Some(data[soi..soi + end].to_vec());
        }
        from = soi + 2;
        if from + 3 > data.len() {
            return None;
        }
    }
    None
}

/// 从 SOI 起走段链：返回含 EOI 的完整 JPEG 长度；结构非法返回 None。
fn jpeg_segment_end(buf: &[u8]) -> Option<usize> {
    let mut i = 2usize; // 跳过 SOI
    for _ in 0..256 {
        while i + 1 < buf.len() && buf[i] == 0xFF && buf[i + 1] == 0xFF {
            i += 1; // 填充
        }
        if i + 2 > buf.len() || buf[i] != 0xFF {
            return None;
        }
        let marker = buf[i + 1];
        match marker {
            0x01 | 0xD0..=0xD7 => i += 2,
            0xD8 => return None,        // 嵌套 SOI：坏结构
            0xD9 => return Some(i + 2), // EOI
            0xDA => {
                // SOS：其后熵编码内不会有 FFD9（FF00 转义），线性找 EOI
                let mut j = i + 2 + seg_len_u16(buf, i)?;
                while j + 1 < buf.len() {
                    if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                        return Some(j + 2);
                    }
                    j += 1;
                }
                return None;
            }
            0xC4 | 0xC8 | 0xCC => i += 2 + seg_len_u16(buf, i)?,
            0xC0..=0xCF => {
                // SOF：粗验尺寸>0
                let len = seg_len_u16(buf, i)?;
                if len < 7 || i + 2 + len > buf.len() {
                    return None;
                }
                let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
                let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]);
                if w == 0 || h == 0 {
                    return None;
                }
                i += 2 + len;
            }
            _ => i += 2 + seg_len_u16(buf, i)?,
        }
    }
    None
}

fn seg_len_u16(buf: &[u8], i: usize) -> Option<usize> {
    let hi = *buf.get(i + 2)?;
    let lo = *buf.get(i + 3)?;
    Some(usize::from(u16::from_be_bytes([hi, lo])))
}

// ---------------------------------------------------------------------------
// 简易计数信号量（std only；解码并发上限）
// ---------------------------------------------------------------------------

/// 计数信号量（std Mutex+Condvar）。守卫 `Send`，可移入解码线程持有
/// （超时放弃后由解码线程释放名额，不占死并发额度）。
struct Permits {
    state: Mutex<u32>,
    cv: Condvar,
}

impl Permits {
    fn new(count: u32) -> Self {
        Self {
            state: Mutex::new(count),
            cv: Condvar::new(),
        }
    }

    fn acquire(self: &Arc<Self>) -> PermitGuard {
        let mut left = self.state.lock().expect("permits mutex poisoned");
        while *left == 0 {
            left = self.cv.wait(left).expect("permits mutex poisoned");
        }
        *left -= 1;
        PermitGuard {
            permits: Arc::clone(self),
        }
    }
}

/// 许可守卫：drop 归还名额并唤醒一个等待者。
struct PermitGuard {
    permits: Arc<Permits>,
}

impl Drop for PermitGuard {
    fn drop(&mut self) {
        let mut left = self.permits.state.lock().expect("permits mutex poisoned");
        *left += 1;
        self.permits.cv.notify_one();
    }
}
