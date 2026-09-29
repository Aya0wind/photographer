import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import { subscribeAppEvents } from "@/ipc/api/events";
import type { AppEvent } from "@/ipc/api/types";
import {
  mapClusters,
  mapGeoDownloadStart,
  mapGeoStatus,
  type GeoStatus,
  type MapCluster,
} from "@/ipc/api/map";
import MapPage from "./MapPage";

// --- 模拟层 ------------------------------------------------------------------

vi.mock("maplibre-gl", () => {
  class Marker {
    element: HTMLElement;
    constructor(opts: { element: HTMLElement }) {
      this.element = opts.element;
    }
    setLngLat() {
      return this;
    }
    addTo() {
      document.body.appendChild(this.element);
      return this;
    }
    remove() {
      this.element.remove();
    }
  }
  class Map {
    handlers: Record<string, (() => void)[]> = {};
    markers: Marker[] = [];
    constructor(public opts: unknown) {}
    on(event: string, cb: () => void) {
      (this.handlers[event] ??= []).push(cb);
      return this;
    }
    addControl() {}
    getZoom() {
      return 1.5;
    }
    flyTo() {}
    remove() {}
    /** 测试触发器：模拟地图事件（气泡 DOM 已在 document） */
    fire(event: string) {
      this.handlers[event]?.forEach((cb) => cb());
    }
  }
  return { Map, Marker, NavigationControl: class {} };
});

vi.mock("@/ipc/api/map", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api/map")>();
  return {
    ...actual,
    mapGeoStatus: vi.fn(),
    mapGeoDownloadStart: vi.fn(),
    mapRegionTree: vi.fn(),
    mapClusters: vi.fn(),
  };
});

vi.mock("@/ipc/api/events", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api/events")>();
  return {
    ...actual,
    subscribeAppEvents: vi.fn(),
  };
});

const statusMock = vi.mocked(mapGeoStatus);
const downloadMock = vi.mocked(mapGeoDownloadStart);
const clustersMock = vi.mocked(mapClusters);
const subscribeMock = vi.mocked(subscribeAppEvents);

// --- 工具 --------------------------------------------------------------------

function status(overrides: Partial<GeoStatus> = {}): GeoStatus {
  return {
    installed: true,
    phase: "ready",
    done: 0,
    total: 0,
    message: null,
    cacheReady: true,
    ...overrides,
  };
}

function cluster(overrides: Partial<MapCluster> = {}): MapCluster {
  return {
    regionId: 10,
    name: "中国",
    lat: 30,
    lon: 104,
    count: 12,
    samples: [
      { id: 1, path: "X:/a.jpg", kind: "photo" },
      { id: 2, path: "X:/b.jpg", kind: "photo" },
      { id: 3, path: "X:/c.jpg", kind: "photo" },
    ],
    ...overrides,
  };
}

function renderPage() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/map"]}>
        <Routes>
          <Route path="/map" element={<MapPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  downloadMock.mockResolvedValue(undefined);
  subscribeMock.mockResolvedValue(() => {});
  // hooks 无条件跑（不渲染地图的分支也会拉聚合）：统一兜底，各测试再覆盖
  clustersMock.mockResolvedValue([]);
  statusMock.mockResolvedValue(status());
});

// --- 场景 --------------------------------------------------------------------

describe("MapPage 数据管线状态机", () => {
  it("未安装 → 下载引导；点击触发下载", async () => {
    statusMock.mockResolvedValue(status({ installed: false, phase: "notInstalled" }));
    renderPage();
    expect(await screen.findByTestId("map-download-hint")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("map-download-start"));
    await waitFor(() => expect(downloadMock).toHaveBeenCalled());
  });

  it("下载中 → 进度页", async () => {
    statusMock.mockResolvedValue(
      status({ phase: "downloading", done: 40, total: 400 }),
    );
    renderPage();
    expect(await screen.findByTestId("map-preparing")).toBeInTheDocument();
  });

  it("失败 → 错误 + 重试按钮", async () => {
    statusMock.mockResolvedValue(
      status({ phase: "failed", message: "network" }),
    );
    renderPage();
    expect(await screen.findByTestId("map-failed")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("map-retry"));
    await waitFor(() => expect(downloadMock).toHaveBeenCalled());
  });

  it("就绪 + 无 GPS 照片 → 空态提示", async () => {
    statusMock.mockResolvedValue(status());
    clustersMock.mockResolvedValue([]);
    renderPage();
    expect(await screen.findByTestId("map-empty")).toBeInTheDocument();
  });
});

describe("MapPage 地图交互", () => {
  it("就绪 → 画布 + 全球面包屑 + 气泡；点击气泡下钻（parent 限定）", async () => {
    statusMock.mockResolvedValue(status());
    clustersMock.mockResolvedValue([cluster()]);
    renderPage();
    expect(await screen.findByTestId("map-canvas")).toBeInTheDocument();
    expect(screen.getByTestId("map-breadcrumb")).toBeInTheDocument();
    await waitFor(() => expect(clustersMock).toHaveBeenCalledWith(0, null));
    // 气泡 DOM 由 MapCanvas 命令式挂载
    const bubble = await screen.findByRole("button", { name: /中国/ });
    fireEvent.click(bubble);
    await waitFor(() => expect(clustersMock).toHaveBeenCalledWith(1, 10));
    expect(screen.getByTestId("map-breadcrumb").textContent).toContain("中国");
  });

  it("换一批 → 重新拉当前层", async () => {
    statusMock.mockResolvedValue(status());
    clustersMock.mockResolvedValue([cluster()]);
    renderPage();
    await screen.findByTestId("map-canvas");
    fireEvent.click(screen.getByTestId("map-refresh"));
    await waitFor(() => expect(clustersMock.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("面包屑回退：钻两层后点国家级 → 回省级层（差一回归）", async () => {
    statusMock.mockResolvedValue(status());
    clustersMock
      .mockResolvedValueOnce([cluster()]) // 国家层：中国
      .mockResolvedValueOnce([cluster({ regionId: 11, name: "浙江省" })]) // 省层
      .mockResolvedValue([cluster({ regionId: 12, name: "杭州市" })]); // 市层+
    renderPage();
    await screen.findByTestId("map-canvas");
    // 气泡 DOM 挂在 body（mock Marker），面包屑在页内：按「不在面包屑里」挑气泡
    const bubbleNamed = async (name: string) => {
      const btn = await screen.findAllByRole("button", { name });
      const bubble = btn.find((b) => !b.closest('[data-testid="map-breadcrumb"]'));
      expect(bubble).toBeDefined();
      return bubble!;
    };
    fireEvent.click(await bubbleNamed("中国"));
    await waitFor(() => expect(clustersMock).toHaveBeenCalledWith(1, 10));
    fireEvent.click(await bubbleNamed("浙江省"));
    await waitFor(() => expect(clustersMock).toHaveBeenCalledWith(2, 11));
    // 面包屑「中国」是 level 0 节点：回它 = 显示子层（省级 level 1，不是市级 2）
    clustersMock.mockClear();
    const crumb = screen
      .getAllByRole("button")
      .find((b) => b.textContent === "中国" && b.closest('[data-testid="map-breadcrumb"]'));
    expect(crumb).toBeDefined();
    fireEvent.click(crumb!);
    await waitFor(() => expect(clustersMock).toHaveBeenCalledWith(1, 10));
    expect(clustersMock.mock.calls.some(([level]) => level === 2)).toBe(false);
  });

  it("mapRegionsUpdated 事件 → 缓存失效重拉", async () => {
    statusMock.mockResolvedValue(status());
    clustersMock.mockResolvedValue([]);
    let handler: ((e: AppEvent) => void) | null = null;
    subscribeMock.mockImplementation(async (cb) => {
      handler = cb;
      return () => {};
    });
    renderPage();
    await screen.findByTestId("map-canvas");
    expect(handler).not.toBeNull();
    handler!({ type: "mapRegionsUpdated" });
    await waitFor(() => expect(clustersMock.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("回填中 → 地图可看 + 顶部进度提示", async () => {
    statusMock.mockResolvedValue(
      status({ phase: "backfilling", done: 50, total: 100 }),
    );
    clustersMock.mockResolvedValue([cluster()]);
    renderPage();
    expect(await screen.findByTestId("map-canvas")).toBeInTheDocument();
    expect(await screen.findByTestId("map-backfill-hint")).toBeInTheDocument();
  });
});
