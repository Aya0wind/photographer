import { assetFixture } from "@/test/fixtures";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import TrashPage from "./TrashPage";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import {
  assetThumbGet,
  trashList,
  trashPurge,
  trashRestore,
  type AssetDto,
} from "@/ipc/api";

/**
 * 回收站页（B1）：trash_list 列表渲染 / 空态 / 多选（复用网格选择机制）/
 * 批量恢复 / 彻底删除两档确认弹窗（保留文件 vs 同时删除文件 + 不可恢复提示）。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    trashList: vi.fn(),
    trashRestore: vi.fn(),
    trashPurge: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

const listMock = vi.mocked(trashList);
const restoreMock = vi.mocked(trashRestore);
const purgeMock = vi.mocked(trashPurge);
const thumbMock = vi.mocked(assetThumbGet);

function makeAsset(id: number): AssetDto {
  return assetFixture(id, {
    capturedAt: `2026-09-1${id % 10}T10:00:00`,
    inTrash: true,
  });
}

function renderTrash() {
  return render(
    <I18nextProvider i18n={i18n}>
      <TrashPage />
    </I18nextProvider>,
  );
}

function tileOf(id: number): HTMLElement {
  const tile = screen.getAllByTestId("gallery-tile").find((t) => t.getAttribute("data-asset-id") === String(id));
  if (!tile) throw new Error(`tile ${id} not rendered`);
  return tile;
}

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
  listMock.mockReset().mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
  restoreMock.mockReset().mockResolvedValue(true);
  purgeMock.mockReset().mockResolvedValue(0);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
});

describe("回收站页：列表与空态", () => {
  it("trash_list(0, 200) 拉取并以网格展示（只读：无收藏星钮）", async () => {
    renderTrash();

    expect(listMock).toHaveBeenCalledWith(0, 200);
    await screen.findByTestId("trash-page");
    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);
    expect(screen.queryByTestId("tile-favorite")).not.toBeInTheDocument();
    expect(screen.queryByTestId("trash-empty")).not.toBeInTheDocument();
  });

  it("空回收站：空态文案（可恢复提示）", async () => {
    listMock.mockResolvedValue([]);
    renderTrash();

    const empty = await screen.findByTestId("trash-empty");
    expect(empty).toHaveTextContent("回收站是空的");
    expect(listMock).toHaveBeenCalledOnce();
  });
});

describe("回收站页：多选与批量操作", () => {
  it("点瓦片进入多选；操作条显示已选数；取消清空", async () => {
    const user = userEvent.setup();
    renderTrash();
    await screen.findAllByTestId("gallery-tile");

    await user.click(within(tileOf(1)).getByTestId("tile-check"));
    await user.click(tileOf(2));
    expect(screen.getByTestId("trash-actions")).toHaveAttribute("data-count", "2");
    expect(screen.getByTestId("trash-selected-count")).toHaveTextContent("已选 2 项");

    await user.click(screen.getByTestId("trash-cancel"));
    await waitFor(() => expect(screen.queryByTestId("trash-actions")).not.toBeInTheDocument());
  });

  it("恢复：trash_restore(选中集) → 列表本地剔除 + toast；恢复失败提示", async () => {
    const user = userEvent.setup();
    renderTrash();
    await screen.findAllByTestId("gallery-tile");

    await user.click(within(tileOf(1)).getByTestId("tile-check"));
    await user.click(within(tileOf(3)).getByTestId("tile-check"));
    await user.click(screen.getByTestId("trash-restore"));

    await waitFor(() => expect(restoreMock).toHaveBeenCalledWith([1, 3]));
    await waitFor(() => {
      const remain = screen.getAllByTestId("gallery-tile").map((t) => t.getAttribute("data-asset-id"));
      expect(remain).toEqual(["2"]);
    });
    expect(screen.getByTestId("trash-toast")).toHaveTextContent("已恢复 2 项");

    // 失败分支：不剔除
    restoreMock.mockResolvedValue(false);
    await user.click(within(tileOf(2)).getByTestId("tile-check"));
    await user.click(screen.getByTestId("trash-restore"));
    await waitFor(() => expect(screen.getByTestId("trash-toast")).toHaveTextContent("恢复失败"));
    expect(screen.getByTestId("gallery-tile")).toHaveAttribute("data-asset-id", "2");
  });

  it("彻底删除：弹窗列出 N 项 + 不可恢复提示；「保留文件」→ trash_purge(ids,false)", async () => {
    const user = userEvent.setup();
    purgeMock.mockResolvedValue(1);
    renderTrash();
    await screen.findAllByTestId("gallery-tile");

    await user.click(within(tileOf(1)).getByTestId("tile-check"));
    await user.click(within(tileOf(2)).getByTestId("tile-check"));
    await user.click(screen.getByTestId("trash-purge-open"));

    const dialog = screen.getByTestId("trash-purge-dialog");
    expect(screen.getByTestId("trash-purge-title")).toHaveTextContent("彻底删除 2 项");
    expect(dialog).toHaveTextContent("将从库记录中移除所选 2 项");
    expect(dialog).toHaveTextContent("彻底删除不可恢复");

    await user.click(screen.getByTestId("trash-purge-keep"));
    await waitFor(() => expect(purgeMock).toHaveBeenCalledWith([1, 2], false));
    await waitFor(() => {
      expect(screen.getByTestId("gallery-tile")).toHaveAttribute("data-asset-id", "3");
    });
    expect(screen.getByTestId("trash-toast")).toHaveTextContent("已从库中移除 2 项");
    expect(screen.queryByTestId("trash-purge-dialog")).not.toBeInTheDocument();
  });

  it("两档：「同时删除文件」→ trash_purge(ids,true)；取消关闭弹窗不动列表", async () => {
    const user = userEvent.setup();
    renderTrash();
    await screen.findAllByTestId("gallery-tile");

    await user.click(within(tileOf(2)).getByTestId("tile-check"));
    await user.click(screen.getByTestId("trash-purge-open"));
    await user.click(screen.getByTestId("trash-purge-cancel"));
    expect(screen.queryByTestId("trash-purge-dialog")).not.toBeInTheDocument();
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(3);

    await user.click(screen.getByTestId("trash-purge-open"));
    await user.click(screen.getByTestId("trash-purge-files"));
    await waitFor(() => expect(purgeMock).toHaveBeenCalledWith([2], true));
    await waitFor(() => {
      const ids = screen.getAllByTestId("gallery-tile").map((t) => t.getAttribute("data-asset-id"));
      expect(ids).not.toContain("2");
    });
    expect(screen.getByTestId("trash-toast")).toHaveTextContent("已彻底删除 1 项");
  });
});

describe("回收站页：全选/反选", () => {
  it("全选 → 已选=全部（按钮变取消全选）；再点 → 清空；反选取补集", async () => {
    listMock.mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
    renderTrash();
    const tiles = await screen.findAllByTestId("gallery-tile");

    // 进多选（点一张）
    fireEvent.click(tiles[0]);
    expect(await screen.findByTestId("trash-actions")).toHaveAttribute("data-count", "1");

    // 全选：3 张 + 按钮变「取消全选」
    fireEvent.click(screen.getByTestId("trash-select-all"));
    await waitFor(() => expect(screen.getByTestId("trash-actions")).toHaveAttribute("data-count", "3"));
    expect(screen.getByTestId("trash-select-all")).toHaveTextContent("取消全选");

    // 反选：补集 = 空集
    fireEvent.click(screen.getByTestId("trash-invert"));
    await waitFor(() => expect(screen.queryByTestId("trash-actions")).not.toBeInTheDocument());

    // 再进多选后全选 → 取消全选回空
    fireEvent.click(screen.getAllByTestId("gallery-tile")[1]);
    fireEvent.click(screen.getByTestId("trash-select-all"));
    await waitFor(() => expect(screen.getByTestId("trash-actions")).toHaveAttribute("data-count", "3"));
    fireEvent.click(screen.getByTestId("trash-select-all"));
    await waitFor(() => expect(screen.queryByTestId("trash-actions")).not.toBeInTheDocument());
  });
});
