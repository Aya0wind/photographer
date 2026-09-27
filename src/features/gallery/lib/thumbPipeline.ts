import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";

import {
  assetThumbGet,
  subscribeAppEvents,
  type AppEvent,
  type ThumbGetResult,
} from "@/ipc/api";

/**
 * 库内资产缩略图管线（画廊网格/搜索结果/查看器胶片条共用，M3）：
 *
 * - 请求链：assetThumbGet(assetId, size) 三态契约——ready=缓存命中直返路径；
 *   pending=已入队后台生成（thumbnailReady 事件到达后重试）；unavailable=
 *   永久不可用（此前 null 一刀切，前端把「排队中」误判成失败而过早降级，
 *   真机 RAW 查看器 219ms 即回落显影兜底的根因，2026-09-20 契约改造）。
 * - 会话缓存 `${assetId}:${size}` → asset URL；in-flight 去重；
 *   信号量并发 ≤6（与后端缩略图生成对齐，快速滚动时不打满 IPC）。
 * - pending → 骨架等待事件重试 + 周期兜底重拉（事件丢失自愈，见 PENDING_RETRY_MS）；
 *   unavailable → 调用方按「确定无图」占位。
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

/** 会话缓存：`${assetId}:${size}` → asset URL（仅缓存成功结果；pending/
 *  unavailable 不落缓存——事件重试与占位由 hook 状态机管理） */
const thumbCache = new Map<string, string>();
/** in-flight 去重：同 key 并发共享同一 Promise */
const thumbInflight = new Map<string, Promise<ThumbResult>>();

/** 缩略图文件路径 → asset 协议 URL；非 Tauri 环境抛错回退 unavailable */
function toAssetUrl(path: string): ThumbGetResult {
  try {
    const url = convertFileSrc(path);
    return url ? { status: "ready", path: url } : { status: "unavailable" };
  } catch {
    return { status: "unavailable" };
  }
}

/** 管线结果：ready 附 URL；pending=排队生成中；failed=永久不可用 */
export type ThumbResult =
  | { kind: "url"; url: string }
  | { kind: "pending" }
  | { kind: "failed" };

/** 取库内资产缩略图（命中会话缓存直接返回；priority：high 插队信号量队列，
 *  low 默认）。pending 不写缓存（事件重试自愈）；failed 缓存避免风暴。 */
export function fetchAssetThumb(
  assetId: number,
  size: number,
  priority: ThumbPriority = "low",
): Promise<ThumbResult> {
  const key = `${assetId}:${size}`;
  const cached = thumbCache.get(key);
  if (cached !== undefined) return Promise.resolve({ kind: "url", url: cached });
  const failed = failedCache.get(key);
  if (failed) return Promise.resolve({ kind: "failed" });
  const running = thumbInflight.get(key);
  if (running) return running;
  const promise = (async () => {
    await acquireSlot(priority);
    try {
      const result = await assetThumbGet(assetId, size);
      if (result.status === "ready") {
        const converted = toAssetUrl(result.path);
        if (converted.status === "ready") {
          thumbCache.set(key, converted.path);
          return { kind: "url", url: converted.path } as ThumbResult;
        }
        return { kind: "failed" } as ThumbResult;
      }
      if (result.status === "unavailable") {
        failedCache.set(key, true);
        return { kind: "failed" } as ThumbResult;
      }
      return { kind: "pending" } as ThumbResult; // 排队中：等事件重试
    } catch {
      return { kind: "failed" } as ThumbResult;
    } finally {
      thumbInflight.delete(key);
      releaseSlot();
    }
  })();
  thumbInflight.set(key, promise);
  return promise;
}

/** 永久失败缓存（连续失败/不可解码）；thumbnailReady 事件按资产整体失效 */
const failedCache = new Map<string, boolean>();

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
    // 该资产缩略图已补齐：丢弃全部档位的缓存记录（成功+失败），让视口内
    // tile 重试取到新结果（档位对齐由后端 snap，按资产整体失效最稳）
    for (const key of [...thumbCache.keys()]) {
      if (key.startsWith(`${event.assetId}:`)) thumbCache.delete(key);
    }
    for (const key of [...failedCache.keys()]) {
      if (key.startsWith(`${event.assetId}:`)) failedCache.delete(key);
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
  /** asset URL；null=尚未就绪（loading/pending 皆可能） */
  url: string | null;
  /** loading=请求在途或排队生成中（骨架等待）；ready=有图；failed=永久无图 */
  status: "loading" | "ready" | "failed";
}

/**
 * pending 兜底重拉间隔。thumbnailReady 事件可能丢失且无任何回执：
 * - 后端生成队列满时直接丢任务**不发事件**（thumb.rs 注释明言依赖
 *   「前端滚动重试自愈」——查看器大图不滚动，必须自己兜底）；
 * - 事件可能先于命令响应到达：重查被 in-flight 去重吞并，之后无人再问；
 * - 事件可能早于本 hook 订阅（Tauri listen 异步注册窗口）发出。
 * pending 态周期重拉直至 settled（ready/failed），是上述全部场景的统一自愈。
 */
export const PENDING_RETRY_MS = 2500;

/**
 * 单资产缩略图（enabled=false 时不请求不订阅——video 短路占位用）。
 * pending → 保持 loading 等 thumbnailReady 事件重试 + 周期兜底重拉
 * （PENDING_RETRY_MS，事件丢失时自愈）；unavailable → failed。
 */
export function useAssetThumbUrl(
  assetId: number,
  size: number,
  enabled: boolean,
  priority: ThumbPriority = "low",
): AssetThumbState {
  const [state, setState] = useState<AssetThumbState>({ url: null, status: "loading" });

  useEffect(() => {
    if (!enabled) {
      setState({ url: null, status: "loading" });
      return;
    }
    let cancelled = false;
    let retryTimer: number | null = null;
    setState({ url: null, status: "loading" });
    const apply = (result: ThumbResult) => {
      if (cancelled) return;
      if (result.kind === "url") setState({ url: result.url, status: "ready" });
      else if (result.kind === "failed") setState({ url: null, status: "failed" });
      // pending：保持 loading——事件重试之外再排一次周期兜底重拉
      else schedulePendingRetry();
    };
    const retry = () => {
      void fetchAssetThumb(assetId, size, priority).then(apply);
    };
    const schedulePendingRetry = () => {
      if (retryTimer !== null) return;
      retryTimer = window.setTimeout(() => {
        retryTimer = null;
        retry();
      }, PENDING_RETRY_MS);
    };
    retry();
    const off = onAssetEvent((event) => {
      if (event.type !== "thumbnailReady" || event.assetId !== assetId) return;
      // 同资产任一档位就绪：立即重查（多数情况此处即命中缓存）；
      // 若仍 pending（如事件属于另一档位/被在途去重吞并），周期兜底接管。
      if (retryTimer !== null) {
        clearTimeout(retryTimer);
        retryTimer = null;
      }
      retry();
    });
    return () => {
      cancelled = true;
      if (retryTimer !== null) clearTimeout(retryTimer);
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

/** 清空管线状态（缓存/in-flight/信号量）。切库必须调用：缓存键是
 *  assetId+size，不同库的 assetId 会撞号，跨库沿用=张冠李戴的缩略图。 */
export function resetThumbPipeline(): void {
  thumbCache.clear();
  failedCache.clear();
  thumbInflight.clear();
  activeLoads = 0;
  slotQueue.length = 0;
}

/** 仅测试用 */
export function resetThumbPipelineForTests(): void {
  resetThumbPipeline();
}
