import { listen } from "@tauri-apps/api/event";
import type { AppEvent } from "./types";

// --- 事件订阅 ----------------------------------------------------------------

/**
 * 订阅唯一事件通道 `app://event`。
 * @returns 取消订阅函数；非 Tauri 环境下（vite dev 预览）返回 noop。
 */
export async function subscribeAppEvents(
  handler: (event: AppEvent) => void,
): Promise<() => void> {
  try {
    return await listen<AppEvent>("app://event", (e) => handler(e.payload));
  } catch {
    return () => {};
  }
}
