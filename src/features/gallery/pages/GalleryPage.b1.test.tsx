import { assetFixture } from "@/test/fixtures";
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import GalleryPage from "./GalleryPage";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { clearGallerySnapshotForTests } from "../lib/galleryCache";
import { resetViewMarkForTests } from "../lib/viewMark";
import { SHORTCUTS_HINT_KEY } from "../components/ShortcutsHint";
import {
  assetGroupDates,
  assetLabelSet,
  lrStagingCreate,
  assetRejectSet,
  assetThumbGet,
  assetTrashMove,
  assetsPage,
  type AssetDto,
} from "@/ipc/api";

/**
 * 画廊选片补全（B1，页面级接线）：颜色标签三入口（操作条/右键/瓦片角标回显）、
 * 拒绝旗标（操作条/右键 + 瓦片弱化）、移入回收站（确认一步 + 乐观剔除）、
 * 反选。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    assetLabelSet: vi.fn(),
    assetRejectSet: vi.fn(),
    assetTrashMove: vi.fn(),
    lrStagingCreate: vi.fn(),
    revealInExplorer: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(),
}));

const assetsPageMock = vi.mocked(assetsPage);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const labelMock = vi.mocked(assetLabelSet);
const rejectMock = vi.mocked(assetRejectSet);
const trashMoveMock = vi.mocked(assetTrashMove);
const stagingMock = vi.mocked(lrStagingCreate);

function makeAsset(id: number, extra?: Partial<AssetDto>): AssetDto {
  return assetFixture(id, {
    capturedAt: `2026-09-1${id}T10:00:00`,
    ...extra,
  });
}


function tileOf(id: number): HTMLElement {
  const tile = screen.getAllByTestId("gallery-tile").find((t) => t.getAttribute("data-asset-id") === String(id));
  if (!tile) throw new Error(`tile ${id} not rendered`);
  return tile;
}

function checkOf(id: number): HTMLElement {
  return within(tileOf(id)).getByTestId("tile-check");
}

function renderGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/gallery" element={<GalleryPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
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
  assetsPageMock.mockReset().mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  labelMock.mockReset().mockResolvedValue(undefined);
  rejectMock.mockReset().mockResolvedValue(undefined);
  trashMoveMock.mockReset().mockResolvedValue(undefined);
  stagingMock.mockReset().mockResolvedValue({
    dir: "I:\\SmartPhoto\\主库\\LR\\0927-2",
    created: 2,
    hardlinked: 2,
    copied: 0,
  });
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  resetViewMarkForTests();
  localStorage.removeItem(SHORTCUTS_HINT_KEY);
});

// --- 颜色标签 ---------------------------------------------------------------------------

describe("画廊：颜色标签（B1）", () => {
  it("操作条色标：选中 2 张 → 弹出五色菜单 → 点红 → assetLabelSet([1,2],'red') + 瓦片色点回显", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(2));
    await user.click(screen.getByTestId("selection-color"));

    const menu = screen.getByTestId("selection-color-menu");
    expect(within(menu).getAllByTestId("selection-color-option")).toHaveLength(5);
    const red = within(menu)
      .getAllByTestId("selection-color-option")
      .find((el) => el.getAttribute("data-label") === "red");
    await user.click(red as HTMLElement);

    await waitFor(() => expect(labelMock).toHaveBeenCalledWith([1, 2], "red"));
    // 本地列表同步：两张瓦片出现色点
    await waitFor(() => {
      expect(within(tileOf(1)).getByTestId("tile-color-dot")).toHaveAttribute("data-label", "red");
      expect(within(tileOf(2)).getByTestId("tile-color-dot")).toHaveAttribute("data-label", "red");
    });
  });

  it("右键菜单「颜色标签」子项：展开五色+清除，作用于目标资产", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(3), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-color"));
    await screen.findByTestId("asset-context-menu-submenu-color");
    // 子项 = 五色 + 清除
    await screen.findByTestId("asset-context-menu-item-color-red");
    await screen.findByTestId("asset-context-menu-item-color-clear");
    await user.click(within(menu).getByTestId("asset-context-menu-item-color-blue"));
    await waitFor(() => expect(labelMock).toHaveBeenCalledWith([3], "blue"));
  });

  it("清除入口：操作条清除行 → assetLabelSet(ids, null)", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(2));
    await user.click(screen.getByTestId("selection-color"));
    await user.click(screen.getByTestId("selection-color-clear"));
    await waitFor(() => expect(labelMock).toHaveBeenCalledWith([2], null));
  });
});

// --- 拒绝旗标 ---------------------------------------------------------------------------

describe("画廊：拒绝旗标（B1）", () => {
  it("操作条拒绝：未拒绝选中集 → assetRejectSet(ids,true)；全部已拒绝 → 切换为取消", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(screen.getByTestId("selection-reject"));
    await waitFor(() => expect(rejectMock).toHaveBeenCalledWith([1], true));
    // 本地态更新：瓦片弱化 + 角标
    await waitFor(() => expect(tileOf(1)).toHaveAttribute("data-rejected", "true"));

    // 此时选中集全部已拒绝（draft 已回传）→ 按钮切换为「取消拒绝」
    const bar = screen.getByTestId("selection-bar");
    await waitFor(() => expect(within(bar).getByTestId("selection-reject")).toHaveAttribute("aria-pressed", "true"));
    await user.click(within(bar).getByTestId("selection-reject"));
    await waitFor(() => expect(rejectMock).toHaveBeenLastCalledWith([1], false));
  });

  it("右键菜单拒绝项：作用于目标资产", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(2), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-reject"));
    await waitFor(() => expect(rejectMock).toHaveBeenCalledWith([2], true));
  });
});

// --- 移入回收站 ---------------------------------------------------------------------------

describe("画廊：移入回收站（B1）", () => {
  it("操作条入口：确认弹窗列出 N 项 → 确认 → asset_trash_move + 列表剔除 + 退出多选", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(3));
    await user.click(screen.getByTestId("selection-trash"));

    const dialog = screen.getByTestId("trash-move-dialog");
    expect(dialog).toHaveTextContent("将所选 2 项移入回收站");
    await user.click(screen.getByTestId("trash-move-accept"));

    await waitFor(() => expect(trashMoveMock).toHaveBeenCalledWith([1, 3]));
    await waitFor(() => {
      const ids = screen.getAllByTestId("gallery-tile").map((t) => t.getAttribute("data-asset-id"));
      expect(ids).toEqual(["2"]);
    });
    expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument();
  });

  it("右键菜单入口（红色危险项）：单张 → 弹窗确认；取消不动列表", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(2), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-trash"));

    expect(screen.getByTestId("trash-move-dialog")).toBeInTheDocument();
    await user.click(screen.getByTestId("trash-move-cancel"));
    expect(trashMoveMock).not.toHaveBeenCalled();
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(3);
  });
});

// --- 反选 -------------------------------------------------------------------------------

describe("画廊：反选（B1）", () => {
  it("选中 1 张 → 反选 → 补集 2 张；再反选回 1 张", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(2));
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");

    await user.click(screen.getByTestId("selection-invert"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 2 张"));
    expect(checkOf(1)).toHaveAttribute("data-selected", "true");
    expect(checkOf(3)).toHaveAttribute("data-selected", "true");
    expect(checkOf(2)).toHaveAttribute("data-selected", "false");

    await user.click(screen.getByTestId("selection-invert"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张"));
    expect(checkOf(2)).toHaveAttribute("data-selected", "true");
  });
});

// --- LR 暂存夹入口（追加包） -------------------------------------------------------------

describe("画廊：生成 LR 暂存夹（追加包）", () => {
  it("操作条入口：选中集 → 命名弹窗（默认 MMDD-选中数）→ 确认调 lr_staging_create → 结果含路径与计数", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(3));
    await user.click(screen.getByTestId("selection-lr"));

    const dialog = await screen.findByTestId("lr-staging-dialog");
    const now = new Date();
    const pad = (n: number) => String(n).padStart(2, "0");
    const prefix = pad(now.getMonth() + 1) + pad(now.getDate());
    expect(within(dialog).getByTestId("lr-staging-name")).toHaveValue(prefix + "-2");
    await user.click(within(dialog).getByTestId("lr-staging-confirm"));

    await waitFor(() => expect(stagingMock).toHaveBeenCalledWith([1, 3], prefix + "-2"));
    const result = await screen.findByTestId("lr-staging-result");
    expect(within(result).getByTestId("lr-staging-counts")).toHaveTextContent("硬链接 2 张 / 复制 0 张");

    await user.click(within(result).getByTestId("lr-staging-done"));
    await waitFor(() => expect(screen.queryByTestId("lr-staging-dialog")).not.toBeInTheDocument());
  });

  it("右键菜单入口（单张）：弹窗打开；Esc 关闭", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(2), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-lr-staging"));

    expect(await screen.findByTestId("lr-staging-dialog")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("lr-staging-dialog")).not.toBeInTheDocument());
  });
});
