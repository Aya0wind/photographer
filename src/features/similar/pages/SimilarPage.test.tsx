import { assetFixture } from "@/test/fixtures";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import SimilarPage from "./SimilarPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  assetThumbGet,
  duplicateDelete,
  duplicatesList,
  type AssetDto,
  type DuplicateGroupDto,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    duplicatesList: vi.fn(),
    duplicateDelete: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const listMock = vi.mocked(duplicatesList);
const deleteMock = vi.mocked(duplicateDelete);
const thumbMock = vi.mocked(assetThumbGet);

// --- 工具 ---------------------------------------------------------------------------

function makeAsset(id: number): AssetDto {
  const file = `IMG_${String(id).padStart(4, "0")}.JPG`;
  return assetFixture(id, {
    path: `Y:\\照片\\SmartPhoto\\2026\\${file}`,
    name: file,
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 1024,
  });
}

function groupOf(ids: number[], kind: "exact" | "similar" = "similar"): DuplicateGroupDto {
  return { kind, assets: ids.map(makeAsset) };
}

function renderSimilar() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/similar"]}>
        <Routes>
          <Route path="/similar" element={<SimilarPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  listMock.mockReset().mockResolvedValue([]);
  deleteMock.mockReset().mockResolvedValue(0);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
});

// --- Tab 与组渲染 -------------------------------------------------------------------

describe("相似照片：Tab 与组渲染", () => {
  it("只展示近似组，不提供完全重复入口", async () => {
    listMock.mockResolvedValue([{ kind: "similar", assets: [makeAsset(1), makeAsset(2)] }]);
    renderSimilar();
    const card = (await screen.findAllByTestId("similar-group"))[0];
    expect(card).toHaveTextContent("近似");
    expect(screen.queryByTestId("similar-tab-exact")).not.toBeInTheDocument();
    expect(listMock).toHaveBeenCalledWith("similar", 0, 20);
  });

  it("没有近似照片时展示空态", async () => {
    renderSimilar();
    expect(await screen.findByTestId("similar-empty")).toHaveTextContent("没有相似的照片");
  });
});

// --- 勾选 + 删除确认流 ----------------------------------------------------------------

describe("相似照片：勾选删除", () => {
  it("点选组内照片 → 删除所选（按钮带计数）→ 二次确认 → duplicateDelete → 组内剔除 + 已删除反馈", async () => {
    listMock.mockResolvedValue([
      { kind: "similar", assets: [makeAsset(1), makeAsset(2), makeAsset(3)] },
    ]);
    deleteMock.mockResolvedValue(2);
    const user = userEvent.setup();
    renderSimilar();

    const card = (await screen.findAllByTestId("similar-group"))[0];
    const deleteButton = within(card).getByTestId("similar-group-delete");
    expect(deleteButton).toBeDisabled();

    // 勾选两张
    const tiles = within(card).getAllByTestId("similar-asset");
    await user.click(within(tiles[1]).getByTestId("similar-asset-check"));
    await user.click(within(tiles[2]).getByTestId("similar-asset-check"));
    expect(tiles[1]).toHaveAttribute("data-selected", "true");
    expect(tiles[0]).not.toHaveAttribute("data-selected");
    expect(deleteButton).toHaveTextContent("删除所选（2）");

    // 确认弹窗（红色二次确认）→ 取消不发
    await user.click(deleteButton);
    const dialog = screen.getByTestId("similar-confirm");
    expect(dialog).toHaveTextContent("将永久删除所选 2 张");
    await user.click(screen.getByTestId("similar-confirm-cancel"));
    expect(deleteMock).not.toHaveBeenCalled();

    // 确认路径：ids 传选中两张；组内剔除后剩 1 张（组卡移除）；行内反馈
    await user.click(within(card).getByTestId("similar-group-delete"));
    await user.click(screen.getByTestId("similar-confirm-yes"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith([2, 3]));
    expect(await screen.findByTestId("similar-feedback")).toHaveTextContent("已删除 2 张");
    await waitFor(() => expect(screen.queryByTestId("similar-group")).not.toBeInTheDocument());
  });

  it("删除失败（后端 Err 文案透传），组不变", async () => {
    listMock.mockResolvedValue([{ kind: "similar", assets: [makeAsset(1), makeAsset(2)] }]);
    deleteMock.mockRejectedValue("未选择活动库");
    const user = userEvent.setup();
    renderSimilar();

    const card = (await screen.findAllByTestId("similar-group"))[0];
    await user.click(within(card).getAllByTestId("similar-asset-check")[0]);
    await user.click(within(card).getByTestId("similar-group-delete"));
    await user.click(screen.getByTestId("similar-confirm-yes"));

    expect(await screen.findByTestId("similar-error")).toHaveTextContent("未选择活动库");
    expect(screen.getByTestId("similar-group")).toBeInTheDocument();
  });
});

// --- 分页游标 ------------------------------------------------------------------------

describe("相似照片：分页游标", () => {
  it("满页出现「加载更多」；after=已取组数（0 基组偏移）；短页到底按钮消失", async () => {
    const page1 = Array.from({ length: 20 }, (_, i) => groupOf([100 + i * 2, 101 + i * 2]));
    const page2 = Array.from({ length: 3 }, (_, i) => groupOf([200 + i * 2, 201 + i * 2]));
    listMock.mockResolvedValueOnce(page1).mockResolvedValueOnce(page2);
    const user = userEvent.setup();
    renderSimilar();

    expect(await screen.findAllByTestId("similar-group")).toHaveLength(20);
    await user.click(screen.getByTestId("similar-load-more"));

    await waitFor(() => expect(screen.getAllByTestId("similar-group")).toHaveLength(23));
    // 游标 = 首页组数（skip 计数，非组 id）
    expect(listMock).toHaveBeenLastCalledWith("similar", 20, 20);
    // 短页（3 < 20）→ 到底
    expect(screen.queryByTestId("similar-load-more")).not.toBeInTheDocument();
  });

  it("删除使整组消失后游标同步收缩：加载更多 after=19（不跳组）", async () => {
    const page1 = Array.from({ length: 20 }, (_, i) => groupOf([100 + i * 2, 101 + i * 2]));
    listMock.mockResolvedValueOnce(page1).mockResolvedValueOnce([groupOf([300, 301])]);
    deleteMock.mockResolvedValue(2);
    const user = userEvent.setup();
    renderSimilar();

    expect(await screen.findAllByTestId("similar-group")).toHaveLength(20);

    // 删除第一组全部两张 → 组卡移除，seen 20→19
    const first = screen.getAllByTestId("similar-group")[0];
    for (const tile of within(first).getAllByTestId("similar-asset")) {
      await user.click(within(tile).getByTestId("similar-asset-check"));
    }
    await user.click(within(first).getByTestId("similar-group-delete"));
    await user.click(screen.getByTestId("similar-confirm-yes"));
    await waitFor(() => expect(screen.getAllByTestId("similar-group")).toHaveLength(19));

    await user.click(screen.getByTestId("similar-load-more"));
    await waitFor(() => expect(listMock).toHaveBeenLastCalledWith("similar", 19, 20));
    await waitFor(() => expect(screen.getAllByTestId("similar-group")).toHaveLength(20));
  });
});

it("图片点击打开预览，勾选删除按钮独立操作", async () => {
  listMock.mockResolvedValue([groupOf([1, 2])]);
  const user = userEvent.setup();
  renderSimilar();
  const cards = await screen.findAllByTestId("similar-asset");
  await user.click(within(cards[0]).getByTestId("similar-asset-preview"));
  expect(await screen.findByTestId("viewer")).toBeInTheDocument();
  expect(cards[0]).not.toHaveAttribute("data-selected");
  await user.keyboard("{Escape}");
  await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
  await user.click(within(cards[0]).getByTestId("similar-asset-check"));
  expect(cards[0]).toHaveAttribute("data-selected", "true");
  expect(screen.queryByTestId("viewer")).not.toBeInTheDocument();
});

describe("相似照片：拖拽划选", () => {
  it("勾选钮按下划过其他瓦片 = 连续选中；已选瓦片按下划过 = 连续取消", async () => {
    listMock.mockResolvedValue([{ kind: "similar", assets: [makeAsset(1), makeAsset(2), makeAsset(3)] }]);
    renderSimilar();
    const cards = await screen.findAllByTestId("similar-group");
    const tiles = within(cards[0]).getAllByTestId("similar-asset");

    // 从第一格勾选钮按下 → 划过第二、三格 → 三格全选
    fireEvent.pointerDown(within(tiles[0]).getByTestId("similar-asset-check"), { button: 0 });
    fireEvent.pointerEnter(tiles[1]);
    fireEvent.pointerEnter(tiles[2]);
    fireEvent.pointerUp(window);
    for (const tile of tiles.slice(0, 3)) {
      expect(tile).toHaveAttribute("data-selected", "true");
    }

    // 已选瓦片上按下（取消意图）→ 划过另一格 → 两格取消
    fireEvent.pointerDown(tiles[0], { button: 0 });
    fireEvent.pointerEnter(tiles[1]);
    fireEvent.pointerUp(window);
    expect(tiles[0]).not.toHaveAttribute("data-selected");
    expect(tiles[1]).not.toHaveAttribute("data-selected");
    expect(tiles[2]).toHaveAttribute("data-selected", "true");
  });
});
