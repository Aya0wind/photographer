import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import CullingPage from "./CullingPage";
import { useCullingStore } from "../cullingStore";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  albumList,
  assetDetail,
  assetsByIds,
  cullDecisionApply,
  cullSessionDiscard,
  cullSessionFinish,
  cullSessionList,
  cullSessionOpen,
  cullSessionRename,
  type CullSessionDto,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumList: vi.fn(),
    cullSessionList: vi.fn(),
    cullSessionOpen: vi.fn(),
    cullSessionRename: vi.fn(),
    cullSessionDiscard: vi.fn(),
    cullSessionFinish: vi.fn(),
    cullDecisionApply: vi.fn(),
    assetThumbGet: vi.fn(),
    assetDetail: vi.fn(),
    assetsByIds: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
  isTauri: vi.fn(() => false),
}));

const listMock = vi.mocked(cullSessionList);
const openMock = vi.mocked(cullSessionOpen);
const renameMock = vi.mocked(cullSessionRename);
const discardMock = vi.mocked(cullSessionDiscard);
const finishMock = vi.mocked(cullSessionFinish);
const albumListMock = vi.mocked(albumList);
const assetsByIdsMock = vi.mocked(assetsByIds);
const detailMock = vi.mocked(assetDetail);

function session(id: number, overrides: Partial<CullSessionDto> = {}): CullSessionDto {
  return {
    id,
    name: `会话 ${id}`,
    scope: { kind: "album", albumId: 3, subgroup: null },
    total: 10,
    accepted: 4,
    rejected: 3,
    undecided: 3,
    createdAt: "2026-09-28T10:00:00Z",
    updatedAt: "2026-09-28T10:00:00Z",
    finishedAt: null,
    ...overrides,
  };
}

beforeEach(() => {
  resetThumbPipelineForTests();
  listMock.mockReset().mockResolvedValue([]);
  openMock.mockReset().mockResolvedValue(null);
  renameMock.mockReset().mockResolvedValue(true);
  discardMock.mockReset().mockResolvedValue(true);
  finishMock.mockReset().mockResolvedValue({ appliedFlag: 4, appliedRating: 0, rejected: 3 });
  albumListMock.mockReset().mockResolvedValue([{ id: 3, name: "婚礼", coverAssetId: null, itemCount: 10, createdAt: "2026-01-01T00:00:00Z" }]);
  assetsByIdsMock.mockReset().mockResolvedValue([]);
  detailMock.mockReset().mockResolvedValue(null);
  vi.mocked(cullDecisionApply).mockReset().mockResolvedValue(session(1));
  useCullingStore.setState({ activeCount: 0 });
});

function renderPage(initialEntries: Array<string | { pathname: string; state: unknown }> = ["/culling"]) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={initialEntries as never}>
        <CullingPage />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

describe("选片会话页（CullingPage /culling）", () => {
  it("空态：新建引导（相册/画廊筛选入口）", async () => {
    renderPage();
    await screen.findByTestId("culling-empty");
    expect(screen.getByTestId("culling-empty").textContent).toContain("还没有选片会话");
  });

  it("进行中卡片：名称/来源描述/进度计数/操作；侧栏徽标计数同步", async () => {
    listMock.mockResolvedValue([
      session(1),
      session(2, { scope: { kind: "query", assetIds: [7, 8, 9] } }),
    ]);
    renderPage();

    const cards = await screen.findAllByTestId("culling-session-card");
    const card1 = cards[0];
    expect(within(card1).getByTestId("culling-session-name")).toHaveTextContent("会话 1");
    expect(within(card1).getByTestId("culling-session-scope")).toHaveTextContent("相册 · 婚礼");
    const progress = within(card1).getByTestId("culling-session-progress");
    expect(progress).toHaveAttribute("data-accepted", "4");
    expect(progress).toHaveAttribute("data-rejected", "3");
    expect(progress).toHaveAttribute("data-undecided", "3");

    const card2 = screen.getAllByTestId("culling-session-card")[1];
    expect(within(card2).getByTestId("culling-session-scope")).toHaveTextContent("筛选结果快照 · 3 张");

    await waitFor(() => {
      expect(useCullingStore.getState().activeCount).toBe(2);
    });
  });

  it("继续选 → 挂载全屏选片层；Esc 关闭回列表并刷新", async () => {
    listMock.mockResolvedValue([session(1)]);
    openMock.mockResolvedValue({
      session: session(1),
      items: [{ assetId: 11, decision: null, origin: "manual" }],
    });
    renderPage();

    await userEvent.click(await screen.findByTestId("culling-session-continue"));
    await screen.findByTestId("culling-overlay");
    expect(openMock).toHaveBeenCalledWith(1);

    fireEvent.keyDown(document.body, { key: "Escape" });
    await waitFor(() => {
      expect(screen.queryByTestId("culling-overlay")).not.toBeInTheDocument();
    });
    // 关闭=保存：重拉清单
    await waitFor(() => {
      expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2);
    });
  });

  it("改名：cullSessionRename + 本地更新", async () => {
    listMock.mockResolvedValue([session(1)]);
    renderPage();

    await userEvent.click(await screen.findByTestId("culling-session-rename"));
    const input = await screen.findByTestId("culling-rename-input");
    await userEvent.clear(input);
    await userEvent.type(input, "复选");
    await userEvent.click(screen.getByTestId("culling-rename-confirm"));

    await waitFor(() => {
      expect(renameMock).toHaveBeenCalledWith(1, "复选");
    });
    await waitFor(() => {
      expect(screen.getByTestId("culling-session-name")).toHaveTextContent("复选");
    });
  });

  it("删除：确认后 cullSessionDiscard + 卡片消失 + 徽标刷新", async () => {
    let live = [session(1)];
    listMock.mockImplementation(async () => live);
    renderPage();

    await userEvent.click(await screen.findByTestId("culling-session-delete"));
    discardMock.mockImplementation(async () => {
      live = [];
      return true;
    });
    await userEvent.click(await screen.findByTestId("culling-delete-confirm"));

    await waitFor(() => {
      expect(discardMock).toHaveBeenCalledWith(1);
    });
    await waitFor(() => {
      expect(screen.queryByTestId("culling-session-card")).not.toBeInTheDocument();
    });
    await waitFor(() => {
      expect(useCullingStore.getState().activeCount).toBe(0);
    });
  });

  it("已完成折叠区：默认收起，展开显示摘要行（无续选）", async () => {
    listMock.mockResolvedValue([
      session(5, { finishedAt: "2026-09-28T12:00:00Z", accepted: 6, rejected: 2, undecided: 2 }),
    ]);
    renderPage();

    // 只出现在折叠区，进行中区为空态
    await screen.findByTestId("culling-finished-section");
    expect(screen.getByTestId("culling-empty")).toBeInTheDocument();
    expect(screen.queryByTestId("culling-finished-row")).not.toBeInTheDocument();

    await userEvent.click(screen.getByTestId("culling-finished-toggle"));
    const row = await screen.findByTestId("culling-finished-row");
    expect(within(row).getByTestId("culling-finished-summary")).toHaveAttribute("data-accepted", "6");
  });

  it("入口跳转协议：state.open 直接打开过片层并清路由态", async () => {
    listMock.mockResolvedValue([session(1)]);
    openMock.mockResolvedValue({
      session: session(1),
      items: [{ assetId: 11, decision: null, origin: "manual" }],
    });
    renderPage([{ pathname: "/culling", state: { open: 1 } }]);

    await screen.findByTestId("culling-overlay");
    expect(openMock).toHaveBeenCalledWith(1);
  });

  it("收尾完成：摘要 toast + 会话清单刷新", async () => {
    listMock.mockResolvedValue([session(1)]);
    openMock.mockResolvedValue({
      session: session(1),
      items: [{ assetId: 11, decision: null, origin: "manual" }],
    });
    renderPage();

    await userEvent.click(await screen.findByTestId("culling-session-continue"));
    await screen.findByTestId("culling-overlay");
    await userEvent.click(screen.getByTestId("culling-finish"));
    await userEvent.click(await screen.findByTestId("cull-finish-confirm"));

    await screen.findByTestId("culling-toast");
    expect(screen.getByTestId("culling-toast")).toHaveAttribute("data-kind", "finished");
    expect(screen.getByTestId("culling-toast").textContent).toContain("旗标 4");
    await waitFor(() => {
      expect(screen.queryByTestId("culling-overlay")).not.toBeInTheDocument();
    });
  });
});
