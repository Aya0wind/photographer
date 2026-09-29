import { subscribeAppEvents } from "@/ipc/api";

import { loadSmartTags, reindexAllTags } from "./smartTags";

/**
 * 智能相册标签索引自动维护：订阅 `aiIndexFinished`（语义索引一轮回填
 * 收尾，本轮有新嵌入）→ 全量重建标签缓存（reindexAllTags——新照片可能
 * 给已索引标签新增命中，也顺带修复早于语义完成时缓存住的 0 命中）。
 * 应用启动订阅一次（main.tsx）；失败静默——模型未就绪/后端不可用时下一轮
 * 导入索引收尾会再触发。
 */

let eventsBound = false;
let running = false;

export async function initSmartTagAutoIndex(): Promise<void> {
  if (eventsBound) return;
  eventsBound = true;
  try {
    await subscribeAppEvents((event) => {
      if (event.type !== "aiIndexFinished" || event.done === 0) return;
      // 进行中不叠加（连发两轮收尾时第二轮的照片已含在重建查询里）
      if (running) return;
      running = true;
      void reindexAllTags(loadSmartTags())
        .catch(() => {})
        .finally(() => {
          running = false;
        });
    });
  } catch {
    // 非 Tauri 环境（vite dev 预览）静默
  }
}

/** 仅测试用：复位绑定与进行中标记。 */
export function resetSmartTagAutoIndexForTests(): void {
  eventsBound = false;
  running = false;
}
