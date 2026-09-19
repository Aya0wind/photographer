import { beforeAll, beforeEach, afterEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import SearchPage, { quickRange } from "./SearchPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { assetThumbGet, assetsPage, cameraList, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetThumbGet: vi.fn(),
    cameraList: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const assetsPageMock = vi.mocked(assetsPage);
const thumbMock = vi.mocked(assetThumbGet);
const cameraListMock = vi.mocked(cameraList);
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
  cameraListMock.mockReset().mockResolvedValue([]);
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

  it("类型+日期范围+相机勾选组合成完整 filters（防抖后单次查询）", async () => {
    vi.useFakeTimers();
    cameraListMock.mockResolvedValue([
      { camera: "Canon EOS R5", count: 12 },
      { camera: "Apple iPhone 15", count: 3 },
    ]);
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-01-15", "raw")]);
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    // 组合条件：RAW + 日期范围 + 相机勾选
    fireEvent.click(screen.getByTestId("search-kind-raw"));
    fireEvent.change(screen.getByTestId("search-from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByTestId("search-to"), { target: { value: "2026-02-01" } });
    fireEvent.click(screen.getByTestId("search-camera-button"));
    const menu = screen.getByTestId("search-camera-menu");
    const options = within(menu).getAllByTestId("search-camera-option");
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveAttribute("data-camera", "Canon EOS R5");
    expect(options[0]).toHaveTextContent("12");
    fireEvent.click(within(options[0]).getByRole("checkbox"));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });

    // fake timers 下 waitFor 不推进：advanceTimersByTimeAsync 已同刷微任务，直接断言
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      kind: "raw",
      capturedAfter: "2026-01-01",
      capturedBefore: "2026-02-01",
      camera: "Canon EOS R5",
    });
    // 防抖后只有两查询：初始 + 组合条件（中间态不发起）
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
  });

  it("日期快捷段：近7天/去年 → 负载端点（本地时区 YYYY-MM-DD）", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    fireEvent.click(screen.getByTestId("search-quick-recent7"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    const [r7From, r7To] = quickRange("recent7");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: r7From,
      capturedBefore: r7To,
    });

    fireEvent.click(screen.getByTestId("search-quick-lastYear"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    const [lyFrom, lyTo] = quickRange("lastYear");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: lyFrom,
      capturedBefore: lyTo,
    });
  });
});

// --- 防抖 -------------------------------------------------------------------------

describe("搜索：300ms 防抖", () => {
  it("连续变更只查一次（299ms 内不查，300ms 后以最终值查）", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    // 快速连点快捷段（条件连续变更）
    fireEvent.click(screen.getByTestId("search-quick-recent7"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.click(screen.getByTestId("search-quick-recent30"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.click(screen.getByTestId("search-quick-thisYear"));
    // 最后一击后 299ms：仍未查询
    await act(async () => {
      await vi.advanceTimersByTimeAsync(299);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
    const [thisFrom, thisTo] = quickRange("thisYear");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: thisFrom,
      capturedBefore: thisTo,
    });
    // 无任何中间态查询
    const withDate = assetsPageMock.mock.calls.filter(
      (call) => (call[2] as { capturedAfter?: string }).capturedAfter,
    );
    expect(withDate).toHaveLength(1);
  });
});

// --- 相机勾选下拉 -------------------------------------------------------------------

describe("搜索：相机勾选（cameraList 清单）", () => {
  it("「全部相机」默认态；列表渲染计数徽标；勾选过滤、多选先传第一个、清空回无条件", async () => {
    cameraListMock.mockResolvedValue([
      { camera: "Canon EOS R5", count: 12 },
      { camera: "Apple iPhone 15", count: 3 },
    ]);
    renderSearch();
    await screen.findByTestId("search-page");
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100, {}));

    // 默认按钮=全部相机；打开下拉
    expect(screen.getByTestId("search-camera-button")).toHaveTextContent("全部相机");
    fireEvent.click(screen.getByTestId("search-camera-button"));
    const options = screen.getAllByTestId("search-camera-option");
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveAttribute("data-camera", "Canon EOS R5");
    expect(options[0]).toHaveTextContent("12");
    expect(options[1]).toHaveTextContent("Apple iPhone 15");
    expect(options[1]).toHaveTextContent("3");

    // 勾选 Canon → 过滤
    fireEvent.click(within(options[0]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon EOS R5" }),
    );

    // 再勾 iPhone：多选 OR（后端数组契约未到位 → 仍传第一个）
    fireEvent.click(within(options[1]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon EOS R5" }),
    );

    // 取消 Canon → 剩 iPhone 成为第一个
    fireEvent.click(within(options[0]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Apple iPhone 15" }),
    );

    // 全部取消 → 回无条件
    fireEvent.click(within(options[1]).getByRole("checkbox"));
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {}));
    expect(screen.getByTestId("search-camera-button")).toHaveTextContent("全部相机");
  });

  it("相机清单为空：下拉显示空态文案（不过滤）", async () => {
    renderSearch();
    await waitFor(() => expect(cameraListMock).toHaveBeenCalled());

    fireEvent.click(screen.getByTestId("search-camera-button"));
    expect(screen.getByTestId("search-camera-menu")).toHaveTextContent("库内还没有相机信息");
  });
});

// --- 结果渲染 ---------------------------------------------------------------------

describe("搜索：结果与状态", () => {
  it("结果复用画廊网格（同一虚拟化+缩略图管线）：分组/瓦片/计数徽标/内容区居中", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(1, "2026-09-18"),
      makeAsset(2, "2026-09-18"),
      makeAsset(3, "2026-09-17"),
    ]);
    renderSearch();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);
    expect(screen.getByTestId("search-count")).toHaveTextContent("3 个结果");
    // 复用 AssetGrid 的组头与滚动容器；内容区与画廊同款居中容器
    expect(screen.getAllByTestId("gallery-group")).toHaveLength(2);
    expect(screen.getByTestId("search-grid-scroll")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-chips")).not.toBeInTheDocument();
    const content = screen.getByTestId("search-content");
    expect(content.className).toContain("mx-auto");
    expect(content.className).toContain("max-w-[1600px]");
    expect(content.className).toContain("px-6");
  });

  it("RAW+JPG 合并展示与画廊同开关：pairId 成对合并 + RAW+JPG 角标", async () => {
    assetsPageMock.mockResolvedValue([
      { ...makeAsset(1, "2026-09-18"), pairId: 5 },
      { ...makeAsset(2, "2026-09-18", "raw"), pairId: 5 },
    ]);
    renderSearch();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(1);
    expect(tiles[0]).toHaveAttribute("data-asset-id", "1"); // 代表=JPG
    expect(within(tiles[0]).getByTestId("gallery-pair-badge")).toHaveTextContent("RAW+JPG");
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
