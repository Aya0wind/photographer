import { defineConfig } from "vitest/config";
// @ts-expect-error type error without @types/node package
import { fileURLToPath, URL } from "node:url";

// 独立于 vite.config.ts 的测试配置（build 不受影响）。
// globals 关闭：测试内显式 import { describe, it, expect, vi } from "vitest"。
// https://vitest.dev/config/
export default defineConfig({
  test: {
    environment: "jsdom",
    globals: false,
    setupFiles: ["./src/test/setup.ts"],
  },
  resolve: {
    alias: {
      // 与 vite.config.ts 保持一致
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
});
