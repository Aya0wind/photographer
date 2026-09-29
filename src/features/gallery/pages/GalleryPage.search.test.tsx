import { assetFixture } from "@/test/fixtures";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useNavigate } from "react-router";

import i18n from "@/i18n";
import GalleryPage from "./GalleryPage";
import { SearchRedirect } from "@/app/routes";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { clearGallerySnapshotForTests } from "../lib/galleryCache";
import {
  aiModelsStatus,
  assetGroupDates,
  assetThumbGet,
  assetsByIds,
  assetsCount,
  assetsPage,
  cameraList,
  formatList,
  indexStatus,
  lensList,
  searchSemantic,
  type AiModelStatus,
  type AssetDto,
  type IndexStatus,
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
    assetsCount: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    cameraList: vi.fn(),
    lensList: vi.fn(),
    formatList: vi.fn(),
    searchSemantic: vi.fn(),
    assetsByIds: vi.fn(),
    aiModelsStatus: vi.fn(),
    indexStatus: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const aiModelsStatusMock = vi.mocked(aiModelsStatus);
const indexStatusMock = vi.mocked(indexStatus);
const assetsPageMock = vi.mocked(assetsPage);
const assetsCountMock = vi.mocked(assetsCount);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const cameraListMock = vi.mocked(cameraList);
const lensListMock = vi.mocked(lensList);
const formatListMock = vi.mocked(formatList);

import { convertFileSrc } from "@tauri-apps/api/core";

const convertMock = vi.mocked(convertFileSrc);

function makeAsset(id: number, date: string | null): AssetDto {
  return assetFixture(id, {
    capturedAt: date === null ? null : `${date}T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
  });
}

function gateModel(
  id: string,
  feature: "semantic" | "face",
  state: AiModelStatus["state"],
): AiModelStatus {
  return {
    id,
    installed: state === "done",
    bytesTotal: 1024,
    downloadedBytes: state === "done" ? 1024 : 0,
    version: null,
    feature,
    state,
  };
}

function readyModels(): AiModelStatus[] {
  return [
    gateModel("siglip2-visual", "semantic", "done"),
    gateModel("siglip2-text", "semantic", "done"),
    gateModel("siglip2-tokenizer", "semantic", "done"),
    gateModel("scrfd", "face", "done"),
    gateModel("arcface", "face", "done"),
  ];
}

/** 语义三件缺 text → 门禁 models 分支 */
function gatedModels(): AiModelStatus[] {
  return readyModels().map((m) =>
    m.id === "siglip2-text" ? { ...m, state: "idle" as const, installed: false } : m,
  );
}

function builtIndex(ai?: Partial<IndexStatus["ai"]>): IndexStatus {
  return {
    thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 100 },
    exif: { pending: 0, running: 0, done: 0, failed: 0, total: 100 },
    ai: { pending: 0, running: 0, done: 100, failed: 0, total: 100, ...ai },
    face: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
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
  assetsCountMock.mockReset().mockResolvedValue(null);
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  cameraListMock.mockReset().mockResolvedValue([]);
  lensListMock.mockReset().mockResolvedValue([]);
  formatListMock.mockReset().mockResolvedValue([{ format: "JPG", count: 10 }, { format: "NEF", count: 5 }]);
  vi.mocked(searchSemantic).mockReset().mockResolvedValue([]);
  vi.mocked(assetsByIds).mockReset().mockResolvedValue([]);
  // 语义门禁默认就绪（模型全装 + 索引已建）；各用例按需覆写为被拦态
  aiModelsStatusMock.mockReset().mockResolvedValue(readyModels());
  indexStatusMock.mockReset().mockResolvedValue(builtIndex());
  useAiStore.getState().resetForTests();
  useSettingsStore.setState({ settings: { ...useSettingsStore.getState().settings } });
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  localStorage.clear();
  IntersectionObserverStub.instances = [];
});

// --- 工具条与默认态 -------------------------------------------------------------------

describe("画廊合并：工具条与默认态", () => {
  it("总数独立于分页，关闭收藏筛选后恢复完整列表", async () => {
    const first = makeAsset(1, "2026-09-18");
    const favorite = { ...makeAsset(2, "2026-09-17"), rating: 5 };
    assetsPageMock.mockImplementation(async (_after, _limit, filters) =>
      filters?.ratingMin === 5 ? [favorite] : [first, favorite],
    );
    assetsCountMock.mockImplementation(async (filters) => filters?.ratingMin === 5 ? 1 : 412);
    renderGallery();
    await waitFor(() => expect(screen.getByTestId("search-count")).toHaveTextContent("412"));
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(2);
    expect(screen.getByTestId("gallery-favorite-filter")).toHaveTextContent("已收藏");

    fireEvent.click(screen.getByTestId("gallery-favorite-filter"));
    await waitFor(() => expect(screen.getByTestId("search-count")).toHaveTextContent("1"));
    await waitFor(() => expect(screen.getAllByTestId("gallery-tile")).toHaveLength(1));

    fireEvent.click(screen.getByTestId("gallery-favorite-filter"));
    await waitFor(() => expect(screen.getByTestId("search-count")).toHaveTextContent("412"));
    await waitFor(() => expect(screen.getAllByTestId("gallery-tile")).toHaveLength(2));
  });

  it("工具条含筛选/计数/尺寸（语义输入与选择按钮已删）；默认态查询 assetsPage(0,100)（无 filters）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getByTestId("search-filter-toggle")).toBeInTheDocument();
    expect(screen.getByTestId("search-count")).toBeInTheDocument();
    // ⑤ 语义输入框移除（唯一入口=TitleBar 全局搜索框）；③ 选择按钮移除（入口=瓦片 check 圆钮）
    expect(screen.queryByTestId("semantic-input")).not.toBeInTheDocument();
    expect(screen.queryByTestId("gallery-select-toggle")).not.toBeInTheDocument();
    expect(screen.getAllByTestId("tile-check")).toHaveLength(1);
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100));
    expect(assetsPageMock.mock.calls[0]).toHaveLength(2); // 默认态不传 filters
  });

  it("/search 重定向 /gallery（参数透传）", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/search?format=NEF"]}>
          <Routes>
            <Route path="/search" element={<SearchRedirect />} />
            <Route path="/gallery" element={<GalleryPage />} />
          </Routes>
        </MemoryRouter>
      </I18nextProvider>,
    );
    // 重定向后画廊挂载并按 format=NEF 查询
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
    // ⑤ 语义态状态头（本地输入框已删）：查询词 + 结果数 + 退出按钮
    const header = screen.getByTestId("semantic-status-header");
    expect(header).toHaveTextContent("“日落”");
    expect(header).toHaveTextContent("2 个结果");
    expect(screen.getByTestId("semantic-exit")).toBeInTheDocument();
  });

  it("语义态退出按钮：退出语义回默认画廊（结果清空、语义头消失）", async () => {
    assetsPageMock.mockResolvedValue([]);
    vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.9 }]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getByTestId("semantic-status-header")).toBeInTheDocument();

    fireEvent.click(screen.getByTestId("semantic-exit"));
    // 退出后：语义头消失、回默认态（默认空态引导，非语义空态）
    await waitFor(() =>
      expect(screen.queryByTestId("semantic-status-header")).not.toBeInTheDocument(),
    );
    await waitFor(() => expect(screen.getByTestId("gallery-empty")).toBeInTheDocument());
  });

  it("?format=NEF → 预置 NEF 格式筛选", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery("/gallery?format=NEF");

    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { formats: ["NEF"] }),
    );
  });

  it("?format 撤离：同路径无参导航清筛选回默认态（回归：URL 进入的筛选永久滞留）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    function NavClean() {
      const navigate = useNavigate();
      return (
        <button type="button" data-testid="nav-clean" onClick={() => navigate("/gallery")}>
          go
        </button>
      );
    }
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery?format=NEF"]}>
          <Routes>
            <Route
              path="/gallery"
              element={
                <>
                  <GalleryPage />
                  <NavClean />
                </>
              }
            />
          </Routes>
        </MemoryRouter>
      </I18nextProvider>,
    );

    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { formats: ["NEF"] }),
    );
    // 同路径去掉 ?format（侧栏图库链接等）：筛选必须同步撤销回默认查询
    fireEvent.click(screen.getByTestId("nav-clean"));
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100));
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
    expect(screen.getByTestId("search-filter-count")).toHaveTextContent("2");

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
      expect(within(screen.getByTestId("search-filter-panel")).queryByTestId("search-format-button")).not.toBeInTheDocument();

      fireEvent.click(screen.getByTestId("search-format-button"));
      fireEvent.click(within(screen.getByTestId("search-format-menu")).getByRole("checkbox", { name: /JPG/ }));
      fireEvent.change(screen.getByTestId("search-from"), { target: { value: "2026-01-01" } });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(300);
      });

      const last = assetsPageMock.mock.calls[assetsPageMock.mock.calls.length - 1];
      expect(last?.[2]).toMatchObject({
        formats: ["JPG"],
        capturedAfter: expect.any(String),
      });
      // 条件变更只发一次查询（防抖）
      expect(assetsPageMock).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("格式下拉支持多选和清空；筛选不会生成照片/RAW分类，页面不再提供已存视图", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery();
    await screen.findAllByTestId("gallery-tile");
    expect(screen.queryByText("已存视图")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("search-format-button"));
    const menu = await screen.findByTestId("search-format-menu");
    fireEvent.click(within(menu).getByRole("checkbox", { name: /JPG/ }));
    fireEvent.click(within(menu).getByRole("checkbox", { name: /NEF/ }));
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { formats: ["JPG", "NEF"] }));
    expect(screen.queryByTestId("search-filter-count")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("search-format-clear"));
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100));
    fireEvent.click(screen.getByTestId("search-filter-toggle"));
    expect(screen.queryByText("保存为视图")).not.toBeInTheDocument();
  });

  it("常用条件常驻工具栏；chips 单独移除回默认态", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    expect(screen.queryByTestId("search-filter-count")).not.toBeInTheDocument();
    expect(screen.getByTestId("gallery-quick-filters")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("search-orientation-portrait"));

    expect(screen.queryByTestId("search-filter-count")).not.toBeInTheDocument();
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
    fireEvent.click(screen.getByTestId("search-format-button"));
    fireEvent.click(within(screen.getByTestId("search-format-menu")).getByRole("checkbox", { name: /NEF/ }));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { formats: ["NEF"] }),
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
    fireEvent.click(screen.getByTestId("search-format-button"));
    fireEvent.click(within(screen.getByTestId("search-format-menu")).getByRole("checkbox", { name: /NEF/ }));
    await waitFor(() => expect(screen.getByTestId("search-empty")).toBeInTheDocument());
    expect(screen.queryByTestId("gallery-empty-import")).not.toBeInTheDocument();
  });

  it("语义空结果 → 语义空态文案", async () => {
    vi.mocked(searchSemantic).mockResolvedValue([]);
    renderGallery("/gallery?mode=semantic&q=%E7%8C%AB");

    expect(await screen.findByTestId("semantic-empty")).toHaveTextContent("没有语义匹配的照片");
  });
});

// --- 语义搜索前置门禁 + 索引建立中提示条 ------------------------------------------------

describe("画廊：语义搜索门禁与索引提示条", () => {
  it("门禁·模型未齐：URL 直达也不发查询，工具条下提示（唯一入口=全局搜索框）", async () => {
    aiModelsStatusMock.mockResolvedValue(gatedModels());
    // 同步预置：挂载即拦（不等异步 refresh）
    act(() =>
      useAiStore.setState({ models: gatedModels(), modelsLoaded: true, indexStatus: builtIndex() }),
    );
    renderGallery("/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9");

    expect(searchSemantic).not.toHaveBeenCalled();
    expect(await screen.findByTestId("gallery-semantic-gate")).toHaveTextContent(
      "语义模型未下载",
    );
    expect(screen.getByTestId("gallery-semantic-gate-gosettings")).toBeInTheDocument();
  });

  it("门禁·索引未建立（模型已齐 + ai.total==0 + 库内有资产）：URL 自动执行也被拦", async () => {
    const unbuilt = builtIndex({ done: 0, total: 0 });
    indexStatusMock.mockResolvedValue(unbuilt);
    act(() =>
      useAiStore.setState({ models: readyModels(), modelsLoaded: true, indexStatus: unbuilt }),
    );
    renderGallery("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");

    expect(await screen.findByTestId("gallery-semantic-gate")).toHaveTextContent(
      "语义索引未建立",
    );
    expect(searchSemantic).not.toHaveBeenCalled();
  });

  it("门禁解除后放行：模型装齐后重新进入同一语义 URL → 正常发查询", async () => {
    aiModelsStatusMock.mockResolvedValueOnce(gatedModels()).mockResolvedValue(readyModels());
    act(() =>
      useAiStore.setState({ models: gatedModels(), modelsLoaded: true, indexStatus: builtIndex() }),
    );
    const blocked = renderGallery("/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9");
    expect(await screen.findByTestId("gallery-semantic-gate")).toBeInTheDocument();
    expect(searchSemantic).not.toHaveBeenCalled();
    blocked.unmount();
    clearGallerySnapshotForTests();

    // 模型装齐：aiModelDownloadFinished → 重拉 → 门禁开（事件 refresh 异步，
    // 同步预置保证第二次挂载时门禁已开——URL 自动执行在挂载 effect 内同步判定）
    act(() => {
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadFinished",
        id: "siglip2-text",
        ok: true,
      });
      useAiStore.setState({ models: readyModels() });
    });
    renderGallery("/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9");
    await waitFor(() => expect(searchSemantic).toHaveBeenCalledWith("海边", 100, undefined));
    await waitFor(() =>
      expect(screen.queryByTestId("gallery-semantic-gate")).not.toBeInTheDocument(),
    );
  });

  it("索引建立中：语义态顶部提示条出现；索引完成自动消失", async () => {
    const indexing = builtIndex({ pending: 60, done: 60, total: 120 });
    vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.87 }]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18")]);
    indexStatusMock.mockResolvedValue(indexing);
    act(() =>
      useAiStore.setState({
        models: readyModels(),
        modelsLoaded: true,
        indexStatus: indexing,
      }),
    );
    renderGallery("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getByTestId("semantic-indexing")).toHaveTextContent(
      "索引正在建立，搜索可能遗漏",
    );

    // 索引完成（任务账结算）：提示条自动消失
    act(() => useAiStore.setState({ indexStatus: builtIndex() }));
    await waitFor(() =>
      expect(screen.queryByTestId("semantic-indexing")).not.toBeInTheDocument(),
    );
  });

  it("索引建立中：默认画廊（非语义态）不弹提示条", async () => {
    const indexing = builtIndex({ pending: 60, done: 60, total: 120 });
    indexStatusMock.mockResolvedValue(indexing);
    act(() =>
      useAiStore.setState({
        models: readyModels(),
        modelsLoaded: true,
        indexStatus: indexing,
      }),
    );
    renderGallery();

    await screen.findByTestId("gallery-page");
    expect(screen.queryByTestId("semantic-indexing")).not.toBeInTheDocument();
  });
});
