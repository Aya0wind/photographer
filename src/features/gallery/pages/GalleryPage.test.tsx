import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import GalleryPage from "./GalleryPage";
import { resetThumbPipelineForTests, emitAssetEventForTests } from "../lib/thumbPipeline";
import {
  assetGroupDates,
  assetThumbGet,
  assetsPage,
  isIpcAvailable,
  type AssetDto,
  type AssetGroupDate,
} from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    isIpcAvailable: vi.fn(() => true),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const assetsPageMock = vi.mocked(assetsPage);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const ipcAvailableMock = vi.mocked(isIpcAvailable);
const convertMock = vi.mocked(convertFileSrc);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(
  id: number,
  date: string | null,
  kind: AssetDto["kind"] = "photo",
  name?: string,
): AssetDto {
  const file = name ?? `IMG_${String(id).padStart(4, "0")}.JPG`;
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\2026\\${file}`,
    name: file,
    kind,
    capturedAt: date === null ? null : `${date}T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
  };
}

/** N 条同日期资产的整页（id 递减，keyset DESC 语义） */
function makePage(count: number, date: string | null, firstId: number): AssetDto[] {
  return Array.from({ length: count }, (_, i) => makeAsset(firstId - i, date));
}

function ImportProbe() {
  return <div data-testid="import-probe">IMPORT_PAGE</div>;
}

function renderGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/gallery" element={<GalleryPage />} />
          <Route path="/import" element={<ImportProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// jsdom 无布局：offsetWidth/offsetHeight 恒 0 → TanStack Virtual 视口为空一行都不渲染；
// scrollHeight/clientHeight 恒 0 → getMaxScrollOffset 钳到 0（日期跳转滚不动）。
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 1200,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => 800,
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get: () => 800,
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get: () => 10000,
  });
});

// IntersectionObserver stub：jsdom 缺失；记录实例供测试手动触发哨兵
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
  disconnect(): void {}
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
});

/** 触发最新的哨兵 observer（每页加载后 effect 重建） */
function triggerSentinel(): void {
  const io = IntersectionObserverStub.instances[IntersectionObserverStub.instances.length - 1];
  expect(io).toBeDefined();
  act(() => io.triggerIntersecting());
}

beforeEach(() => {
  assetsPageMock.mockReset().mockResolvedValue([]);
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue(null);
  convertMock.mockReset().mockReturnValue("");
  ipcAvailableMock.mockReset().mockReturnValue(true);
  resetThumbPipelineForTests();
  IntersectionObserverStub.instances = [];
});

// --- 分组渲染 ---------------------------------------------------------------------

describe("画廊：日期分组照片墙", () => {
  it("按日期分组渲染组头（日期+数量）与资产块", async () => {
    assetsPageMock.mockResolvedValue([
      ...makePage(3, "2026-09-18", 5),
      ...makePage(2, "2026-09-17", 2),
    ]);

    renderGallery();

    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(5);
    const headers = screen.getAllByTestId("gallery-group");
    expect(headers).toHaveLength(2);
    expect(headers[0]).toHaveTextContent("2026年9月18日");
    expect(headers[0]).toHaveTextContent("3 张");
    expect(headers[1]).toHaveTextContent("2026年9月17日");
    expect(headers[1]).toHaveTextContent("2 张");
  });

  it("capturedAt 为 NULL 的资产归「未知日期」组且排在最前", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(1, "2026-01-05"),
      makeAsset(2, null),
      makeAsset(3, null),
      makeAsset(4, "2026-09-18"),
    ]);

    renderGallery();

    // 未知组组头（专用 testid）在最前：先于任何已知日期组
    const unknown = await screen.findByTestId("gallery-group-unknown");
    expect(unknown).toHaveTextContent("未知日期");
    expect(unknown).toHaveTextContent("2 张");
    const known = await screen.findAllByTestId("gallery-group");
    expect(known).toHaveLength(2);
    expect(unknown.compareDocumentPosition(known[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(4);
  });

  it("缩略图管线：photo/RAW 未命中先占位，thumbnailReady 后重试成功；RAW 恒带水印角标", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(1, "2026-09-18"),
      makeAsset(2, "2026-09-18", "raw", "IMG_0002.CR3"),
    ]);
    // photo：首次未命中，thumbnailReady 后重试命中；RAW：内嵌提取未就绪（null）→占位
    thumbMock.mockImplementation(async (id: number) =>
      id === 1 && thumbMock.mock.calls.filter(([i]) => i === 1).length >= 2
        ? "C:\\thumbs\\256\\img1.jpg"
        : null,
    );
    convertMock.mockImplementation((p: string) => `asset://${p}`);

    renderGallery();

    // 初始：photo/RAW 都进管线（RAW 走后端内嵌提取，可能较慢）——先占位
    expect(await screen.findByTestId("thumb-photo")).toBeInTheDocument();
    expect(screen.getByTestId("thumb-raw")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(1, 240));
    // RAW 不再短路：同样请求缩略图
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(2, 240));
    // RAW 水印角标（占位期就渲染）
    expect(screen.getByTestId("thumb-raw-badge")).toBeInTheDocument();

    // 后台补齐缓存 → 事件驱动重试 → img 出现
    act(() => {
      emitAssetEventForTests({
        type: "thumbnailReady",
        assetId: 1,
        size: 256,
        path: "C:\\thumbs\\256\\img1.jpg",
      });
    });
    const img = await screen.findByRole("img", { name: "IMG_0001.JPG" });
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\256\\img1.jpg");
    // RAW 仍未就绪：占位 + 水印
    expect(screen.getByTestId("thumb-raw")).toBeInTheDocument();
    expect(screen.getByTestId("thumb-raw-badge")).toBeInTheDocument();
  });

  it("点击资产块打开查看器（?asset=），Esc 关闭返回画廊（画廊未卸载）", async () => {
    assetsPageMock.mockResolvedValue(makePage(3, "2026-09-18", 3));
    const user = userEvent.setup();
    renderGallery();

    const tile = (await screen.findAllByTestId("gallery-tile"))[0];
    await user.click(tile);

    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0003.JPG");

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
    expect(screen.getByTestId("gallery-page")).toBeInTheDocument();
  });
});

// --- 无限滚动 ---------------------------------------------------------------------

describe("画廊：无限滚动（keyset 下一页）", () => {
  it("哨兵触发 assetsPage(afterId=已加载最后一条, 100)；短页后停止", async () => {
    // 第一页整页 100 条（id 200..101），第二页短页 30 条（id 100..71）→ 耗尽
    assetsPageMock.mockImplementation(async (afterId: number) => {
      if (afterId === 0) return makePage(100, "2026-09-18", 200);
      if (afterId === 101) return makePage(30, "2026-09-17", 100);
      return [];
    });

    renderGallery();
    await screen.findAllByTestId("gallery-tile");
    expect(assetsPageMock).toHaveBeenCalledTimes(1);
    expect(assetsPageMock).toHaveBeenCalledWith(0, 100);

    triggerSentinel();
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledTimes(2));
    expect(assetsPageMock).toHaveBeenLastCalledWith(101, 100);

    // 第二页为短页（30 < 100）→ 耗尽，再触发不再请求
    triggerSentinel();
    await Promise.resolve();
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
  });

  it("加载中不重复触发（并发防抖）", async () => {
    let resolveFirst: (page: AssetDto[]) => void = () => {};
    let resolveSecond: (page: AssetDto[]) => void = () => {};
    assetsPageMock
      .mockImplementationOnce(() => new Promise<AssetDto[]>((resolve) => (resolveFirst = resolve)))
      .mockImplementationOnce(() => new Promise<AssetDto[]>((resolve) => (resolveSecond = resolve)));

    renderGallery();
    await screen.findByTestId("gallery-skeleton");

    // 首页整页 100 条（保持 hasMore）；第二页 pending
    await act(async () => {
      resolveFirst(makePage(100, "2026-09-18", 200));
      await Promise.resolve();
    });
    await screen.findAllByTestId("gallery-tile");

    // 连续触发哨兵：第二页在途时只有一次在途请求
    triggerSentinel();
    triggerSentinel();
    await Promise.resolve();
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
    expect(assetsPageMock).toHaveBeenLastCalledWith(101, 100);

    // 第二页为短页 → 耗尽：结算后再触发不再请求（证明 loading 态已复位）
    await act(async () => {
      resolveSecond(makePage(1, "2026-09-17", 8));
      await Promise.resolve();
    });
    triggerSentinel();
    await Promise.resolve();
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
  });
});

// --- 状态态 -----------------------------------------------------------------------

describe("画廊：空态与降级", () => {
  it("空库：引导文案 + 去导入按钮跳转", async () => {
    assetsPageMock.mockResolvedValue([]);
    const user = userEvent.setup();
    renderGallery();

    const empty = await screen.findByTestId("gallery-empty");
    expect(empty).toHaveTextContent("库里还没有照片");
    await user.click(screen.getByTestId("gallery-empty-import"));
    expect(await screen.findByTestId("import-probe")).toBeInTheDocument();
  });

  it("IPC 不可用：降级提示 + 重试恢复", async () => {
    ipcAvailableMock.mockReturnValue(false);
    assetsPageMock.mockResolvedValue([]);
    const user = userEvent.setup();
    renderGallery();

    expect(await screen.findByTestId("gallery-degraded")).toHaveTextContent("后端未连接");

    // 重试：后端恢复（IPC 可用 + 有数据）→ 正常画廊
    ipcAvailableMock.mockReturnValue(true);
    assetsPageMock.mockResolvedValue(makePage(2, "2026-09-18", 5));
    await user.click(screen.getByRole("button", { name: "重试" }));
    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(2);
  });
});

// --- 日期 chips 与跳转 --------------------------------------------------------------

describe("画廊：日期 chips 条", () => {
  const dates: AssetGroupDate[] = [
    { date: "2026-09-18", count: 3, coverAssetId: 1 },
    { date: "2026-09-17", count: 2, coverAssetId: 4 },
    { date: null, count: 5, coverAssetId: 9 },
  ];

  it("chips 来自 assetGroupDates，含「未知」chip 且排最前", async () => {
    assetsPageMock.mockResolvedValue(makePage(3, "2026-09-18", 3));
    groupDatesMock.mockResolvedValue(dates);
    renderGallery();

    const known = await screen.findAllByTestId("gallery-chip");
    expect(known).toHaveLength(2);
    const unknown = screen.getByTestId("gallery-chip-unknown");
    expect(unknown).toHaveTextContent("未知");
    expect(unknown).toHaveTextContent("5");
    // 未知 chip 在所有已知日期 chip 之前
    expect(unknown.compareDocumentPosition(known[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy();
    expect(known[0]).toHaveAttribute("data-date", "2026-09-18");
    expect(known[1]).toHaveAttribute("data-date", "2026-09-17");
  });

  it("点击 chip 滚动到对应组（scrollTop 对齐组头）", async () => {
    assetsPageMock.mockResolvedValue([
      ...makePage(6, "2026-09-18", 20),
      ...makePage(6, "2026-09-17", 10),
    ]);
    groupDatesMock.mockResolvedValue(dates);
    const user = userEvent.setup();
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    const scroll = screen.getByTestId("gallery-grid-scroll");
    expect(scroll.scrollTop).toBe(0);

    // 第二个日期 chip（09-17，DOM 顺序在 09-18 之后）
    await user.click(screen.getAllByTestId("gallery-chip")[1]);

    await waitFor(() => expect(scroll.scrollTop).toBeGreaterThan(0));
    const headers = screen.getAllByTestId("gallery-group");
    expect(headers[0]).toHaveTextContent("2026年9月18日");
    expect(headers[1]).toHaveTextContent("2026年9月17日");
  });

  it("目标组未加载时顺序补页直到出现再滚动", async () => {
    // 首页整页 100 条（09-18）；chip 目标 09-16 在第二页（短页）
    assetsPageMock.mockImplementation(async (afterId: number) => {
      if (afterId === 0) return makePage(100, "2026-09-18", 200);
      if (afterId === 101) return makePage(2, "2026-09-16", 10);
      return [];
    });
    groupDatesMock.mockResolvedValue([
      { date: "2026-09-18", count: 100, coverAssetId: 200 },
      { date: "2026-09-16", count: 2, coverAssetId: 10 },
    ]);
    const user = userEvent.setup();
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    expect(screen.queryByText("2026年9月16日")).not.toBeInTheDocument();

    await user.click(screen.getAllByTestId("gallery-chip")[1]);
    await waitFor(() => expect(screen.getByText("2026年9月16日")).toBeInTheDocument());
    expect(assetsPageMock).toHaveBeenCalledWith(101, 100);
  });

  it("日历按钮：年月下拉弹层 + 跳转滚动到该月最新组并收起（v1 年月跳转）", async () => {
    // 两组：2026-09（首屏）+ 2025-05（视口外）；跳 2025-05 需要真实滚动
    assetsPageMock.mockResolvedValue([
      ...makePage(6, "2026-09-18", 20),
      ...makePage(6, "2025-05-10", 10),
    ]);
    groupDatesMock.mockResolvedValue([
      { date: "2026-09-18", count: 6, coverAssetId: 20 },
      { date: "2025-05-10", count: 6, coverAssetId: 10 },
    ]);
    const user = userEvent.setup();
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    const scroll = screen.getByTestId("gallery-grid-scroll");
    expect(scroll.scrollTop).toBe(0);

    await user.click(screen.getByTestId("gallery-calendar-button"));
    const panel = screen.getByTestId("gallery-calendar-panel");
    expect(within(panel).getByTestId("gallery-calendar-year")).toBeInTheDocument();
    expect(within(panel).getByTestId("gallery-calendar-month")).toBeInTheDocument();

    // 选 2025 年 5 月 → 跳转到该月最新组（2025-05-10）
    fireEvent.change(within(panel).getByTestId("gallery-calendar-year"), {
      target: { value: "2025" },
    });
    fireEvent.change(within(panel).getByTestId("gallery-calendar-month"), {
      target: { value: "5" },
    });
    await user.click(screen.getByTestId("gallery-calendar-jump"));

    await waitFor(() => expect(scroll.scrollTop).toBeGreaterThan(0));
    await waitFor(() =>
      expect(screen.queryByTestId("gallery-calendar-panel")).not.toBeInTheDocument(),
    );
  });
});

// --- RAW+JPG 合并展示 --------------------------------------------------------------

describe("RAW+JPG 合并展示", () => {
  function pairAssets(): AssetDto[] {
    return [
      { ...makeAsset(1, "2026-09-18"), pairId: 9 }, // JPG（代表卡）
      { ...makeAsset(2, "2026-09-18", "raw", "IMG_0002.CR3"), pairId: 9 },
      makeAsset(3, "2026-09-18"),
    ];
  }

  it("pairId 成对合并为一张卡（代表=JPG）+ RAW+JPG 角标；点击开 JPG 版", async () => {
    assetsPageMock.mockResolvedValue(pairAssets());
    const user = userEvent.setup();
    renderGallery();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    const merged = tiles.find((el) => el.getAttribute("data-asset-id") === "1");
    expect(merged).toBeDefined();
    expect(within(merged!).getByTestId("gallery-pair-badge")).toHaveTextContent("RAW+JPG");
    const single = tiles.find((el) => el.getAttribute("data-asset-id") === "3")!;
    expect(single.querySelector('[data-testid="gallery-pair-badge"]')).toBeNull();

    await user.click(merged!);
    expect(await screen.findByTestId("viewer-name")).toHaveTextContent("IMG_0001.JPG");
  });

  it("设置关闭合并 → RAW/JPG 分开展示（3 张卡，无合并角标；RAW 卡有水印）", async () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, gallery: { mergeRawJpg: false } },
    }));
    assetsPageMock.mockResolvedValue(pairAssets());
    renderGallery();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);
    expect(screen.queryByTestId("gallery-pair-badge")).not.toBeInTheDocument();
    expect(screen.getByTestId("thumb-raw-badge")).toBeInTheDocument();

    // 恢复默认开（store 跨用例共享）
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, gallery: { mergeRawJpg: true } },
    }));
  });
});

// --- 工具条：居中 / 三档尺寸 ---------------------------------------------------------

describe("画廊工具条：居中与尺寸", () => {
  it("内容区水平居中 + 对称 padding（max-w 容器，chips 与网格同宽对齐）", async () => {
    assetsPageMock.mockResolvedValue(makePage(2, "2026-09-18", 2));
    renderGallery();

    const content = await screen.findByTestId("gallery-content");
    expect(content.className).toContain("mx-auto");
    expect(content.className).toContain("max-w-[1600px]");
    expect(content.className).toContain("px-6");
  });

  it("三档尺寸：默认中档 200px；切大 280px 并写 localStorage", async () => {
    assetsPageMock.mockResolvedValue(makePage(2, "2026-09-18", 2));
    const user = userEvent.setup();
    renderGallery();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles[0].style.width).toBe("200px");

    await user.click(screen.getByTestId("gallery-tile-size-large"));
    expect(screen.getAllByTestId("gallery-tile")[0].style.width).toBe("280px");
    expect(localStorage.getItem("smartphoto.gallery.tileSize")).toBe("large");
  });

  it("localStorage 预设小档 → 首渲染 120px（画廊/搜索跨页共享）", async () => {
    localStorage.setItem("smartphoto.gallery.tileSize", "small");
    assetsPageMock.mockResolvedValue(makePage(2, "2026-09-18", 2));
    renderGallery();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles[0].style.width).toBe("120px");
  });
});
