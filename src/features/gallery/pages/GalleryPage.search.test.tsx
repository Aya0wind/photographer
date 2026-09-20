import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import GalleryPage from "./GalleryPage";
import { SearchRedirect } from "@/app/routes";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { clearGallerySnapshotForTests } from "../lib/galleryCache";
import {
  assetGroupDates,
  assetThumbGet,
  assetsByIds,
  assetsPage,
  cameraList,
  formatList,
  lensList,
  searchSemantic,
  type AssetDto,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import { useSettingsStore } from "@/stores/settingsStore";

/**
 * 画廊+搜索合并（M4.5 wave-3）：三态（默认/筛选/语义）、URL 协议、筛选面板
 * 在画廊可用、/search 重定向。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    cameraList: vi.fn(),
    lensList: vi.fn(),
    formatList: vi.fn(),
    searchSemantic: vi.fn(),
    assetsByIds: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const assetsPageMock = vi.mocked(assetsPage);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const cameraListMock = vi.mocked(cameraList);
const lensListMock = vi.mocked(lensList);
const formatListMock = vi.mocked(formatList);

import { convertFileSrc } from "@tauri-apps/api/core";

const convertMock = vi.mocked(convertFileSrc);

function makeAsset(id: number, date: string | null): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: date === null ? null : `${date}T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
  };
}

function renderGallery(initialEntry = "/gallery") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialEntry]}>
        <Routes>
          <Route path="/gallery" element={<GalleryPage />} />
          <Route path="/search" element={<SearchRedirect />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

class IntersectionObserverStub {
  static instances: IntersectionObserverStub[] = [];
  callback: IntersectionObserverCallback;
  observed: Element[] = [];
  constructor(callback: IntersectionObserverCallback) {
    this.callback = callback;
    IntersectionObserverStub.instances.push(this);
  }
  observe(el: Element): void {
    this.observed.push(el);
  }
  unobserve(): void {}
  disconnect(): void {
    this.observed = [];
  }
  triggerIntersecting(): void {
    for (const el of this.observed) {
      this.callback(
        [{ isIntersecting: true, target: el } as IntersectionObserverEntry],
        this as unknown as IntersectionObserver,
      );
    }
  }
}

beforeAll(() => {
  (window as unknown as Record<string, unknown>).IntersectionObserver = IntersectionObserverStub;
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  assetsPageMock.mockReset().mockResolvedValue([]);
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  cameraListMock.mockReset().mockResolvedValue([]);
  lensListMock.mockReset().mockResolvedValue([]);
  formatListMock.mockReset().mockResolvedValue([]);
  vi.mocked(searchSemantic).mockReset();
  vi.mocked(assetsByIds).mockReset().mockResolvedValue([]);
  useAiStore.getState().resetForTests();
  useSettingsStore.setState({ settings: { ...useSettingsStore.getState().settings } });
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  localStorage.clear();
  IntersectionObserverStub.instances = [];
});

// --- 工具条与默认态 -------------------------------------------------------------------

describe("画廊合并：工具条与默认态", () => {
  it("工具条含语义输入/筛选/选择/尺寸；默认态查询 assetsPage(0,100)（无 filters）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getByTestId("semantic-input")).toBeInTheDocument();
    expect(screen.getByTestId("search-filter-toggle")).toBeInTheDocument();
    expect(screen.getByTestId("gallery-select-toggle")).toBeInTheDocument();
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100));
    expect(assetsPageMock.mock.calls[0]).toHaveLength(2); // 默认态不传 filters
  });

  it("/search 重定向 /gallery（参数透传）", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/search?kind=raw"]}>
          <Routes>
            <Route path="/search" element={<SearchRedirect />} />
            <Route path="/gallery" element={<GalleryPage />} />
          </Routes>
        </MemoryRouter>
      </I18nextProvider>,
    );
    // 重定向后画廊挂载并按 kind=raw 查询
    expect(screen.getByTestId("gallery-skeleton")).toBeInTheDocument();
  });
});

// --- URL 协议 -------------------------------------------------------------------------

describe("画廊合并：URL 协议", () => {
  it("?mode=semantic&q=… → 语义模式自动执行（分数角标渲染）", async () => {
    assetsPageMock.mockResolvedValue([]);
    vi.mocked(searchSemantic).mockResolvedValue([
      { assetId: 1, score: 0.87 },
      { assetId: 2, score: 0.42 },
    ]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18"), makeAsset(2, "2026-09-17")]);
    renderGallery("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    expect(searchSemantic).toHaveBeenCalledWith("日落", 100, undefined);
    expect(within(tiles[0]).getByTestId("search-score-badge")).toHaveTextContent("87%");
    // 语义输入预填
    expect(screen.getByTestId("semantic-input")).toHaveValue("日落");
  });

  it("?kind=raw → 预置 RAW 类型筛选（kinds=[raw]）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery("/gallery?kind=raw");

    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { kinds: ["raw"] }),
    );
  });
});

// --- 筛选面板（画廊内可用） -------------------------------------------------------------

describe("画廊合并：筛选面板", () => {
  it("相机多选使用后端 cameras 严格契约，不再发送会被忽略的单数 camera", async () => {
    cameraListMock.mockResolvedValue([
      { camera: "Canon EOS R5", count: 12 },
      { camera: "Sony A7R5", count: 8 },
    ]);
    renderGallery();
    await screen.findByTestId("gallery-empty");
    fireEvent.click(screen.getByTestId("search-filter-toggle"));
    fireEvent.click(screen.getByTestId("search-camera-button"));

    const options = await screen.findAllByTestId("search-camera-option");
    fireEvent.click(within(options[0]).getByRole("checkbox"));
    fireEvent.click(within(options[1]).getByRole("checkbox"));

    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
        cameras: ["Canon EOS R5", "Sony A7R5"],
      }),
    );
    const calls = assetsPageMock.mock.calls;
    const payload = calls[calls.length - 1]?.[2] as unknown as Record<string, unknown>;
    expect(payload).not.toHaveProperty("camera");

    // 清除入口属于下拉控件本身，而不是菜单内另设一行按钮。
    const menu = screen.getByTestId("search-camera-menu");
    const clear = screen.getByTestId("search-camera-clear");
    expect(menu).not.toContainElement(clear);
    expect(screen.getByTestId("search-camera-button").parentElement).toContainElement(clear);
    fireEvent.click(clear);
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100));
    expect(screen.queryByTestId("search-camera-clear")).not.toBeInTheDocument();
  });

  it("默认收起；展开后条件变更触发筛选查询（防抖 300ms 后单次）", async () => {
    vi.useFakeTimers();
    try {
      assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
      renderGallery();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.queryByTestId("search-filter-panel")).not.toBeInTheDocument();

      fireEvent.click(screen.getByTestId("search-filter-toggle"));
      expect(screen.getByTestId("search-filter-panel")).toBeInTheDocument();

      fireEvent.click(screen.getByTestId("search-kind-photo"));
      fireEvent.change(screen.getByTestId("search-from"), { target: { value: "2026-01-01" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });

      const last = assetsPageMock.mock.calls[assetsPageMock.mock.calls.length - 1];
      expect(last?.[2]).toMatchObject({
        kinds: ["photo", "raw"],
        capturedAfter: expect.any(String),
      });
      // 条件变更只发一次查询（防抖）
      expect(assetsPageMock).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("激活条件徽标 + chips 单独移除回默认态", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    expect(screen.queryByTestId("search-filter-count")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("search-filter-toggle"));
    fireEvent.click(screen.getByTestId("search-orientation-portrait"));

    await waitFor(() => expect(screen.getByTestId("search-filter-count")).toHaveTextContent("1"));
    expect(screen.getByTestId("search-chip")).toHaveTextContent("竖拍");
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { orientation: "portrait" }),
    );

    // 移除 chip → 回默认（无 filters 两参调用）
    fireEvent.click(screen.getByTestId("search-chip-remove"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("search-filter-chips")).not.toBeInTheDocument(),
    );
  });
});

// --- 三态切换 -------------------------------------------------------------------------

describe("画廊合并：三态切换", () => {
  it("语义态 → 修改筛选自动退出回筛选态", async () => {
    assetsPageMock.mockResolvedValue([]);
    vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.9 }]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery("/gallery?mode=semantic&q=%E7%8C%AB");

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getAllByTestId("search-score-badge")).toHaveLength(1);

    // 展开筛选面板改条件 → 退出语义态（分数角标消失、走 assetsPage filters）
    fireEvent.click(screen.getByTestId("search-filter-toggle"));
    fireEvent.click(screen.getByTestId("search-kind-video"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { kinds: ["video"] }),
    );
    await waitFor(() =>
      expect(screen.queryByTestId("search-score-badge")).not.toBeInTheDocument(),
    );
  });

  it("筛选空结果 → 筛选空态（默认空库仍是导入引导）", async () => {
    assetsPageMock.mockResolvedValue([]);
    renderGallery();
    await screen.findByTestId("gallery-empty");
    expect(screen.getByTestId("gallery-empty-import")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("search-filter-toggle"));
    fireEvent.click(screen.getByTestId("search-kind-video"));
    await waitFor(() => expect(screen.getByTestId("search-empty")).toBeInTheDocument());
    expect(screen.queryByTestId("gallery-empty-import")).not.toBeInTheDocument();
  });

  it("语义空结果 → 语义空态文案", async () => {
    vi.mocked(searchSemantic).mockResolvedValue([]);
    renderGallery("/gallery?mode=semantic&q=%E7%8C%AB");

    expect(await screen.findByTestId("semantic-empty")).toHaveTextContent("没有语义匹配的照片");
  });
});
