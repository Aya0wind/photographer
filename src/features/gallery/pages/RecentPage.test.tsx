import { assetFixture } from "@/test/fixtures";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import RecentPage from "./RecentPage";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { resetViewMarkForTests } from "../lib/viewMark";
import { assetThumbGet, assetViewMark, recentViewed, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    recentViewed: vi.fn(),
    assetViewMark: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const recentViewedMock = vi.mocked(recentViewed);
const markMock = vi.mocked(assetViewMark);
const thumbMock = vi.mocked(assetThumbGet);

// --- 工具 ---------------------------------------------------------------------------

function makeAsset(id: number): AssetDto {
  return assetFixture(id, {
    capturedAt: "2026-09-18T10:00:00",
  });
}

function renderRecent() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/recent"]}>
        <Routes>
          <Route path="/recent" element={<RecentPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  recentViewedMock.mockReset().mockResolvedValue([]);
  markMock.mockReset().mockResolvedValue(undefined);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
  resetViewMarkForTests();
});

// --- 首屏 ---------------------------------------------------------------------------

describe("最近浏览：首屏", () => {
  it("进入页面拉 recentViewed(200)（最后浏览时间 DESC），渲染 square 网格", async () => {
    recentViewedMock.mockResolvedValue([makeAsset(5), makeAsset(4)]);
    renderRecent();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    expect(tiles.map((t) => t.getAttribute("data-asset-id"))).toEqual(["5", "4"]);
    expect(recentViewedMock).toHaveBeenCalledWith(200);
    expect(screen.getByTestId("recent-grid-scroll")).toHaveAttribute("data-layout", "square");
  });

  it("无浏览记录 / 后端未就绪（空回退）→ 空态「打开过的照片会出现在这里」", async () => {
    recentViewedMock.mockResolvedValue([]);
    renderRecent();

    const empty = await screen.findByTestId("recent-empty");
    expect(empty).toHaveTextContent("还没有浏览记录");
    expect(empty).toHaveTextContent("打开过的照片会出现在这里");
    expect(screen.queryByTestId("gallery-tile")).not.toBeInTheDocument();
  });
});

// --- 浏览打点闭环 ---------------------------------------------------------------------

describe("最近浏览：查看器打点闭环", () => {
  it("点击瓦片打开查看器 → asset_view_mark；30s 内重开不重复打点；切图同样打点", async () => {
    recentViewedMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    const user = userEvent.setup();
    renderRecent();

    const tiles = await screen.findAllByTestId("gallery-tile");
    await user.click(tiles[0]);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    await waitFor(() => expect(markMock).toHaveBeenCalledWith(1));

    // 关闭后 30s 内重开同一张：去抖，不重复打点
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
    await user.click(screen.getAllByTestId("gallery-tile")[0]);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    expect(markMock.mock.calls.filter(([id]) => id === 1)).toHaveLength(1);

    // 切到下一张：另一资产打点（切图也算浏览）
    await user.click(screen.getByTestId("viewer-next"));
    await waitFor(() => expect(markMock).toHaveBeenCalledWith(2));
  });
});

describe("最近浏览：多选", () => {
  it("Ctrl+点击进多选 → 操作条（收藏/加册/全选/回收站）；全选计数与取消全选", async () => {
    recentViewedMock.mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
    renderRecent();
    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);

    fireEvent.click(tiles[0], { ctrlKey: true });
    expect(await screen.findByTestId("selection-bar")).toBeInTheDocument();
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");

    // 全选 → 3 张；再点 → 取消全选（切换语义）
    fireEvent.click(screen.getByTestId("selection-all"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 3 张"));
    expect(screen.getByTestId("selection-all")).toHaveTextContent("取消全选");
    fireEvent.click(screen.getByTestId("selection-all"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 0 张"));

    fireEvent.click(screen.getByTestId("selection-cancel"));
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
  });
});
