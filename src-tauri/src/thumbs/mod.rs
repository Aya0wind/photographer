//! 缩略图服务（M3 管线提前的核心块，2026-09-18 真机反馈：前端原图全量
//! 解码 30-60MB/张的 A7R5 JPG 导致卡顿）。
//!
//! 契约：`thumb_get(path, size)` 返回**缓存文件绝对路径**（前端经 asset
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

/// 可解码扩展名（小写；image crate 位图格式集）。
pub const DECODABLE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "gif", "tif", "tiff"];
/// 支持的缩略图档位。
pub const SIZE_TIERS: &[u16] = &[256, 512];
/// 解码超时（61MP 解码 1-2s 可接受；>10s 视为失败放弃）。
const DECODE_TIMEOUT: Duration = Duration::from_secs(10);
/// 后台解码并发上限（CPU 密集）：动态 = min(6, 可用核数/2)，下限 1
///（turbojpeg 缩放解码后单张成本大降，允许更高并行吃满多核；
/// 8 核 → 4，12 核 → 6，4 核 → 2）。
#[doc(hidden)]
#[allow(dead_code)]
pub fn permit_count_for_test(cores: usize) -> u32 {
    permit_count(cores)
}

fn permit_count(cores: usize) -> u32 {
    (cores as u32 / 2).clamp(1, 6)
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
    if !DECODABLE_EXTS.contains(&ext.as_str()) {
        return None; // RAW/视频/未知格式 v1 不支持
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

/// 源是否在可解码集内（扩展名预检；入队前的廉价否决）。
pub fn is_decodable(src: &Path) -> bool {
    src.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DECODABLE_EXTS.contains(&e.to_ascii_lowercase().as_str()))
}

/// 缓存命中探测（不生成、不阻塞）：命中返回缓存文件绝对路径，未命中/
/// 不可解码/源缺失返回 None。按需管线（thumb_get(asset_id)）的快路径。
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
    db_dir
        .join("thumbs")
        .join(size.to_string())
        .join(format!("{:016x}-{secs}.jpg", xxh.digest()))
}

/// 生成一枚缩略图：并发许可 + 解码线程 + 超时放弃。
fn generate(cache: &Path, src: &Path, size: u16) -> Option<PathBuf> {
    let permit = service().acquire();
    DECODE_COUNT.fetch_add(1, Ordering::SeqCst);

    let (tx, rx) = mpsc::channel();
    let src_display = src.display().to_string();
    let src = src.to_path_buf();
    let cache = cache.to_path_buf();
    // 解码线程持有许可：超时放弃后它自行收尾并释放许可（不占死名额）。
    // 61MP 解码 1-2s；超时=异常文件/挂死，绝不留半份缓存。
    let decoder = std::thread::Builder::new()
        .name("thumb-decode".into())
        .spawn(move || {
            let _permit = permit;
            let out = decode_and_encode(&src, size).and_then(|jpeg| {
                write_atomic(&cache, &jpeg).ok()?;
                Some(cache.clone())
            });
            let _ = tx.send(out.clone());
            out
        })
        .expect("spawn thumb decoder");
    drop(decoder); // 不 join：超时后任其收尾

    match rx.recv_timeout(DECODE_TIMEOUT) {
        Ok(result) => result,
        Err(_) => {
            eprintln!("缩略图解码超时（>{DECODE_TIMEOUT:?}），放弃: {src_display}");
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
    let thumb: image::RgbImage = if matches!(ext.as_str(), "jpg" | "jpeg") {
        jpeg_scaled(src, size).unwrap_or_else(|| full_decode_resize(src, size))
    } else {
        full_decode_resize(src, size)
    };
    let mut jpeg = Vec::new();
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 80);
    thumb.write_with_encoder(encoder).ok()?;
    Some(jpeg)
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
    let header = turbojpeg::read_header(&data).ok()?;
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
    decompressor.decompress(&data, image.as_deref_mut()).ok()?;
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
