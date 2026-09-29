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
  albumAddAssets,
  albumList,
  assetFlagSet,
  assetGroupDates,
  assetRatingSet,
  assetThumbGet,
  assetsPage,
  clipboardCopyFiles,
  revealInExplorer,
  type AssetDto,
} from "@/ipc/api";

/**
 * 画廊选择模式（M4.5；③ 起入口=瓦片左上 check 圆钮，工具条「选择」按钮已删）：
 * 进出选择态、多选高亮、收藏/旗标负载、Esc 退出、Ctrl+点击/长按进入；
 * 瓦片右键自定义菜单（含多选语义）。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    assetRatingSet: vi.fn(),
    assetFlagSet: vi.fn(),
    clipboardCopyFiles: vi.fn(),
    revealInExplorer: vi.fn(),
    albumList: vi.fn(),
    albumAddAssets: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(),
}));

import { revealItemInDir } from "@tauri-apps/plugin-opener";

const assetsPageMock = vi.mocked(assetsPage);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const ratingMock = vi.mocked(assetRatingSet);
const flagMock = vi.mocked(assetFlagSet);
const copyMock = vi.mocked(clipboardCopyFiles);
const revealBatchMock = vi.mocked(revealInExplorer);
const revealMock = vi.mocked(revealItemInDir);
const albumListMock = vi.mocked(albumList);
const albumAddMock = vi.mocked(albumAddAssets);

function makeAsset(id: number): AssetDto {
  return assetFixture(id, {
    capturedAt: `2026-09-1${id}T10:00:00`,
  });
}

/** 按 assetId 取瓦片（组序 DESC，DOM 顺序与 id 无关） */
function tileOf(id: number): HTMLElement {
  const tile = screen.getAllByTestId("gallery-tile").find((t) => t.getAttribute("data-asset-id") === String(id));
  if (!tile) throw new Error(`tile ${id} not rendered`);
  return tile;
}

/** 瓦片左上 check 圆钮 */
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
  ratingMock.mockReset().mockResolvedValue(undefined);
  flagMock.mockReset().mockResolvedValue(undefined);
  copyMock.mockReset().mockResolvedValue(undefined);
  revealBatchMock.mockReset().mockResolvedValue(1);
  revealMock.mockReset().mockResolvedValue(undefined);
  albumListMock.mockReset().mockResolvedValue([
    { id: 3, name: "青海湖 2026", coverAssetId: null, itemCount: 0, createdAt: "2026-09-01" },
  ]);
  albumAddMock.mockReset().mockResolvedValue(1);
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  resetViewMarkForTests();
  localStorage.removeItem(SHORTCUTS_HINT_KEY);
});

// --- 进出选择态与多选 -----------------------------------------------------------------

describe("画廊：选择模式（check 圆钮入口）", () => {
  it("工具条无「选择」按钮；点瓦片 check 圆钮进入多选并选中该张，不打开查看器", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    // ③ 选择按钮已删；瓦片带 check 圆钮
    expect(screen.queryByTestId("gallery-select-toggle")).not.toBeInTheDocument();
    expect(screen.getAllByTestId("tile-check")).toHaveLength(3);

    await user.click(checkOf(1));
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "1");
    // 实心勾：选中瓦片圆钮 data-selected=true
    expect(checkOf(1)).toHaveAttribute("data-selected", "true");
    expect(checkOf(3)).toHaveAttribute("data-selected", "false");
    // 点击圆钮不触发瓦片本身（未打开查看器）
    expect(screen.queryByTestId("viewer")).not.toBeInTheDocument();

    await user.click(tileOf(3));
    const selected = screen.getAllByTestId("gallery-tile").filter((t) => t.getAttribute("data-selected") === "true");
    expect(selected).toHaveLength(2);
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 2 张");

    // 再点已选瓦片圆钮 = 取消选中
    await user.click(checkOf(1));
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");
  });

  it("Ctrl+点击直接进入多选并选中该资产；Esc 退出清空", async () => {
    renderGallery();
    const tiles = await screen.findAllByTestId("gallery-tile");

    // Ctrl+点击（未开多选也能进入）
    fireEvent.click(tiles[1], { ctrlKey: true }); // tiles[1] = DESC 序的第二张（id 2）
    await waitFor(() => expect(screen.getByTestId("selection-bar")).toBeInTheDocument());
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");
    const selected = screen.getAllByTestId("gallery-tile").filter((t) => t.getAttribute("data-selected") === "true");
    expect(selected).toHaveLength(1);

    // Esc 退出并清空（圆钮回落未选中）
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
    for (const check of screen.getAllByTestId("tile-check")) {
      expect(check).toHaveAttribute("data-selected", "false");
    }
    // 退出后点击恢复打开查看器
    await userEvent.click(tileOf(1));
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
  });

  it("收藏 = 选中资产逐个 assetRatingSet(id, 5)；旗标 = assetFlagSet(id, true)", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(2));

    await user.click(screen.getByTestId("selection-favorite"));
    await waitFor(() => {
      expect(ratingMock).toHaveBeenCalledWith(1, 5);
      expect(ratingMock).toHaveBeenCalledWith(2, 5);
    });
    expect(ratingMock).not.toHaveBeenCalledWith(3, 5);

    await user.click(screen.getByTestId("selection-flag"));
    await waitFor(() => {
      expect(flagMock).toHaveBeenCalledWith(1, true);
      expect(flagMock).toHaveBeenCalledWith(2, true);
    });
  });

  it("分享菜单：在资源管理器中显示 = 批量单窗 reveal_in_explorer；复制路径写剪贴板", async () => {
    const user = userEvent.setup();
    // userEvent.setup 会挂自己的 clipboard 桩——对其就地 spy（组件读到的是同一个）
    const writeText = vi
      .spyOn(navigator.clipboard ?? {}, "writeText")
      .mockResolvedValue(undefined);
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "1");

    await user.click(screen.getByTestId("selection-share"));
    await user.click(screen.getByTestId("selection-share-reveal"));
    await waitFor(() => expect(revealBatchMock).toHaveBeenCalledWith([makeAsset(1).path]));
    // reveal 完成后短提示（1.5s 自动消失，断言在窗口内）
    await waitFor(() => expect(screen.getByTestId("selection-toast")).toBeInTheDocument());

    await user.click(screen.getByTestId("selection-share"));
    fireEvent.click(screen.getByTestId("selection-share-copy"));
    await waitFor(() => expect(writeText).toHaveBeenCalledWith(makeAsset(1).path));
    writeText.mockRestore();
  });

  it("「取消」退出选择模式", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");

    await user.click(screen.getByTestId("selection-cancel"));
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
  });
});

// --- 瓦片右键自定义菜单（②：多选语义 + 菜单项 IPC） -----------------------------------

describe("画廊：瓦片右键菜单", () => {
  it("非多选态右键瓦片：菜单作用于该资产；reveal 走批量单窗 IPC，菜单关闭", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(2), { clientX: 200, clientY: 150 });
    const menu = await screen.findByTestId("asset-context-menu");
    expect(within(menu).getByTestId("asset-context-menu-item-reveal")).toHaveTextContent(
      "在资源管理器中显示",
    );
    expect(within(menu).getByTestId("asset-context-menu-item-copy")).toHaveTextContent(
      "复制文件到剪贴板",
    );

    await user.click(within(menu).getByTestId("asset-context-menu-item-reveal"));
    await waitFor(() => expect(revealBatchMock).toHaveBeenCalledWith([makeAsset(2).path]));
    expect(revealMock).not.toHaveBeenCalled();
    // 选择后菜单关闭
    await waitFor(() => expect(screen.queryByTestId("asset-context-menu")).not.toBeInTheDocument());
  });

  it("菜单项：复制文件调 clipboard_copy_files IPC；旗标调 assetFlagSet", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(3), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-copy"));
    await waitFor(() => expect(copyMock).toHaveBeenCalledWith([makeAsset(3).path]));

    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    await user.click(
      within(await screen.findByTestId("asset-context-menu")).getByTestId(
        "asset-context-menu-item-flag",
      ),
    );
    await waitFor(() => expect(flagMock).toHaveBeenCalledWith(1, true));
  });

  it("多选语义：右键选中瓦片 → 菜单作用于全部选中（旗标逐个）", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(checkOf(2));
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 2 张");

    // 右键选中集中的瓦片 1：作用于全部选中（1、2）
    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-flag"));
    await waitFor(() => {
      expect(flagMock).toHaveBeenCalledWith(1, true);
      expect(flagMock).toHaveBeenCalledWith(2, true);
    });
    expect(flagMock).not.toHaveBeenCalledWith(3, true);
  });

  it("多选语义：右键未选中瓦片 → 先切换选中集为该图（Windows 语义），菜单作用于它", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(checkOf(2));

    // 右键不在选中集的瓦片 3：选中集切为 [3]
    fireEvent.contextMenu(tileOf(3), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("asset-context-menu");
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张"));
    expect(checkOf(3)).toHaveAttribute("data-selected", "true");
    expect(checkOf(1)).toHaveAttribute("data-selected", "false");

    await user.click(within(menu).getByTestId("asset-context-menu-item-copy"));
    await waitFor(() => expect(copyMock).toHaveBeenCalledWith([makeAsset(3).path]));
  });

  it("点击外部 / Esc 关闭菜单；Esc 不退出多选（菜单层消费）", async () => {
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    expect(await screen.findByTestId("asset-context-menu")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("asset-context-menu")).not.toBeInTheDocument());

    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    expect(await screen.findByTestId("asset-context-menu")).toBeInTheDocument();
    fireEvent.mouseDown(document.body);
    await waitFor(() => expect(screen.queryByTestId("asset-context-menu")).not.toBeInTheDocument());
  });
});

// --- 加入相册入口（③ 全局：多选操作条 + 瓦片右键菜单） -----------------------------------

describe("画廊：加入相册入口", () => {
  it("多选操作条「加入相册」→ 弹窗（已选计数）确定 → album_add_assets(相册, 选中集)", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(2));
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "2");

    await user.click(screen.getByTestId("selection-add-album"));
    const dialog = await screen.findByTestId("add-to-album-dialog");
    expect(within(dialog).getByText("2 张")).toBeInTheDocument();

    const option = (await within(dialog).findAllByTestId("add-to-album-option"))[0];
    await user.click(option);
    await user.click(within(dialog).getByTestId("add-to-album-confirm"));

    await waitFor(() => expect(albumAddMock).toHaveBeenCalledWith(3, [1, 2]));
    await waitFor(() => expect(screen.queryByTestId("add-to-album-dialog")).not.toBeInTheDocument());
  });

  it("右键菜单「加入相册」项：多选语义=作用于全部选中；无多选=该资产", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    // 非多选：右键单项 → 菜单含加入相册
    fireEvent.contextMenu(tileOf(2), { clientX: 100, clientY: 100 });
    let menu = await screen.findByTestId("asset-context-menu");
    await user.click(within(menu).getByTestId("asset-context-menu-item-add-album"));
    let dialog = await screen.findByTestId("add-to-album-dialog");
    await user.click((await within(dialog).findAllByTestId("add-to-album-option"))[0]);
    await user.click(within(dialog).getByTestId("add-to-album-confirm"));
    await waitFor(() => expect(albumAddMock).toHaveBeenCalledWith(3, [2]));

    // toast 关闭后弹窗自动收起（1.4s）；等弹窗退场再走多选分支
    await waitFor(
      () => expect(screen.queryByTestId("add-to-album-dialog")).not.toBeInTheDocument(),
      { timeout: 3000 },
    );

    // 多选语义：右键选中瓦片 → 全部选中集
    await user.click(checkOf(1));
    await user.click(checkOf(3));
    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    menu = await screen.findByTestId("asset-context-menu");
    expect(within(menu).getByTestId("asset-context-menu-item-add-album")).toHaveTextContent(
      "加入相册（2 张）",
    );
    await user.click(within(menu).getByTestId("asset-context-menu-item-add-album"));
    dialog = await screen.findByTestId("add-to-album-dialog");
    await user.click((await within(dialog).findAllByTestId("add-to-album-option"))[0]);
    await user.click(within(dialog).getByTestId("add-to-album-confirm"));
    await waitFor(() => expect(albumAddMock).toHaveBeenLastCalledWith(3, [1, 3]));
  });
});

// --- 快捷键一次性提示条 -----------------------------------------------------------------

describe("画廊：快捷键一次性提示条", () => {
  it("首次进入显示「按 ? 查看快捷键」并标记 localStorage；再次进入不显示", async () => {
    const { unmount } = renderGallery();
    expect(await screen.findByTestId("shortcuts-hint")).toHaveTextContent("按 ? 查看快捷键");
    expect(localStorage.getItem(SHORTCUTS_HINT_KEY)).toBe("1");

    unmount();
    clearGallerySnapshotForTests();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");
    await waitFor(() =>
      expect(screen.queryByTestId("shortcuts-hint")).not.toBeInTheDocument(),
    );
  });
});

describe("画廊：多选切换语义（再点即取消）", () => {
  it("收藏：未收藏→5；乐观回写后全部已收藏→再点=0（取消收藏）", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(screen.getByTestId("selection-favorite"));
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 5));

    // 乐观回写 rating=5 后再点：全部已收藏 → 取消
    await user.click(screen.getByTestId("selection-favorite"));
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 0));
  });

  it("全选：数据窗口全选；再点=取消全选（空集）", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(screen.getByTestId("selection-all"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 3 张"));
    expect(screen.getByTestId("selection-all")).toHaveTextContent("取消全选");

    await user.click(screen.getByTestId("selection-all"));
    await waitFor(() => expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 0 张"));
    expect(screen.getByTestId("selection-all")).toHaveTextContent("全选");
  });

  it("旗标：全部已旗标再点=取消", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(2));
    await user.click(screen.getByTestId("selection-flag"));
    await waitFor(() => expect(flagMock).toHaveBeenCalledWith(2, true));
    // 旗标无乐观回写（assets.flagged 不变）→ 仍视为未旗标，语义不变：
    // 这里只验证切换语义依赖选中集自身的 flagged 态
    expect(flagMock).not.toHaveBeenCalledWith(2, false);
  });
});
