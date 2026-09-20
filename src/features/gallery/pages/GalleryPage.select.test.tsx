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
  assetFlagSet,
  assetGroupDates,
  assetRatingSet,
  assetThumbGet,
  assetsPage,
  type AssetDto,
} from "@/ipc/api";

/**
 * 画廊选择模式（M4.5）：进出选择态、多选计数/序号角标、收藏/旗标负载、Esc 退出、
 * Ctrl+点击进入；快捷键一次性提示条。
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
const revealMock = vi.mocked(revealItemInDir);

function makeAsset(id: number): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: `2026-09-1${id}T10:00:00`,
    camera: null,
    sizeBytes: 1,
  };
}

/** 按 assetId 取瓦片（组序 DESC，DOM 顺序与 id 无关） */
function tileOf(id: number): HTMLElement {
  const tile = screen.getAllByTestId("gallery-tile").find((t) => t.getAttribute("data-asset-id") === String(id));
  if (!tile) throw new Error(`tile ${id} not rendered`);
  return tile;
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
  revealMock.mockReset().mockResolvedValue(undefined);
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  resetViewMarkForTests();
  localStorage.removeItem(SHORTCUTS_HINT_KEY);
});

// --- 进出选择态与多选 -----------------------------------------------------------------

describe("画廊：选择模式", () => {
  it("「选择」按钮进入；点击瓦片=切换选中（高亮+序号角标），不打开查看器", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(screen.getByTestId("gallery-select-toggle"));
    expect(screen.getByTestId("gallery-select-toggle")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "0");

    await user.click(tileOf(1));
    await user.click(tileOf(3));

    const selected = screen.getAllByTestId("gallery-tile").filter((t) => t.getAttribute("data-selected") === "true");
    expect(selected).toHaveLength(2);
    // 序号角标（选中序：先点 id1=1，再点 id3=2；DOM 序与 id 无关按 tile 断言）
    expect(within(tileOf(1)).getByTestId("gallery-tile-select-badge")).toHaveTextContent("1");
    expect(within(tileOf(3)).getByTestId("gallery-tile-select-badge")).toHaveTextContent("2");
    // 操作条计数
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 2 张");
    // 不打开查看器
    expect(screen.queryByTestId("viewer")).not.toBeInTheDocument();

    // 再点已选瓦片 = 取消选中
    await user.click(tileOf(1));
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

    // Esc 退出并清空
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
    expect(screen.queryByTestId("gallery-tile-select-badge")).not.toBeInTheDocument();
    // 退出后点击恢复打开查看器
    await userEvent.click(tileOf(1));
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
  });

  it("收藏 = 选中资产逐个 assetRatingSet(id, 5)；旗标 = assetFlagSet(id, true)", async () => {
    const user = userEvent.setup();
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(screen.getByTestId("gallery-select-toggle"));
    await user.click(tileOf(1));
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

  it("分享菜单：在资源管理器中显示 = revealItemInDir(path)；复制路径写剪贴板", async () => {
    const user = userEvent.setup();
    // userEvent.setup 会挂自己的 clipboard 桩——对其就地 spy（组件读到的是同一个）
    const writeText = vi
      .spyOn(navigator.clipboard ?? {}, "writeText")
      .mockResolvedValue(undefined);
    renderGallery();
    await screen.findAllByTestId("gallery-tile");

    await user.click(screen.getByTestId("gallery-select-toggle"));
    await user.click(tileOf(1));
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "1");

    await user.click(screen.getByTestId("selection-share"));
    await user.click(screen.getByTestId("selection-share-reveal"));
    await waitFor(() => expect(revealMock).toHaveBeenCalledWith(makeAsset(1).path));
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

    await user.click(screen.getByTestId("gallery-select-toggle"));
    await user.click(tileOf(1));
    expect(screen.getByTestId("selection-count")).toHaveTextContent("已选 1 张");

    await user.click(screen.getByTestId("selection-cancel"));
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
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
