import { assetFixture } from "@/test/fixtures";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import { mapRegionTree, type RegionCacheRow } from "@/ipc/api/map";
import {
  assetGroupDates,
  assetThumbGet,
  assetViewMark,
  assetsCount,
  assetsPage,
  type AssetDto,
} from "@/ipc/api";
import MapPhotosPage from "./MapPhotosPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { resetViewMarkForTests } from "@/features/gallery/lib/viewMark";

/**
 * 拍摄地图联动照片子页（/map/photos?region=<id>）：
 * - 读参渲染：面包屑标题（mapRegionTree 上溯）/ 资产计数（assets_count）/
 *   首页 assets_page(filters={regionId})
 * - 空态防御兜底；树缓存未就绪回退只显 #id；非法 region 回 /map
 * - 无限滚动哨兵补页（keyset afterId = 上一页末条 id）
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetsCount: vi.fn(),
    assetGroupDates: vi.fn(),
    assetsSeek: vi.fn(),
    assetThumbGet: vi.fn(),
    assetViewMark: vi.fn(),
  };
});

vi.mock("@/ipc/api/map", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api/map")>();
  return {
    ...actual,
    mapRegionTree: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const pageMock = vi.mocked(assetsPage);
const countMock = vi.mocked(assetsCount);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const markMock = vi.mocked(assetViewMark);
const treeMock = vi.mocked(mapRegionTree);

// --- 工具 ---------------------------------------------------------------------------

function treeFixture(): RegionCacheRow[] {
  return [
    { id: 1, parent: null, level: 0, name: "中国", code: "CN", lat: 35, lon: 104 },
    { id: 33, parent: 1, level: 1, name: "浙江省", code: "33", lat: 30, lon: 120 },
    { id: 330100, parent: 33, level: 2, name: "杭州市", code: "3301", lat: 30.2, lon: 120.1 },
  ];
}

function makeAsset(id: number): AssetDto {
  return assetFixture(id, { capturedAt: "2026-09-18T10:00:00" });
}

/** N 条整页（id 递减，keyset DESC 语义；末条 id = firstId - N + 1） */
function makePage(count: number, firstId: number): AssetDto[] {
  return Array.from({ length: count }, (_, i) => makeAsset(firstId - i));
}

function MapProbe() {
  return <div data-testid="map-probe">MAP</div>;
}

function renderPage(entry = "/map/photos?region=330100") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[entry]}>
        <Routes>
          <Route path="/map" element={<MapProbe />} />
          <Route path="/map/photos" element={<MapPhotosPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// IntersectionObserver stub：jsdom 缺失；记录实例供测试手动触发哨兵补页
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
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 800 });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => 10000 });
  (window as unknown as Record<string, unknown>).IntersectionObserver = IntersectionObserverStub;
});

/** 触发最新的哨兵 observer（每页加载后 effect 重建） */
function triggerSentinel(): void {
  const io = IntersectionObserverStub.instances[IntersectionObserverStub.instances.length - 1];
  expect(io).toBeDefined();
  act(() => io.triggerIntersecting());
}

beforeEach(() => {
  vi.clearAllMocks();
  IntersectionObserverStub.instances = [];
  treeMock.mockResolvedValue(treeFixture());
  pageMock.mockResolvedValue([]);
  countMock.mockResolvedValue(0);
  groupDatesMock.mockResolvedValue([]);
  thumbMock.mockResolvedValue({ status: "pending" });
  markMock.mockResolvedValue(undefined);
  resetThumbPipelineForTests();
  resetViewMarkForTests();
});

afterEach(() => {
  document.querySelectorAll(".map-bubble").forEach((el) => el.remove());
});

// --- 场景 ---------------------------------------------------------------------------

describe("MapPhotosPage：读参渲染", () => {
  it("region 参数驱动：面包屑标题（树上溯）+ 计数 + 首页 assets_page(filters={regionId})", async () => {
    pageMock.mockResolvedValue(makePage(2, 50));
    countMock.mockResolvedValue(57);
    renderPage();

    expect(await screen.findByTestId("mapphotos-title")).toHaveTextContent("中国 / 浙江省 / 杭州市");
    expect(screen.getByTestId("mapphotos-count")).toHaveTextContent("57");
    expect(screen.getByTestId("mapphotos-back")).toHaveTextContent("返回地图");
    await waitFor(() =>
      expect(pageMock).toHaveBeenCalledWith(0, 100, { regionId: 330100 }),
    );
    await waitFor(() => expect(countMock).toHaveBeenCalledWith({ regionId: 330100 }));
    await waitFor(() =>
      expect(groupDatesMock).toHaveBeenCalledWith({ regionId: 330100 }),
    );
  });

  it("树缓存未就绪（mapRegionTree → null）→ 标题回退只显 #id", async () => {
    treeMock.mockResolvedValue(null);
    pageMock.mockResolvedValue(makePage(1, 10));
    renderPage();

    expect(await screen.findByTestId("mapphotos-title")).toHaveTextContent("#330100");
  });

  it("空结果 → 居中空态（该地区暂无照片，防御兜底）", async () => {
    pageMock.mockResolvedValue([]);
    renderPage();

    const empty = await screen.findByTestId("mapphotos-empty");
    expect(empty).toHaveTextContent("该地区暂无照片");
    expect(screen.queryByTestId("gallery-tile")).not.toBeInTheDocument();
  });

  it("非法 region 参数（缺省）→ 重定向回 /map", async () => {
    renderPage("/map/photos");
    expect(await screen.findByTestId("map-probe")).toBeInTheDocument();
    expect(pageMock).not.toHaveBeenCalled();
  });

  it("返回地图按钮 → navigate /map", async () => {
    pageMock.mockResolvedValue(makePage(1, 10));
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("mapphotos-title");
    await user.click(screen.getByTestId("mapphotos-back"));
    expect(await screen.findByTestId("map-probe")).toBeInTheDocument();
  });
});

describe("MapPhotosPage：无限滚动补页", () => {
  it("整页结果触哨兵 → keyset 补页（afterId = 上一页末条 id）", async () => {
    pageMock.mockResolvedValueOnce(makePage(100, 100)).mockResolvedValue(makePage(5, 900));
    renderPage();

    await waitFor(() =>
      expect(pageMock).toHaveBeenCalledWith(0, 100, { regionId: 330100 }),
    );
    // 首页 id 100..1（DESC）→ 下一页 afterId=1
    triggerSentinel();
    await waitFor(() =>
      expect(pageMock).toHaveBeenLastCalledWith(1, 100, { regionId: 330100 }),
    );
  });

  it("短页（< 100）到底，触哨兵不补页", async () => {
    pageMock.mockResolvedValue(makePage(3, 30));
    renderPage();

    await waitFor(() =>
      expect(pageMock).toHaveBeenCalledWith(0, 100, { regionId: 330100 }),
    );
    triggerSentinel();
    expect(pageMock).toHaveBeenCalledTimes(1);
  });
});
