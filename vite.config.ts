import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error type error without @types/node package
import { fileURLToPath, URL } from "node:url";
// @ts-expect-error type error without @types/node package
import process from "node:process";
import { createRequire } from "node:module";
import { readFileSync } from "node:fs";
const host = process.env.TAURI_DEV_HOST;
const nodeRequire = createRequire(import.meta.url);

/**
 * maplibre v6 worker 伴生资产（2026-09-30 release 黑图真因）：
 * worker 以 `?url` 引入时 vite 只原样拷贝 worker 文件本身，其
 * `import "./maplibre-gl-shared.mjs"`（~516KB 共享块）不进产物——release
 * 里该相对路径命中 SPA 的 index.html 回退，worker 模块解析失败静默死亡，
 * 地图无瓦片永黑（dev 正常：vite dev server 从 node_modules 服务真身）。
 * 构建时从 node_modules 现解析并 emit 到与 worker 同目录，相对 import 即命中。
 */
function maplibreWorkerSharedAsset() {
  return {
    name: "maplibre-worker-shared-asset",
    apply: "build" as const,
    generateBundle(this: { emitFile: (o: { type: string; fileName: string; source: Buffer }) => void }) {
      const source = readFileSync(
        nodeRequire.resolve("maplibre-gl/dist/maplibre-gl-shared.mjs"),
      );
      this.emitFile({
        type: "asset",
        fileName: "assets/maplibre-gl-shared.mjs",
        source,
      });
    },
  };
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [react(), tailwindcss(), maplibreWorkerSharedAsset()],

  resolve: {
    alias: {
      "@": fileURLToPath(new URL("./src", import.meta.url)),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 2420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
