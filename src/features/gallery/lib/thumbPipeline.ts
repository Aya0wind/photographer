import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

import { assetThumbGet, subscribeAppEvents, type AppEvent } from "@/ipc/api";

/**
 * 库内资产缩略图管线（画廊网格/搜索结果/查看器胶片条共用，M3）：
 *
 * - 请求链：assetThumbGet(assetId, size) → 缓存文件绝对路径 → convertFileSrc；
 *   size 传名义边长（网格 240 / 查看器 1280），后端 snap 到 256/512 档就近返回。
 * - RAW（NEF/ARW 等）与视频 assetThumbGet 恒 null——调用方按 kind==="photo" 短路，
 *   非 photo 直接永久占位（不请求、不订阅重试，避免无效请求风暴）。
 * - 会话缓存 `${assetId}:${size}` → asset URL（null=未命中）；in-flight 去重；
 *   信号量并发 ≤6（与后端缩略图生成对齐，快速滚动时不打满 IPC）。
 * - 未命中 → 占位；`thumbnailReady` 事件到达（后台补齐缓存）后仅当前挂载的
 *   （=虚拟化后视口内的）tile 重试——订阅者天然被虚拟化限定在视口。
 */

const CONCURRENCY = 6;

/** 请求优先级：high=查看器主图/相邻预取（插队），low=网格/胶片条（默认） */
export type ThumbPriority = "high" | "low";

let activeLoads = 0;
const slotQueue: Array<() => void> = [];

function acquireSlot(priority: ThumbPriority): Promise<void> {
  if (activeLoads < CONCURRENCY) {
    activeLoads += 1;
    return Promise.resolve();
  }
  return new Promise((resolve) => {
    const grant = () => {
      activeLoads += 1;
      resolve();
    };
    // high 插到队首（主图不该排在几十个胶片条格子后面）
    if (priority === "high") slotQueue.unshift(grant);
    else slotQueue.push(grant);
  });
}

function releaseSlot(): void {
  activeLoads = Math.max(0, activeLoads - 1);
  const next = slotQueue.shift();
  if (next) next();
}

/**
 * 图片解码预热（查看器相邻预取）：prefetchAssetThumb 只预热 IPC 缓存路径，
 * 不预热解码器——补 new Image().src 让相邻图进入 Chromium 解码缓存，
 * 箭头切换时大概率瞬时显示。fire-and-forget，失败静默。
 */
export function warmImageDecode(url: string | null | undefined): void {
  if (!url) return;
  const img = new Image();
  img.src = url;
}

/** 会话缓存：`${assetId}:${size}` → asset URL（null=未命中，等待 thumbnailReady） */
const thumbCache = new Map<string, string | null>();
/** in-flight 去重：同 key 并发共享同一 Promise */
const thumbInflight = new Map<string, Promise<string | null>>();

/** 缩略图文件路径 → asset 协议 URL；非 Tauri 环境抛错/空结果回退 null（占位） */
function toAssetUrl(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** 取库内资产缩略图 URL（命中缓存直接返回；失败/未生成记 null 占位）。
 *  priority：high 插队信号量队列（查看器主图/预取），low 默认（网格/胶片条）。 */
export function fetchAssetThumb(
  assetId: number,
  size: number,
  priority: ThumbPriority = "low",
): Promise<string | null> {
  const key = `${assetId}:${size}`;
  const cached = thumbCache.get(key);
  if (cached !== undefined) return Promise.resolve(cached);
  const running = thumbInflight.get(key);
  if (running) return running;
  const promise = (async () => {
    await acquireSlot(priority);
    try {
      const path = await assetThumbGet(assetId, size);
      const url = path !== null ? toAssetUrl(path) : null;
      thumbCache.set(key, url);
      return url;
    } catch {
      thumbCache.set(key, null);
      return null;
    } finally {
      thumbInflight.delete(key);
      releaseSlot();
    }
  })();
  thumbInflight.set(key, promise);
  return promise;
}

/** 预取（查看器相邻预热/胶片条）：静默，失败仅保持占位；默认高优先级 */
export function prefetchAssetThumb(
  assetId: number,
  size: number,
  priority: ThumbPriority = "high",
): void {
  void fetchAssetThumb(assetId, size, priority);
}

// --- thumbnailReady 事件分发（单次订阅 + 按需注册监听） ----------------------------

type AssetEventListener = (event: AppEvent) => void;
const listeners = new Set<AssetEventListener>();
let subscribed = false;

function dispatchAssetEvent(event: AppEvent): void {
  if (event.type === "thumbnailReady") {
    // 该资产缩略图已补齐：丢弃全部档位的未命中记录，让视口内 tile 重试取到新结果
    // （档位对齐由后端 snap，事件里的 size 与名义请求尺寸可能不同，按资产整体失效最稳）
    for (const key of [...thumbCache.keys()]) {
      if (key.startsWith(`${event.assetId}:`)) thumbCache.delete(key);
    }
  }
  for (const listener of listeners) listener(event);
}

function ensureSubscribed(): void {
  if (subscribed) return;
  subscribed = true;
  void subscribeAppEvents((event) => dispatchAssetEvent(event));
}

/** 注册资产事件监听（组件卸载时调用返回的解绑函数） */
export function onAssetEvent(listener: AssetEventListener): () => void {
  ensureSubscribed();
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

// --- React 绑定 -------------------------------------------------------------------

export interface AssetThumbState {
  /** asset URL；null=未就绪（未命中或仍在途——以 settled 区分） */
  url: string | null;
  /** 本次请求是否已结算（true 且 url=null 表示确定无缩略图，不再等待） */
  settled: boolean;
}

/**
 * 单资产缩略图（enabled=false 时不请求不订阅——video 短路占位用）。
 * 未命中先渲染占位，thumbnailReady 事件到达后自动重试一次。
 * 注意：disabled 时 settled 保持 false（惰性启用档位刚翻转时，调用方的
 * 「结算即降级」判断不会被 disabled 期的陈旧 settled=true 误触发）。
 */
export function useAssetThumbUrl(
  assetId: number,
  size: number,
  enabled: boolean,
  priority: ThumbPriority = "low",
): AssetThumbState {
  const [state, setState] = useState<AssetThumbState>({ url: null, settled: false });

  useEffect(() => {
    if (!enabled) {
      setState({ url: null, settled: false });
      return;
    }
    let cancelled = false;
    setState({ url: null, settled: false });
    void fetchAssetThumb(assetId, size, priority).then((result) => {
      if (!cancelled) setState({ url: result, settled: true });
    });
    const off = onAssetEvent((event) => {
      if (event.type !== "thumbnailReady" || event.assetId !== assetId) return;
      void fetchAssetThumb(assetId, size, priority).then((result) => {
        if (!cancelled) setState({ url: result, settled: true });
      });
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [assetId, size, enabled, priority]);

  return state;
}

// --- 测试辅助 ---------------------------------------------------------------------

/** 仅测试用：直接注入事件（绕过 Tauri 事件桥），驱动 thumbnailReady 重试路径 */
export function emitAssetEventForTests(event: AppEvent): void {
  dispatchAssetEvent(event);
}

/** 仅测试用：清空管线状态（缓存/in-flight/信号量） */
export function resetThumbPipelineForTests(): void {
  thumbCache.clear();
  thumbInflight.clear();
  activeLoads = 0;
  slotQueue.length = 0;
}
