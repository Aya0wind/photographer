//! 取景帧中继（2026-10-09 联机预览卡顿治理）：相机原始 JPEG → 按需
//! 降档重编码 → 二进制 IPC 载荷组装。
//!
//! 设计要点：
//! - **降档换流畅**（用户定案）：取景帧不需要成片画质，长边超过
//!   [`DOWNSCALE_LONG_EDGE`] 的帧用 turbojpeg DCT 域 4/8（减半）解码 +
//!   q75 重编码——USB 流大小相机说了算，降档只作用于 IPC 载荷体积与
//!   WebView 端解码成本；失败一律原样透传（取景优先不断流）。
//! - **二进制载荷**取代 base64 data URL JSON（每帧 +33% 体积且前端还要
//!   再解一次 data URL）：32 字节 AF 头（4×f64 LE，全 NaN = 本帧无 AF
//!   数据）+ JPEG 正文；空载荷 = 无帧（相机忙/收片中）。

/// 降档阈值：长边超过此值才重编码（小流原样透传——不少机身取景流本就
/// 640×426 级别，重编码是纯损耗）。
const DOWNSCALE_LONG_EDGE: usize = 1280;

/// 降档重编码质量（libjpeg-turbo q75：预览档常用值）。
const DOWNSCALE_QUALITY: i32 = 75;

/// AF 头固定长度（4×f64 LE）。
pub const AF_HEADER_BYTES: usize = 32;

/// 相机原始 JPEG → 按需降档。长边 ≤ 阈值或任何解码/编码失败时原样返回。
pub fn downscale_frame(jpeg: Vec<u8>) -> Vec<u8> {
    let Ok(header) = turbojpeg::read_header(&jpeg) else {
        return jpeg;
    };
    if header.width.max(header.height) <= DOWNSCALE_LONG_EDGE {
        return jpeg;
    }
    // libjpeg 缩放只支持 n/8 档：4/8（减半）正是「降一档」。
    let factor = turbojpeg::ScalingFactor::new(4, 8);
    let width = factor.scale(header.width);
    let height = factor.scale(header.height);
    let mut decompressor = match turbojpeg::Decompressor::new() {
        Ok(decompressor) => decompressor,
        Err(_) => return jpeg,
    };
    if decompressor.set_scaling_factor(factor).is_err() {
        return jpeg;
    }
    let mut pixels = vec![0u8; width * height * turbojpeg::PixelFormat::RGB.size()];
    {
        let mut image = turbojpeg::Image {
            pixels: pixels.as_mut_slice(),
            width,
            pitch: width * turbojpeg::PixelFormat::RGB.size(),
            height,
            format: turbojpeg::PixelFormat::RGB,
        };
        if decompressor.decompress(&jpeg, image.as_deref_mut()).is_err() {
            return jpeg;
        }
    }
    let mut encoder = match turbojpeg::Compressor::new() {
        Ok(encoder) => encoder,
        Err(_) => return jpeg,
    };
    if encoder.set_quality(DOWNSCALE_QUALITY).is_err() {
        return jpeg;
    }
    let image = turbojpeg::Image {
        pixels: pixels.as_slice(),
        width,
        pitch: width * turbojpeg::PixelFormat::RGB.size(),
        height,
        format: turbojpeg::PixelFormat::RGB,
    };
    encoder.compress_to_vec(image).unwrap_or(jpeg)
}

/// 组装二进制帧载荷：32B AF 头 + JPEG 正文（af=None 时头写全 NaN）。
pub fn frame_payload(af: Option<(f64, f64, f64, f64)>, jpeg: Vec<u8>) -> Vec<u8> {
    let (x, y, w, h) = af.unwrap_or((f64::NAN, f64::NAN, f64::NAN, f64::NAN));
    let mut out = Vec::with_capacity(AF_HEADER_BYTES + jpeg.len());
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.extend_from_slice(&jpeg);
    out
}

/// 解析 AF 头（[`frame_payload`] 的逆操作；任一分量 NaN → None）。
/// 生产解析在前端（JS DataView）；Rust 侧保留为线格式契约测试。
#[cfg(test)]
pub fn parse_af_header(payload: &[u8]) -> Option<(f64, f64, f64, f64)> {
    if payload.len() < AF_HEADER_BYTES {
        return None;
    }
    let value = |offset: usize| -> Option<f64> {
        payload
            .get(offset..offset + 8)
            .and_then(|bytes| bytes.try_into().ok())
            .map(f64::from_le_bytes)
    };
    let region = (value(0)?, value(8)?, value(16)?, value(24)?);
    if region.0.is_nan() || region.1.is_nan() || region.2.is_nan() || region.3.is_nan() {
        return None;
    }
    Some(region)
}

/// 慢帧探针（2026-10-09 索尼取景卡顿排查）：单帧相机读取 ≥200ms 记一条
/// 诊断（含后端与帧大小——USB 传输占比可判），2s 内至多一条防刷屏。
/// 频率上限保证探针本身不构成帧延迟（此前每帧双日志正是瓶颈之一）。
pub fn note_frame_latency(elapsed: std::time::Duration, backend: &str, jpeg_len: usize) {
    use std::sync::Mutex;
    if elapsed < SLOW_FRAME_THRESHOLD {
        return;
    }
    static LAST_NOTE: std::sync::OnceLock<Mutex<Option<std::time::Instant>>> =
        std::sync::OnceLock::new();
    let last_note = LAST_NOTE.get_or_init(|| Mutex::new(None));
    let mut last = last_note.lock().unwrap();
    if last.is_some_and(|at| at.elapsed() < SLOW_FRAME_RATE_LIMIT) {
        return;
    }
    *last = Some(std::time::Instant::now());
    drop(last);
    crate::devices::diagnostics::record(format!(
        "tethering frame slow backend={backend} elapsed={elapsed:?} jpeg={jpeg_len}"
    ));
}

/// 慢帧阈值：30fps 帧预算 33ms 的 6 倍——达到即用户可感。
const SLOW_FRAME_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(200);
/// 慢帧探针记录限频。
const SLOW_FRAME_RATE_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试 JPEG 夹具：纯色编码（指定尺寸）。
    fn solid_jpeg(width: usize, height: usize) -> Vec<u8> {
        let pixels = vec![128u8; width * height * 3];
        let image = turbojpeg::Image {
            pixels: pixels.as_slice(),
            width,
            pitch: width * 3,
            height,
            format: turbojpeg::PixelFormat::RGB,
        };
        let mut compressor = turbojpeg::Compressor::new().unwrap();
        compressor.compress_to_vec(image).unwrap()
    }

    #[test]
    fn small_frames_pass_through_untouched() {
        let jpeg = solid_jpeg(640, 426);
        assert_eq!(downscale_frame(jpeg.clone()), jpeg);
    }

    #[test]
    fn large_frames_transcode_to_half_size() {
        let source = solid_jpeg(2048, 1365);
        let transcoded = downscale_frame(source);
        let header = turbojpeg::read_header(&transcoded).unwrap();
        assert_eq!((header.width, header.height), (1024, 683));
    }

    #[test]
    fn invalid_jpeg_passes_through() {
        let junk = vec![1u8, 2, 3, 4];
        assert_eq!(downscale_frame(junk.clone()), junk);
    }

    #[test]
    fn payload_roundtrip_carries_af_and_jpeg() {
        let jpeg = solid_jpeg(32, 32);
        let payload = frame_payload(Some((0.25, 0.5, 0.75, 1.0)), jpeg.clone());
        assert_eq!(payload.len(), AF_HEADER_BYTES + jpeg.len());
        assert_eq!(parse_af_header(&payload), Some((0.25, 0.5, 0.75, 1.0)));
        assert_eq!(&payload[AF_HEADER_BYTES..], &jpeg[..]);
        // 无 AF：头为全 NaN，正文完整
        let bare = frame_payload(None, jpeg.clone());
        assert_eq!(parse_af_header(&bare), None);
        assert_eq!(&bare[AF_HEADER_BYTES..], &jpeg[..]);
        // 长度不足：无 AF
        assert_eq!(parse_af_header(&bare[..16]), None);
    }
}
