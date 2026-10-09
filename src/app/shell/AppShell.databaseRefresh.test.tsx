import { render, screen, waitFor } from "@testing-library/react";
import { act } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// 事件订阅 mock：捕获 handler 便于用例手动发 databasesChanged（真实实现
// 非常 Tauri 环境为 noop，无法驱动）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseList: vi.fn(async () => ({
      databases: [{ id: "db-1", name: "主数据库", dbDir: "D:\\db" }],
      activeId: "db-1",
    })),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import i18n from "@/i18n";
import AppShell from "./AppShell";
import { subscribeAppEvents, type AppEvent } from "@/ipc/api";
import { clone, DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";

const subscribeMock = vi.mocked(subscribeAppEvents);

/**
 * 数据库切换全量刷新（2026-10-09 多数据库修正）：databasesChanged 后
 * AppShell 重挂内容区（nonce 作 key）——页面级挂载 effect 重拉库内数据
 * （画廊/相册/照片库列表，换库语义）；侧栏当前库名随 useDatabases 重拉。
 */

type Handler = (event: AppEvent) => void;
let handlers: Handler[] = [];

function emit(event: AppEvent): void {
  act(() => {
    for (const handler of [...handlers]) handler(event);
  });
}

/** 内容区子页：挂载即拉数据（用挂载计数验证重挂） */
let mounts = 0;
function GalleryProbe() {
  mounts += 1;
  return <div data-testid="gallery-probe">GALLERY-{mounts}</div>;
}

function renderShell() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/" element={<AppShell />}>
            <Route path="gallery" element={<GalleryProbe />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  handlers = [];
  subscribeMock.mockReset().mockImplementation(async (handler) => {
    handlers.push(handler);
    return () => {
      handlers = handlers.filter((h) => h !== handler);
    };
  });
  mounts = 0;
  useSettingsStore.setState({ settings: clone(DEFAULT_SETTINGS), loaded: true });
});

describe("AppShell：databasesChanged 全量刷新", () => {
  it("切换/新建/移除数据库事件 → 内容区重挂载（页面挂载 effect 重拉）", async () => {
    renderShell();
    expect(screen.getByTestId("gallery-probe")).toHaveTextContent("GALLERY-1");
    expect(subscribeMock).toHaveBeenCalled();

    emit({ type: "databasesChanged" });
    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toHaveTextContent("GALLERY-2"));
    expect(mounts).toBe(2);

    emit({ type: "databasesChanged" });
    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toHaveTextContent("GALLERY-3"));
  });

  it("无关事件不重挂载（photoLibrariesChanged 等不触发换库重挂）", async () => {
    renderShell();
    emit({ type: "photoLibrariesChanged" });
    emit({ type: "importSessionFinished", jobId: 1, stats: {
      totalFiles: 0, doneFiles: 0, skippedDuplicates: 0, failedFiles: 0,
      totalBytes: 0, doneBytes: 0, elapsedMs: 0, bytesPerSec: 0,
    } });
    await new Promise((resolve) => setTimeout(resolve, 30));
    expect(mounts).toBe(1);
    expect(screen.getByTestId("gallery-probe")).toHaveTextContent("GALLERY-1");
  });

  it("侧栏底部只读展示当前数据库名（useDatabases）", async () => {
    renderShell();
    expect(await screen.findByTestId("sidebar-database-name")).toHaveTextContent("主数据库");
  });
});
