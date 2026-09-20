//! 视频海报提取（M8：ffmpeg 侧车）。
//!
//! 视频资产此前在缩略图管线无路径（查看器恒占位）。本模块用打包进安装
//! 包的 ffmpeg 侧车（tauri.conf.json `bundle.externalBin`，运行时落位在
//! 主程序旁的 `ffmpeg.exe`）抽 1s 处一帧做海报：
//!
//! - 命令：`ffmpeg -hide_banner -loglevel error -ss 1 -i <src> -frames:v 1
//!   -vf scale={W}:-2 -q:v 4 <out>.jpg`（`-ss` 前置 = 输入端快进，毫秒级
//!   定位；`-2` 保证高度为偶数；q4≈视觉无损）。
//! - 缓存：沿用 `dbDir/thumbs/` 根（M8 批次 1 的 LRU 上限自动覆盖海报），
//!   档位目录 `video-{size}-v1`（代际前缀防与图片档撞名；升代际即失效
//!   重建）。键 = `xxh64(路径小写)-<mtime secs>`，与 thumbs 同构。
//! - 降级契约：侧车缺失 / ffmpeg 失败 / 视频短于 1s → `None`（调用方走
//!   既有失败计数 ≥3 Unavailable 契约）；本模块零 panic、零 crate 依赖
//!   （独立可测，测试二进制直连不牵连其他模块）。
//! - 进程治理：`try_wait` 轮询 + 30s 上限，超时 kill 收尸，不留孤儿进程；
//!   全部调用发生在索引 worker / 按需队列 / run_blocking 线程——绝不碰
//!   UI 线程。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use xxhash_rust::xxh64::Xxh64;

/// 走海报路径的视频扩展名（小写）。导入侧的资产分类见
/// `devices::VIDEO_EXTS`；这里按任务规格取超集（m4v/webm/mpg/mpeg 等
/// 即使未分类入册，管线也不误走位图解码），并补齐 devices 的 avchd。
pub const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "3gp", "avi", "mkv", "webm", "mts", "m2ts", "mpg", "mpeg", "wmv", "avchd",
];

/// 扩展名是否走视频海报路径。
pub fn is_video_ext(ext: &str) -> bool {
    VIDEO_EXTS.contains(&ext.to_ascii_lowercase().as_str())
}

/// ffmpeg 单次运行上限（4K 长视频输入端快进 + 单帧解码 ≤ 数秒；超时视为
/// 挂死，kill 后走失败计数）。
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(30);

/// 海报代际（命令行/滤镜变更时 +1：旧缓存档位自动失效重建）。
const POSTER_GENERATION: u32 = 1;

/// 解析侧车路径：安装包落位（exe 旁裸名）→ target/<profile>/（build.rs
/// dev 拷贝，裸名 + 三元组名都试）→ 源码仓 binaries/（dev 兜底）。进程级
/// 缓存（OnceLock：文件系统状态视为稳定）。
pub fn ffmpeg_path() -> Option<PathBuf> {
    static CACHE: OnceLock<Option<PathBuf>> = OnceLock::new();
    CACHE.get_or_init(locate_ffmpeg).clone()
}

fn locate_ffmpeg() -> Option<PathBuf> {
    // 编译期嵌入的源码仓路径：仅 dev 机上有意义（安装机 is_file 为假，
    // 无副作用）。
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            // 安装包 / tauri dev：externalBin 落位名（裸名）
            candidates.push(dir.join("ffmpeg.exe"));
            // 三元组名（tauri CLI dev 拷贝形态）
            candidates.push(dir.join("ffmpeg-x86_64-pc-windows-msvc.exe"));
        }
    }
    // build.rs 已把侧车拷到 target/<profile>/ffmpeg.exe（同 current_exe
    // 目录）；源码仓 binaries/ 是未跑 build.rs 场景的兜底。
    candidates.push(manifest.join("binaries").join("ffmpeg.exe"));
    candidates.push(
        manifest
            .join("binaries")
            .join("ffmpeg-x86_64-pc-windows-msvc.exe"),
    );
    candidates.into_iter().find(|p| p.is_file())
}

/// 失败收尾：尽力移除本次可能新建的空档目录。严格契约是失败不落任何
/// 缓存痕迹——空目录也算（thumb_test raw_and_video_return_none_without_cache）；
/// remove_dir 只删空目录，并发成功写入时天然无害。
fn cleanup_empty_tier(parent: &Path) {
    if std::fs::remove_dir(parent).is_ok() {
        if let Some(grand) = parent.parent() {
            let _ = std::fs::remove_dir(grand);
        }
    }
}

/// 取（或生成）视频海报缓存。无法生成（侧车缺失/ffmpeg 失败/源不可读）
/// 返回 None——调用方走既有失败计数契约。
pub fn video_poster(db_dir: &Path, src: &Path, size: u16) -> Option<PathBuf> {
    let meta = std::fs::metadata(src).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let cache = poster_path(db_dir, src, size, mtime);
    if cache.is_file() {
        return Some(cache); // 命中，不重跑 ffmpeg
    }
    let exe = ffmpeg_path()?;
    let parent = cache.parent()?;
    std::fs::create_dir_all(parent).ok()?;
    // tmp + rename 原子落位（半文件绝不污染缓存键）。tmp 必须保持 .jpg
    // 结尾——ffmpeg 按输出扩展名推 muxer（.part 会报「无合适输出格式」）
    let name = cache
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("poster")
        .trim_end_matches(".jpg");
    let tmp = cache.with_file_name(format!("{name}.part.jpg"));
    let _ = std::fs::remove_file(&tmp);
    if !extract_with(&exe, src, u32::from(size), &tmp) {
        let _ = std::fs::remove_file(&tmp);
        cleanup_empty_tier(parent);
        return None;
    }
    if std::fs::rename(&tmp, &cache).is_err() {
        let _ = std::fs::remove_file(&tmp);
        cleanup_empty_tier(parent);
        return None;
    }
    Some(cache)
}

/// 缓存命中探测（不生成、不 spawn）：thumb 管线快路径。
pub fn cached_poster(db_dir: &Path, src: &Path, size: u16) -> Option<String> {
    let meta = std::fs::metadata(src).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime = meta.modified().ok()?;
    let cache = poster_path(db_dir, src, size, mtime);
    cache
        .is_file()
        .then(|| cache.to_string_lossy().into_owned())
}

/// 海报缓存路径：`dbDir/thumbs/video-{size}-v{gen}/<xxh64(路径小写)>-<mtime>.jpg`。
fn poster_path(db_dir: &Path, src: &Path, size: u16, mtime: SystemTime) -> PathBuf {
    let mut xxh = Xxh64::new(0);
    xxh.update(src.to_string_lossy().to_lowercase().as_bytes());
    let secs = mtime
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    db_dir
        .join("thumbs")
        .join(format!("video-{size}-v{POSTER_GENERATION}"))
        .join(format!("{:016x}-{secs}.jpg", xxh.digest()))
}

/// 跑一次抽帧。成功 = 退出码 0 且输出文件存在。可注入 exe（测试侧车
/// 缺失/坏 exe 降级）。
fn extract_with(exe: &Path, src: &Path, width: u32, out: &Path) -> bool {
    let Ok(mut child) = Command::new(exe)
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-ss")
        .arg("1") // 输入端快进到 1s（首帧常为黑场/淡入）
        .arg("-i")
        .arg(src)
        .arg("-frames:v")
        .arg("1")
        .arg("-vf")
        .arg(
            format!("scale={width}:-2") // 高度取偶（yuv420p 对齐）
                .as_str(),
        )
        .arg("-q:v")
        .arg("4")
        .arg("-y")
        .arg(out)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
    else {
        return false; // 侧车缺失/不可执行：优雅降级
    };
    // try_wait 轮询：超时可直接 kill（child 未被 move，无孤儿进程）
    let deadline = Instant::now() + FFMPEG_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success() && out.is_file(),
            Ok(None) => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(_) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_ext_routing() {
        for ext in ["mp4", "MP4", "Mov", "m4v", "webm", "mts", "MPG", "wmv"] {
            assert!(is_video_ext(ext), "{ext} 应走海报路径");
        }
        for ext in ["jpg", "cr3", "nef", "txt", "dng", "png"] {
            assert!(!is_video_ext(ext), "{ext} 不应走海报路径");
        }
    }

    #[test]
    fn missing_or_broken_exe_degrades_to_none() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.mp4");
        std::fs::write(&src, b"not-a-real-video").unwrap();
        let out = dir.path().join("poster.jpg");
        // 不存在的 exe：spawn 失败 → false，不 panic
        assert!(!extract_with(
            Path::new(r"Z:\definitely\not\ffmpeg.exe"),
            &src,
            256,
            &out
        ));
        assert!(!out.exists(), "失败不得留半产物");
        // 存在但不是可执行文件（文本）：Windows CreateProcess 失败 → false
        let fake = dir.path().join("fake-ffmpeg.exe");
        std::fs::write(&fake, b"MZ-not-really").unwrap();
        assert!(!extract_with(&fake, &src, 256, &out));
        assert!(!out.exists());
    }

    #[test]
    fn poster_path_shape_and_keying() {
        let dir = tempfile::tempdir().unwrap();
        let src = Path::new(r"X:\media\C0121.MP4");
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let p = poster_path(dir.path(), src, 256, t);
        assert!(p.starts_with(dir.path().join("thumbs").join("video-256-v1")));
        assert!(p
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-1700000000.jpg"));
        // 路径大小写不敏感：大小写不同的同文件同键
        let src2 = Path::new(r"X:\media\c0121.mp4");
        assert_eq!(poster_path(dir.path(), src2, 256, t), p);
        // mtime 变化 → 键变（源更新自然失效）
        let t2 = t + Duration::from_secs(1);
        assert_ne!(poster_path(dir.path(), src, 256, t2), p);
        // 侧车缺失环境：video_poster 优雅 None（真实侧车存在时另测）
        let real = dir.path().join("real.mp4");
        std::fs::write(&real, b"x").unwrap();
        if ffmpeg_path().is_none() {
            assert!(video_poster(dir.path(), &real, 256).is_none());
        }
    }
}
