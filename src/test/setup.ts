/**
 * Vitest 全局 setup（每个测试文件运行前执行）。
 * - 注册 jest-dom 断言扩展 + 每个用例后自动 cleanup
 * - 全局 mock Tauri API（单测环境无 Rust 后端），各测试可用 vi.mocked(...) 再覆写
 * - 补齐 jsdom 缺失的浏览器 API
 */
import "@testing-library/jest-dom/vitest";

import { afterEach, vi } from "vitest";
import { cleanup, configure } from "@testing-library/react";

afterEach(cleanup);

// waitFor/findBy 默认 1s 上限在 CI 慢机上不够（ViewerOverlay 侧栏退场动画、
// 预取层、画廊分组等时序断言在 CI 稳定超时，本地快所以一直绿——2026-10-12
// 双平台回归 CI 全挂在同一批用例）。统一放宽到 5s：语义不变（仍需最终
// 成立），只消除慢机竞态。
configure({ asyncUtilTimeout: 5000 });

// --- Tauri API 全局 mock ----------------------------------------------------

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  isTauri: vi.fn(() => false),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
}));

// 窗口控制（自绘标题栏）：三钮/最大化状态/resize 订阅。
// 各测试可用 vi.mocked(getCurrentWindow).mockReturnValue(...) 覆写为可控实例。
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
    isMaximized: vi.fn().mockResolvedValue(false),
    isFullscreen: vi.fn().mockResolvedValue(false),
    setFullscreen: vi.fn().mockResolvedValue(undefined),
    onResized: vi.fn().mockResolvedValue(() => {}),
  })),
}));

// --- jsdom 缺失的浏览器 API stub ---------------------------------------------

if (typeof window !== "undefined") {
  // matchMedia：motion / 响应式库依赖
  if (!window.matchMedia) {
    Object.defineProperty(window, "matchMedia", {
      writable: true,
      value: (query: string): MediaQueryList => ({
        matches: false,
        media: query,
        onchange: null,
        addListener: () => {},
        removeListener: () => {},
        addEventListener: () => {},
        removeEventListener: () => {},
        dispatchEvent: () => false,
      }),
    });
  }

  // ResizeObserver：虚拟滚动 / motion 依赖
  if (typeof window.ResizeObserver === "undefined") {
    class ResizeObserverStub implements ResizeObserver {
      observe(): void {}
      unobserve(): void {}
      disconnect(): void {}
    }
    Object.defineProperty(window, "ResizeObserver", {
      writable: true,
      value: ResizeObserverStub,
    });
  }
}
