/**
 * HEVC/H.265 播放能力检测（WebView2 依赖系统 MediaFoundation HEVC 解码器，
 * 即「HEVC 视频扩展」；未安装时 canPlayType 对 hvc1 返回空串）。
 * 检测失败保守返回 false（宁可回退系统播放，不要黑屏）。
 */
export function supportsHevc(): boolean {
  try {
    const video = document.createElement("video");
    // hvc1 = HEVC in MP4 主流标签；带完整 codec string 提高判定可信度
    return video.canPlayType('video/mp4; codecs="hvc1.1.6.L93.B0"') !== "";
  } catch {
    return false;
  }
}
