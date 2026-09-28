//! 缩略图服务（M3 管线提前的核心块，2026-09-18 真机反馈：前端原图全量
//! 解码 30-60MB/张的 A7R5 JPG 导致卡顿）。
//!
//! 契约：`thumb_get_by_path(path, size)` 返回**缓存文件绝对路径**（前端经 asset
//! 协议加载）；无法生成（RAW/读取失败/超时）返回 null。
//!
//! - 可解码集：image crate 支持的位图格式（JPG/PNG/WEBP/BMP/GIF/TIFF）；
//!   RAW（NEF/ARW/CR3…）走内嵌 JPEG 预览提取。
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

/// RAW 缩略图源代际：v2 = 取「最大」内嵌 JPEG 为源（v1 取第一段=小缩略图，
/// 全线偏糊）。低档目录名带代际后缀，升代际即失效重建（照片档不受影响）。
pub const RAW_THUMB_GENERATION: u32 = 2;

/// 内嵌全幅直出档语义标记：RAW 且请求 size > 2048 → 提取最大内嵌 JPEG
/// 原样直出（orientation=1 零重编码）。Windows 照片看 RAW 就是这条路——
/// 相机自己渲染的全幅预览，毫秒级 IO，无需去马赛克显影。
fn is_raw_embed_request(ext: &str, size: u16) -> bool {
    size > 2048 && is_raw_ext(ext)
}

/// 取（或生成）缩略图缓存文件路径。无法生成返回 None。
/// `db_dir` = 库 dbDir（缓存根）；源文件只读不动。
pub fn thumb_file(db_dir: &Path, src: &Path, size: u16) -> Option<String> {
    let ext = src.extension()?.to_str()?.to_ascii_lowercase();
    if !DECODABLE_EXTS.contains(&ext.as_str()) && !is_raw_ext(&ext) {
        return None; // 其他类型永久不支持
    }
    // 内嵌直出档不 snap（>2048 是语义标记而非目标边长）
    let size = if is_raw_embed_request(&ext, size) {
        size
    } else {
        snap_size(size)
    };
    let meta = fs::metadata(src).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let cache = cache_path(db_dir, src, size, mtime);
    if cache.exists() {
        touch_lru(&cache); // 命中续命（LRU：mtime 即热度账）
        return Some(cache.to_string_lossy().into_owned()); // 命中，不重解码
    }

    // 同 key 并发合并：首个调用者生成，其余阻塞共享结果
    let svc = service();
    let cell = svc.cell_for(&cache);
    let result = cell.get_or_init(|| generate(&cache, src, size));
    svc.remove(&cache);
    if result.is_some() {
        note_generation(db_dir); // 每 N 次生成机会式触发 LRU 淘汰检查
    }
    result.as_ref().map(|p| p.to_string_lossy().into_owned())
}

/// 缓存 mtime 续命（touch）。失败静默（LRU 只是优化，非正确性依赖）。
fn touch_lru(cache: &Path) {
    if let Ok(file) = fs::File::options().write(true).open(cache) {
        let _ = file.set_modified(SystemTime::now());
    }
}

// ---------------------------------------------------------------------------
// LRU 缓存上限（M8-③）
// ---------------------------------------------------------------------------

/// 缓存上限（字节；0 = 不限）。启动/settings_set 时刷新。
static THUMB_CACHE_CAP: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(20 * 1024 * 1024 * 1024);
/// 机会式检查步长：每 N 次生成检查一次（淘汰本身再退线程，不阻塞生成）。
const EVICT_CHECK_EVERY: u64 = 16;
/// 淘汰时正在写入的保护窗（秒）：60s 内新 mtime 的文件跳过。
const EVICT_MIN_AGE_SECS: u64 = 60;

/// 刷新缓存上限（settings 加载 / settings_set 调用；gb=0 不限）。
pub fn set_thumb_cache_cap_bytes(bytes: u64) {
    THUMB_CACHE_CAP.store(bytes, std::sync::atomic::Ordering::Relaxed);
}

/// 当前上限（字节；0 不限）。
pub fn thumb_cache_cap_bytes() -> u64 {
    THUMB_CACHE_CAP.load(std::sync::atomic::Ordering::Relaxed)
}

/// 生成计数：每 EVICT_CHECK_EVERY 次机会式派一次后台淘汰。
fn note_generation(db_dir: &Path) {
    use std::sync::atomic::AtomicU64;
    static COUNT: std::sync::OnceLock<AtomicU64> = std::sync::OnceLock::new();
    let count = COUNT.get_or_init(|| AtomicU64::new(0));
    if count
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        .is_multiple_of(EVICT_CHECK_EVERY)
    {
        kick_evict(db_dir);
    }
}

/// 超限则后台线程执行 LRU 淘汰（绝不碰 UI/生成线程——spawn 即返回）。
fn kick_evict(db_dir: &Path) {
    let cap = thumb_cache_cap_bytes();
    if cap == 0 {
        return; // 不限
    }
    let db_dir = db_dir.to_path_buf();
    std::thread::Builder::new()
        .name("thumb-lru-evict".into())
        .spawn(move || {
            evict_lru(&db_dir, cap, EVICT_MIN_AGE_SECS, SystemTime::now());
        })
        .ok();
}

/// 启动扫一次（后台线程；上限 0 不做）。与 kick_evict 同走裸线程——
/// 淘汰是幂等 IO 清理，无需 supervisor 生命周期管理（也更利于直连
/// thumbs 模块的测试编译）。
pub fn kick_startup_evict(db_dir: PathBuf) {
    let cap = thumb_cache_cap_bytes();
    if cap == 0 {
        return;
    }
    std::thread::Builder::new()
        .name("thumb-lru-startup".into())
        .spawn(move || {
            let (deleted, freed) = evict_lru(&db_dir, cap, EVICT_MIN_AGE_SECS, SystemTime::now());
            if deleted > 0 {
                eprintln!("[thumbs] LRU 启动清理：删 {deleted} 个缓存，释放 {freed} 字节");
            }
        })
        .ok();
}

/// LRU 淘汰执行体（可测：min_age/now 可注入）。扫 dbDir/thumbs 递归汇总，
/// 超上限按 mtime 升序删到 ≤ cap；跳过 mtime 距 now < min_age 的文件
/// （正在写入/刚生成的保护窗）。返回 (删除文件数, 释放字节)。
pub fn evict_lru(db_dir: &Path, cap_bytes: u64, min_age_secs: u64, now: SystemTime) -> (u64, u64) {
    let thumbs = db_dir.join("thumbs");
    if !thumbs.is_dir() {
        return (0, 0);
    }
    // 汇总（路径, mtime, 大小）
    let mut entries: Vec<(PathBuf, SystemTime, u64)> = Vec::new();
    let mut total = 0u64;
    for item in walkdir::WalkDir::new(&thumbs).into_iter().flatten() {
        let Ok(meta) = item.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let (Ok(mtime), size) = (meta.modified(), meta.len()) else {
            continue;
        };
        total += size;
        entries.push((item.into_path(), mtime, size));
    }
    if total <= cap_bytes {
        return (0, 0);
    }
    let min_age = std::time::Duration::from_secs(min_age_secs);
    entries.sort_by_key(|(_, mtime, _)| *mtime); // 最旧先删
    let mut freed = 0u64;
    let mut deleted = 0u64;
    for (path, mtime, size) in entries {
        if total - freed <= cap_bytes {
            break;
        }
        if now
            .duration_since(mtime)
            .map(|d| d < min_age)
            .unwrap_or(true)
        {
            continue; // 保护窗内（mtime 异常按保护处理）
        }
        if fs::remove_file(&path).is_ok() {
            freed += size;
            deleted += 1;
        }
    }
    (deleted, freed)
}

/// 源是否可出缩略图（位图直解 / RAW 内嵌预览提取）；入队前的廉价否决。
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
    let ext = src
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    let size = if is_raw_embed_request(&ext, size) {
        size
    } else {
        snap_size(size)
    };
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
        .filter(|ext| is_raw_ext(ext))
        .map(|ext| {
            if is_raw_embed_request(ext, size) {
                "raw-embed-v1".to_string() // 内嵌全幅直出（相机渲染，零显影）
            } else if size == 2048 {
                "2048-raw-full-v1".to_string() // rawler 显影（无内嵌预览机型兜底）
            } else {
                // 低档带源代际：升代际自动失效重建
                format!("raw-{size}-v{RAW_THUMB_GENERATION}")
            }
        })
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
    // 仅 2048 显影档走重管护（单飞锁 + 60s 超时）；内嵌直出档是纯 IO
    // （orientation=1 零重编码）或一次常规解码转正，普通超时即可
    let full_raw = src
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| size == 2048 && is_raw_ext(ext));
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
    if is_raw_embed_request(&ext, size) {
        // 内嵌全幅直出：最大内嵌 JPEG。orientation=1 零重编码原样落盘
        // （毫秒级）；带方向优先 JPEG 无损变换（重排 DCT 块不解码，百 ms 级），
        // MCU 不整除（perfect 失败）才全幅解码转正重编（秒级慢路径）。
        let preview = raw_preview_jpeg(src)?;
        if orientation == 1 {
            return Some(preview);
        }
        if let Some(rotated) = jpeg_lossless_transform(&preview, orientation) {
            return Some(rotated);
        }
        let img = full_decode_bytes(&preview)?;
        let img = apply_orientation(img, orientation);
        let mut jpeg = Vec::new();
        let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92);
        img.write_with_encoder(encoder).ok()?;
        return Some(jpeg);
    }
    let thumb: image::RgbImage = if is_raw_ext(&ext) {
        if size == 2048 {
            // 显影兜底档（无内嵌预览机型）：完整解码、去马赛克和色彩显影；
            // 失败时仍回退相机内嵌 JPEG，保证不因少数未支持机型失去预览。
            develop_raw_resize(src, size).or_else(|| {
                let preview = raw_preview_jpeg(src)?;
                Some(
                    jpeg_scaled_bytes(&preview, size)
                        .unwrap_or_else(|| full_decode_bytes_resize(&preview, size)),
                )
            })?
        } else {
            // 低清档优先内嵌 JPEG（最大段），快速给出首帧。
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

/// 全量解码字节源（原尺寸，内嵌直出档转正重编用）。
fn full_decode_bytes(data: &[u8]) -> Option<image::RgbImage> {
    let img = image::ImageReader::new(std::io::Cursor::new(data))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.decode().ok())?;
    if img.width() == 0 || img.height() == 0 {
        return None;
    }
    Some(img.to_rgb8())
}

/// RAW 内嵌预览提取上限（预览 JPEG 通常 <5MB；留足余量）。
const RAW_SCAN_LIMIT: u64 = 256 * 1024 * 1024;

/// 在 RAW 文件里提取**最大**的内嵌 JPEG。两级策略：
/// ① **TIFF 指针定位（范围读）**：NEF/ARW/CR2/DNG 等 TIFF 基容器里，
///    缩略图/预览都由 IFD 链引用（JPEGInterchangeFormat 或 compression
///    6/7 的 StripOffsets）——只读 IFD 结构（KB 级）+ 头窗口验尺寸 +
///    载荷精确范围读。NAS 61MP ARW 实测：73MB 全读 → ~2.6MB（28 倍）。
/// ② 兜底：SOI 全文件扫描（非 TIFF 基/指针缺失的怪容器）。
/// 两级都按像素数取最大；NEF/ARW 里第一段常是 160px 小缩略图，取第一段
/// 会让整条管线建立在最糊的源上（真机踩坑 2026-09-20）。
pub fn raw_preview_jpeg(src: &Path) -> Option<Vec<u8>> {
    let meta = fs::metadata(src).ok()?;
    if meta.len() > RAW_SCAN_LIMIT {
        return None;
    }
    let mut file = fs::File::open(src).ok()?;
    if let Some(jpeg) = preview_via_tiff(&mut file) {
        return Some(jpeg);
    }
    let data = fs::read(src).ok()?;
    scan_best_embedded(&data)
}

/// SOI 全文件扫描路径（兜底）：逐个候选走段链验证，取像素最大者。
pub fn scan_best_embedded(data: &[u8]) -> Option<Vec<u8>> {
    let mut from = 0usize;
    let mut best: Option<(u64, usize, usize)> = None; // (pixels, soi, len)
    let mut candidates = 0usize;
    while let Some(rel) = data[from..]
        .windows(3)
        .position(|w| w == [0xFF, 0xD8, 0xFF])
    {
        let soi = from + rel;
        if let Some((end, w, h)) = jpeg_segment_end(&data[soi..]) {
            let pixels = u64::from(w) * u64::from(h);
            if best.is_none_or(|(p, _, _)| pixels > p) {
                best = Some((pixels, soi, end));
            }
        }
        from = soi + 2;
        if from + 3 > data.len() {
            break;
        }
        candidates += 1;
        if candidates >= 16 {
            break; // 结构异常文件防御：不无限扫
        }
    }
    best.map(|(_, soi, end)| data[soi..soi + end].to_vec())
}

// --- TIFF 指针定位（范围读） --------------------------------------------------------

/// 精确范围读（seek + read_exact；长度夹取到文件内）。
fn read_at<R: std::io::Read + std::io::Seek>(r: &mut R, off: u64, len: u64) -> Option<Vec<u8>> {
    r.seek(std::io::SeekFrom::Start(off)).ok()?;
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// JPEG 头部尺寸解析（走到 SOS 即可，无需 EOI；范围读候选的廉价验证）。
pub fn jpeg_header_dims(buf: &[u8]) -> Option<(u32, u32)> {
    if buf.len() < 4 || buf[..3] != [0xFF, 0xD8, 0xFF] {
        return None;
    }
    let mut i = 2usize;
    for _ in 0..64 {
        while i + 1 < buf.len() && buf[i] == 0xFF && buf[i + 1] == 0xFF {
            i += 1;
        }
        if i + 4 > buf.len() || buf[i] != 0xFF {
            return None;
        }
        let marker = buf[i + 1];
        if marker == 0xDA || marker == 0xD9 {
            return None; // 头窗口内没遇到 SOF
        }
        let len = seg_len_u16(buf, i)?;
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            if len < 7 || i + 2 + len > buf.len() {
                return None;
            }
            let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
            let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]);
            return (w > 0 && h > 0).then_some((u32::from(w), u32::from(h)));
        }
        i += 2 + len;
    }
    None
}

/// TIFF IFD 走读产物：一个「JPEG 数据段」的完整布局（多 strip 拼接为连续段）。
#[derive(Debug, Clone)]
struct TiffJpegBlock {
    /// 连续段：单一 (offset, len)；多 strip：各段依次拼接
    parts: Vec<(u64, u64)>,
    total: u64,
}

/// 走 IFD0 → NextIFD 链 + SubIFD(0x014A)，收集全部内嵌 JPEG 数据段。
/// 防御：IFD 数 ≤64、访问去重、SubIFD 深度 ≤4；非经典 TIFF（BigTIFF 等）
/// 返回 None 走兜底扫描。
fn tiff_jpeg_blocks<R: std::io::Read + std::io::Seek>(r: &mut R) -> Option<Vec<TiffJpegBlock>> {
    let mut head = [0u8; 8];
    r.read_exact(&mut head).ok()?;
    let little = match &head[..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    if u16_val(&head[2..4], little) != Some(42) {
        return None; // BigTIFF（magic 43）等不支持
    }
    let ifd0 = u32_val(&head[4..8], little)? as u64;

    let mut blocks = Vec::new();
    let mut visited = std::collections::HashSet::new();
    // (offset, is_root)：NextIFD 链延续同级遍历；SubIFD 深度受限
    let mut queue: Vec<(u64, u32)> = vec![(ifd0, 0)];
    while let Some((off, depth)) = queue.pop() {
        if depth > 4 || !visited.insert(off) || blocks.len() >= 64 {
            continue;
        }
        let mut ifd_buf = [0u8; 2];
        if r.seek(std::io::SeekFrom::Start(off)).is_err() || r.read_exact(&mut ifd_buf).is_err() {
            continue;
        }
        let count = u16_val(&ifd_buf, little)? as usize;
        if count == 0 || count > 512 {
            continue;
        }
        let mut entries = vec![0u8; count * 12 + 4];
        if r.read_exact(&mut entries).is_err() {
            continue;
        }
        let mut compression = None;
        let mut strip_offsets: Vec<u64> = Vec::new();
        let mut strip_counts: Vec<u64> = Vec::new();
        let mut jpg_off: Option<u64> = None;
        let mut jpg_len: Option<u64> = None;
        for i in 0..count {
            let e = &entries[i * 12..i * 12 + 12];
            let tag = u16_val(&e[..2], little)?;
            let typ = u16_val(&e[2..4], little)?;
            let n = u32_val(&e[4..8], little)? as usize;
            let field = &e[8..12];
            let read_arr = |r: &mut R, field: &[u8], typ: u16, n: usize| -> Option<Vec<u64>> {
                let unit: usize = match typ {
                    1 | 2 | 6 | 7 => 1,
                    3 | 8 => 2,
                    4 | 9 | 11 => 4,
                    _ => return None,
                };
                let size = unit.checked_mul(n)?;
                let data: Vec<u8> = if size <= 4 {
                    field[..size].to_vec()
                } else {
                    let off = u32_val(field, little)? as u64;
                    read_at(r, off, size as u64)?
                };
                let mut vals = Vec::with_capacity(n);
                for k in 0..n {
                    let v = match unit {
                        1 => u64::from(data[k]),
                        2 => u64::from(u16_val(&data[k * 2..k * 2 + 2], little)?),
                        _ => u64::from(u32_val(&data[k * 4..k * 4 + 4], little)?),
                    };
                    vals.push(v);
                }
                Some(vals)
            };
            match tag {
                0x0103 => compression = u16_val(field, little),
                0x0111 => strip_offsets = read_arr(r, field, typ, n).unwrap_or_default(),
                0x0117 => strip_counts = read_arr(r, field, typ, n).unwrap_or_default(),
                0x0201 => jpg_off = read_arr(r, field, typ, n).and_then(|v| v.first().copied()),
                0x0202 => jpg_len = read_arr(r, field, typ, n).and_then(|v| v.first().copied()),
                0x014A => {
                    if let Some(subs) = read_arr(r, field, typ, n) {
                        for s in subs {
                            queue.push((s, depth + 1));
                        }
                    }
                }
                _ => {}
            }
        }
        // NextIFD 链（entries 之后的 4 字节）
        if let Some(next) = u32_val(&entries[count * 12..count * 12 + 4], little) {
            if next > 0 {
                queue.push((u64::from(next), depth));
            }
        }
        // 本 IFD 的 JPEG 数据段
        if let (Some(off), Some(len)) = (jpg_off, jpg_len) {
            if len > 0 && len <= 64 * 1024 * 1024 {
                blocks.push(TiffJpegBlock {
                    parts: vec![(off, len)],
                    total: len,
                });
            }
        } else if compression == Some(6) || compression == Some(7) {
            let parts: Vec<(u64, u64)> = strip_offsets
                .iter()
                .zip(strip_counts.iter())
                .filter(|(_, &c)| c > 0 && c <= 64 * 1024 * 1024)
                .map(|(&o, &c)| (o, c))
                .collect();
            if !parts.is_empty() {
                let total = parts.iter().map(|(_, c)| c).sum();
                blocks.push(TiffJpegBlock { parts, total });
            }
        }
    }
    (!blocks.is_empty()).then_some(blocks)
}

fn u16_val(b: &[u8], little: bool) -> Option<u16> {
    let arr: [u8; 2] = b.try_into().ok()?;
    Some(if little {
        u16::from_le_bytes(arr)
    } else {
        u16::from_be_bytes(arr)
    })
}

fn u32_val(b: &[u8], little: bool) -> Option<u32> {
    let arr: [u8; 4] = b.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(arr)
    } else {
        u32::from_be_bytes(arr)
    })
}

/// JPEG 无损变换（EXIF Orientation 1-8 → turbojpeg DCT 域重排；不解码
/// 不重编，60MP 亦百 ms 级）。perfect=true：宽高不整除 MCU（典型 4:2:0 为
/// 16px）时报错 → 调用方回退解码转正重编慢路径（相机全幅尺寸几乎都整除）。
fn jpeg_lossless_transform(data: &[u8], orientation: u32) -> Option<Vec<u8>> {
    use turbojpeg::{Transform, TransformOp};
    let op = match orientation {
        2 => TransformOp::Hflip,
        3 => TransformOp::Rot180,
        4 => TransformOp::Vflip,
        5 => TransformOp::Transpose,
        6 => TransformOp::Rot90,
        7 => TransformOp::Transverse,
        8 => TransformOp::Rot270,
        _ => return None,
    };
    let mut transform = Transform::op(op);
    transform.perfect = true;
    let out = turbojpeg::transform(&transform, data).ok()?;
    let vec = out.to_vec();
    (!vec.is_empty()).then_some(vec)
}

/// TIFF 指针路线：定位全部候选 → 头窗口验尺寸取最大 → 精确读载荷。
pub fn preview_via_tiff<R: std::io::Read + std::io::Seek>(r: &mut R) -> Option<Vec<u8>> {
    let blocks = tiff_jpeg_blocks(r)?;
    let mut best: Option<(u64, &TiffJpegBlock)> = None;
    for block in &blocks {
        let (off, len) = block.parts[0];
        let window = read_at(r, off, len.min(256 * 1024))?;
        let Some((w, h)) = jpeg_header_dims(&window) else {
            continue;
        };
        let pixels = u64::from(w) * u64::from(h);
        if best.is_none_or(|(p, _)| pixels > p) {
            best = Some((pixels, block));
        }
    }
    let (_, block) = best?;
    let mut jpeg = Vec::with_capacity(block.total as usize);
    for (off, len) in &block.parts {
        jpeg.extend_from_slice(&read_at(r, *off, *len)?);
    }
    // 载荷完整性验证：SOI 起手且头可解析（防 TIFF 脏指针读出垃圾）
    jpeg_header_dims(&jpeg)?;
    Some(jpeg)
}

/// 从 SOI 起走段链：返回 (含 EOI 的完整 JPEG 长度, SOF 宽, SOF 高)；
/// 结构非法返回 None。
fn jpeg_segment_end(buf: &[u8]) -> Option<(usize, u16, u16)> {
    let mut i = 2usize; // 跳过 SOI
    let mut sof = (0u16, 0u16);
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
            0xD8 => return None,                        // 嵌套 SOI：坏结构
            0xD9 => return Some((i + 2, sof.0, sof.1)), // EOI
            0xDA => {
                // SOS：其后熵编码内不会有 FFD9（FF00 转义），线性找 EOI
                let mut j = i + 2 + seg_len_u16(buf, i)?;
                while j + 1 < buf.len() {
                    if buf[j] == 0xFF && buf[j + 1] == 0xD9 {
                        return Some((j + 2, sof.0, sof.1));
                    }
                    j += 1;
                }
                return None;
            }
            0xC4 | 0xC8 | 0xCC => i += 2 + seg_len_u16(buf, i)?,
            0xC0..=0xCF => {
                // SOF：粗验尺寸>0（记录尺寸供调用方选最大段）
                let len = seg_len_u16(buf, i)?;
                if len < 7 || i + 2 + len > buf.len() {
                    return None;
                }
                let h = u16::from_be_bytes([buf[i + 5], buf[i + 6]]);
                let w = u16::from_be_bytes([buf[i + 7], buf[i + 8]]);
                if w == 0 || h == 0 {
                    return None;
                }
                sof = (w, h);
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
