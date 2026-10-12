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
    // 单测上限放宽（默认 5s）：waitFor 已统一 5s（见 setup.ts），含多个
    // 长等待的用例在 CI 慢机上会撞默认测试超时——只允许更久，不改变语义。
    testTimeout: 15000,
  },
  resolve: {
    alias: {
      // 与 vite.config.ts 保持一致
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },
});
