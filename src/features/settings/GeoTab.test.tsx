import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import { subscribeAppEvents } from "@/ipc/api/events";
import type { AppEvent } from "@/ipc/api/types";
import { mapGeoCancel, mapGeoDelete, mapGeoDownloadStart, mapGeoStatus, type GeoStatus } from "@/ipc/api/map";
import GeoTab from "./GeoTab";

vi.mock("@/ipc/api/map", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api/map")>();
  return {
    ...actual,
    mapGeoStatus: vi.fn(),
    mapGeoDownloadStart: vi.fn(),
    mapGeoCancel: vi.fn(),
    mapGeoDelete: vi.fn(),
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
const cancelMock = vi.mocked(mapGeoCancel);
const deleteMock = vi.mocked(mapGeoDelete);
const subscribeMock = vi.mocked(subscribeAppEvents);

function status(overrides: Partial<GeoStatus> = {}): GeoStatus {
  return {
    installed: true,
    phase: "ready",
    done: 0,
    total: 0,
    message: null,
    cacheReady: true,
    datavFiles: 361,
    ...overrides,
  };
}

function renderTab() {
  return render(
    <I18nextProvider i18n={i18n}>
      <GeoTab />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  statusMock.mockResolvedValue(status());
  downloadMock.mockResolvedValue(undefined);
  cancelMock.mockResolvedValue(undefined);
  deleteMock.mockResolvedValue(undefined);
  subscribeMock.mockResolvedValue(() => {});
});

describe("设置页 地图数据管理", () => {
  it("未安装 → 下载按钮触发 mapGeoDownloadStart", async () => {
    statusMock.mockResolvedValue(status({ installed: false, phase: "notInstalled" }));
    renderTab();
    fireEvent.click(await screen.findByTestId("settings-geo-download"));
    await waitFor(() => expect(downloadMock).toHaveBeenCalled());
  });

  it("下载中 → 进度条 + 取消按钮", async () => {
    statusMock.mockResolvedValue(status({ phase: "downloading", done: 40, total: 400 }));
    renderTab();
    expect(await screen.findByTestId("settings-geo-bar")).toBeInTheDocument();
    expect(screen.getByTestId("settings-geo-pct").textContent).toBe("10%");
    fireEvent.click(screen.getByTestId("settings-geo-cancel"));
    await waitFor(() => expect(cancelMock).toHaveBeenCalled());
  });

  it("mapGeoProgress 事件驱动进度快进", async () => {
    let handler: ((e: AppEvent) => void) | null = null;
    subscribeMock.mockImplementation(async (cb) => {
      handler = cb;
      return () => {};
    });
    statusMock.mockResolvedValue(status({ phase: "downloading", done: 0, total: 400 }));
    renderTab();
    await screen.findByTestId("settings-geo-bar");
    handler!({ type: "mapGeoProgress", stage: "downloading", done: 200, total: 400, message: null });
    await waitFor(() => expect(screen.getByTestId("settings-geo-pct").textContent).toBe("50%"));
  });

  it("失败 → 错误信息 + 重试", async () => {
    statusMock.mockResolvedValue(status({ phase: "failed", message: "请求失败 https://x/710000_full.json: 404" }));
    renderTab();
    expect(await screen.findByTestId("settings-geo-failed")).toBeInTheDocument();
    expect(screen.getByTestId("settings-geo-failed-message").textContent).toContain("710000_full.json");
    fireEvent.click(screen.getByTestId("settings-geo-download"));
    await waitFor(() => expect(downloadMock).toHaveBeenCalled());
  });

  it("已安装 → 两步确认删除（mapGeoDelete）", async () => {
    renderTab();
    fireEvent.click(await screen.findByTestId("settings-geo-delete"));
    // 第一步只出确认，不触发删除
    expect(screen.getByTestId("settings-geo-delete-yes")).toBeInTheDocument();
    expect(deleteMock).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId("settings-geo-delete-yes"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalled());
  });

  it("中断可补全（datavFiles<250）→ 按钮显示补全/重试语义", async () => {
    statusMock.mockResolvedValue(status({ datavFiles: 120 }));
    renderTab();
    expect(await screen.findByTestId("settings-geo-summary")).toBeInTheDocument();
    expect(screen.getByTestId("settings-geo-state").getAttribute("data-phase")).toBe("ready");
  });
});
