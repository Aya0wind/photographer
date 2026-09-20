import { useEffect } from "react";

/**
 * 屏蔽 WebView 原生行为（桌面应用的浏览器残留）：
 * - 全局 contextmenu preventDefault——右键一律走应用自定义菜单（瓦片/大图），
 *   未挂自定义菜单的区域右键无动作（浏览器菜单不出）。
 * - 开发者工具快捷键：F12 / Ctrl+Shift+I（DevTools）/ Ctrl+Shift+J（控制台）/
 *   Ctrl+Shift+C（检查元素）preventDefault。
 * 只精确匹配上述组合，不碰业务快捷键（?/Esc/方向键/Ctrl+1..5 等）。
 * 监听挂 document（捕获期）+ window——jsdom 测试环境同样可驱动断言。
 */

/** 精确匹配的开发者工具快捷键（keydown 的 event.key，区分大小写） */
const DEVTOOL_KEYS = new Set(["I", "J", "C"]);

export function useNativeBehaviorGuard(): void {
  useEffect(() => {
    function onContextMenu(e: MouseEvent): void {
      e.preventDefault();
    }
    function onKeyDown(e: KeyboardEvent): void {
      if (e.key === "F12") {
        e.preventDefault();
        return;
      }
      // Ctrl+Shift+I/J/C（metaKey=macOS ⌘）；再按 Alt/其它修饰不匹配
      if ((e.ctrlKey || e.metaKey) && e.shiftKey && !e.altKey && DEVTOOL_KEYS.has(e.key)) {
        e.preventDefault();
      }
    }
    document.addEventListener("contextmenu", onContextMenu);
    window.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("contextmenu", onContextMenu);
      window.removeEventListener("keydown", onKeyDown, true);
    };
  }, []);
}
