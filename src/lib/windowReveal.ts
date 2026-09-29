import { useEffect } from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow, Window } from "@tauri-apps/api/window";

/**
 * 主窗口显窗 + 关闭 splash（首启白屏消除，2026-09-29）：
 * tauri.conf 主窗口 visible=false，由首个有内容的页面在 ready=true 时调用
 * 本钩子显示自身并关闭 splash 窗口（按 label 查找，非 Tauri 环境静默跳过——
 * 浏览器 dev/测试不受影响）。幂等：进程内只执行一次。
 */
let revealed = false;

export function useWindowReveal(ready: boolean): void {
  useEffect(() => {
    if (!ready || revealed || !isTauri()) return;
    revealed = true;
    const current = getCurrentWindow();
    void current
      .show()
      .then(() => Window.getByLabel("splash"))
      .then((splash) => (splash === null ? undefined : splash.close()))
      .catch(() => {
        /* splash 已关/窗口竞争失败：主窗已显示，目标达成 */
      });
  }, [ready]);
}
