import { convertFileSrc } from "@tauri-apps/api/core";
import { thumbGet } from "@/ipc/api";

/** 缩略图请求边长（px）：后端缓存档位就近 */
export const THUMB_SIZE = 256;

/** thumb_get 在途并发上限：后端已提速（turbojpeg 缩放解码+动态并发），前端拉高铺屏速度 */
const IMAGE_LOAD_CONCURRENCY = 8;
let activeImageLoads = 0;
const imageSlotQueue: Array<() => void> = [];

function acquireImageSlot(): Promise<void> {
  if (activeImageLoads < IMAGE_LOAD_CONCURRENCY) {
    activeImageLoads += 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    imageSlotQueue.push(() => {
      activeImageLoads += 1;
      resolve();
    });
  });
}

function releaseImageSlot(): void {
  activeImageLoads = Math.max(0, activeImageLoads - 1);
  const next = imageSlotQueue.shift();
  if (next) next();
}

/** 有界会话缓存：来源、版本 → asset URL；失败不缓存。 */
export const thumbUrlCache = new Map<string, string>();
/** in-flight 去重：同 absPath 的并发请求共享同一 Promise（滚动复用不重复 IPC） */
const thumbInflight = new Map<string, { promise: Promise<string | null>; consumers: Array<() => boolean> }>();

/** 缩略图文件路径 → asset 协议 URL；非 Tauri 环境抛错/空结果回退 null（占位） */
function thumbAssetUrl(thumbPath: string): string | null {
  try {
    return convertFileSrc(thumbPath) || null;
  } catch {
    return null;
  }
}

/**
 * 取某源文件的缩略图 asset URL（M2 起 img 一律读后端小图，不再解码原图）：
 * thumb_get(absPath, 256) → 缓存文件路径 → convertFileSrc；null → 调用方保持占位。
 * 命中缓存直接返回；同 key 并发共享请求；失败不缓存以便重试。
 */
export function cachedThumbUrl(key: string, request: () => Promise<string | null>, isWanted: () => boolean = () => true): Promise<string | null> {
  const cached = thumbUrlCache.get(key);
  if (cached !== undefined) return Promise.resolve(cached);
  const inflight = thumbInflight.get(key);
  if (inflight) { inflight.consumers.push(isWanted); return inflight.promise; }
  const consumers = [isWanted];
  const promise = (async () => {
    await acquireImageSlot();
    try {
      // 快速滚动离开的图片，尚未开始的请求直接跳过。
      if (!consumers.some((wanted) => wanted())) return null;
      const thumbPath = await request();
      const url = thumbPath !== null ? thumbAssetUrl(thumbPath) : null;
      // 临时失败不永久缓存；设备重连或手动重试后仍可以正常取图。
      if (url) {
        if (thumbUrlCache.size >= 512) thumbUrlCache.delete(thumbUrlCache.keys().next().value!);
        thumbUrlCache.set(key, url);
      }
      return url;
    } catch {
      return null;
    } finally {
      releaseImageSlot();
      thumbInflight.delete(key);
    }
  })();
  thumbInflight.set(key, { promise, consumers });
  return promise;
}

export function fetchThumbUrl(absPath: string, version = "", isWanted?: () => boolean): Promise<string | null> {
  return cachedThumbUrl(JSON.stringify([absPath, version]), () => thumbGet(absPath, THUMB_SIZE), isWanted);
}

/** 仅测试用：清空缩略图会话缓存 */
export function resetThumbCacheForTests(): void {
  thumbUrlCache.clear();
  thumbInflight.clear();
}
