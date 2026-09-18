/**
 * Vitest 全局 setup（每个测试文件运行前执行）。
 * - 注册 jest-dom 断言扩展 + 每个用例后自动 cleanup
 * - 全局 mock Tauri API（单测环境无 Rust 后端），各测试可用 vi.mocked(...) 再覆写
 * - 补齐 jsdom 缺失的浏览器 API
 */
import "@testing-library/jest-dom/vitest";

import { afterEach, vi } from "vitest";
import { cleanup } from "@testing-library/react";

afterEach(cleanup);

// --- Tauri API 全局 mock ----------------------------------------------------

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn().mockResolvedValue(() => {}),
  emit: vi.fn().mockResolvedValue(undefined),
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
