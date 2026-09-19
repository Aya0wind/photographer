import { beforeAll, beforeEach, afterEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import SearchPage from "./SearchPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { assetThumbGet, assetsPage, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const assetsPageMock = vi.mocked(assetsPage);
const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(id: number, date: string | null, kind: AssetDto["kind"] = "photo"): AssetDto {
  const ext = kind === "raw" ? "CR3" : kind === "video" ? "MP4" : "JPG";
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\2026\\IMG_${String(id).padStart(4, "0")}.${ext}`,
    name: `IMG_${String(id).padStart(4, "0")}.${ext}`,
    kind,
    capturedAt: date === null ? null : `${date}T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
  };
}

function renderSearch() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/search"]}>
        <Routes>
          <Route path="/search" element={<SearchPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// IntersectionObserver stub（jsdom 缺失；本套件不触发哨兵，仅防挂载报错）
beforeAll(() => {
  (window as unknown as Record<string, unknown>).IntersectionObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  vi.useRealTimers();
  assetsPageMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue(null);
  convertMock.mockReset().mockReturnValue("");
  resetThumbPipelineForTests();
});

afterEach(() => {
  vi.useRealTimers();
});

// --- 条件负载组装 -------------------------------------------------------------------

describe("搜索：filters 负载组装（camelCase 平铺）", () => {
  it("初始无条件查询：assetsPage(0, 100, {})", async () => {
    renderSearch();

    await screen.findByTestId("search-page");
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100, {}));
  });

  it("类型+日期范围+相机组合成完整 filters（防抖后单次查询）", async () => {
    vi.useFakeTimers();
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-01-15", "raw")]);
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    // 组合条件：RAW + 日期范围 + 相机
    fireEvent.click(screen.getByTestId("search-kind-raw"));
    fireEvent.change(screen.getByTestId("search-from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByTestId("search-to"), { target: { value: "2026-02-01" } });
    fireEvent.change(screen.getByTestId("search-camera"), { target: { value: "Canon" } });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });

    // fake timers 下 waitFor 不推进：advanceTimersByTimeAsync 已同刷微任务，直接断言
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      kind: "raw",
      capturedAfter: "2026-01-01",
      capturedBefore: "2026-02-01",
      camera: "Canon",
    });
    // 防抖后只有两查询：初始 + 组合条件（中间态不发起）
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
  });

  it("相机输入去首尾空白；空条件不产生 filters 字段", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    fireEvent.change(screen.getByTestId("search-camera"), { target: { value: "  Canon  " } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon" });

    // 清空 → 回到无条件
    fireEvent.change(screen.getByTestId("search-camera"), { target: { value: "" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {});
  });
});

// --- 防抖 -------------------------------------------------------------------------

describe("搜索：300ms 防抖", () => {
  it("连续输入只查一次（299ms 内不查，300ms 后以最终值查）", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    // 快速连打 5 个字符
    const input = screen.getByTestId("search-camera");
    fireEvent.change(input, { target: { value: "C" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.change(input, { target: { value: "Ca" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.change(input, { target: { value: "Can" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.change(input, { target: { value: "Cano" } });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.change(input, { target: { value: "Canon" } });
    // 最后一击后 299ms：仍未查询
    await act(async () => {
      await vi.advanceTimersByTimeAsync(299);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon" });
    // 无任何中间态查询（"C"/"Ca"/…）
    const cameraCalls = assetsPageMock.mock.calls.filter((call) => (call[2] as { camera?: string }).camera);
    expect(cameraCalls).toHaveLength(1);
  });
});

// --- 结果渲染 ---------------------------------------------------------------------

describe("搜索：结果与状态", () => {
  it("结果复用画廊网格（同一虚拟化+缩略图管线）：分组/瓦片/计数徽标", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(1, "2026-09-18"),
      makeAsset(2, "2026-09-18"),
      makeAsset(3, "2026-09-17"),
    ]);
    renderSearch();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);
    expect(screen.getByTestId("search-count")).toHaveTextContent("3 个结果");
    // 复用 AssetGrid 的组头与滚动容器
    expect(screen.getAllByTestId("gallery-group")).toHaveLength(2);
    expect(screen.getByTestId("search-grid-scroll")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-chips")).not.toBeInTheDocument();
  });

  it("点击结果瓦片打开查看器（?asset=）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderSearch();

    fireEvent.click((await screen.findAllByTestId("gallery-tile"))[0]);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
  });

  it("空态：无匹配文案", async () => {
    assetsPageMock.mockResolvedValue([]);
    renderSearch();

    expect(await screen.findByTestId("search-empty")).toHaveTextContent("没有匹配的照片");
    expect(screen.getByTestId("search-count")).toHaveTextContent("0 个结果");
  });
});
