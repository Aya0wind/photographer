import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import AlbumDetailPage from "./AlbumDetailPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  albumAddAssets,
  albumAssetsPage,
  albumItemMoveSubgroup,
  albumList,
  albumSubgroups,
  albumRemoveAssets,
  albumRename,
  assetThumbGet,
  type AssetDto,
  type AssetFilters,
} from "@/ipc/api";

/**
 * 手工相册详情页（/albums/:id，数字参数经 AlbumEntryPage 分发）：
 * keyset 游标分页（afterId=上一页末条 id）、相册上下文「从相册移除」（多选操作条 +
 * 右键菜单）、FilterPanel 筛选透传（隐藏相册维度）、页头重命名、「添加照片」提示。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumList: vi.fn(),
    albumAssetsPage: vi.fn(),
    albumSubgroups: vi.fn(),
    albumItemMoveSubgroup: vi.fn(),
    albumRemoveAssets: vi.fn(),
    albumRename: vi.fn(),
    albumAddAssets: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const albumListMock = vi.mocked(albumList);
const assetsPageMock = vi.mocked(albumAssetsPage);
const subgroupsMock = vi.mocked(albumSubgroups);
const moveMock = vi.mocked(albumItemMoveSubgroup);
const removeMock = vi.mocked(albumRemoveAssets);
const renameMock = vi.mocked(albumRename);
const addMock = vi.mocked(albumAddAssets);
const thumbMock = vi.mocked(assetThumbGet);

const PAGE_LIMIT = 100;

function makeAsset(id: number): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: `2026-09-1${(id % 9) + 1}T10:00:00`,
    camera: null,
    sizeBytes: 1,
  };
}

function makeAlbum(id: number, name: string, itemCount: number) {
  return { id, name, coverAssetId: null, itemCount, createdAt: "2026-09-01T00:00:00" };
}

/** 立即触发一次 intersecting 的 IO 桩（哨兵进视口 → 补页） */
class IntersectionObserverFireStub {
  callback: IntersectionObserverCallback;
  constructor(callback: IntersectionObserverCallback) {
    this.callback = callback;
  }
  observe(): void {
    this.callback(
      [{ isIntersecting: true } as IntersectionObserverEntry],
      this as unknown as IntersectionObserver,
    );
  }
  unobserve(): void {}
  disconnect(): void {}
}

function tileOf(id: number): HTMLElement {
  const tile = screen
    .getAllByTestId("gallery-tile")
    .find((t) => t.getAttribute("data-asset-id") === String(id));
  if (!tile) throw new Error(`tile ${id} not rendered`);
  return tile;
}

function checkOf(id: number): HTMLElement {
  return within(tileOf(id)).getByTestId("tile-check");
}

function renderDetail(albumId = 1) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[`/albums/${albumId}`]}>
        <Routes>
          <Route path="/albums/:tag" element={<AlbumDetailPage />} />
          <Route path="/gallery" element={<div data-testid="gallery-probe" />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  (window as unknown as Record<string, unknown>).IntersectionObserver =
    IntersectionObserverFireStub;
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  vi.clearAllMocks();
  albumListMock.mockResolvedValue([makeAlbum(1, "青海湖 2026", 3)]);
  assetsPageMock.mockResolvedValue([]);
  subgroupsMock.mockReset().mockResolvedValue([]);
  moveMock.mockReset().mockResolvedValue(true);
  removeMock.mockResolvedValue(true);
  renameMock.mockResolvedValue({ ok: true });
  addMock.mockResolvedValue(0);
  thumbMock.mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
});

describe("相册详情页：数据与分页", () => {
  it("首屏 album_assets_page(id, 0, 100)；页头展示相册名与张数", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
    renderDetail();

    expect(await screen.findByTestId("album-detail-page")).toHaveAttribute("data-album-id", "1");
    expect(screen.getByTestId("album-detail-name")).toHaveTextContent("青海湖 2026");
    expect(screen.getByTestId("album-detail-count")).toHaveTextContent("3 张");
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenCalledWith(1, 0, PAGE_LIMIT, expect.objectContaining({ subgroupIsNull: true })),
    );
    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(3);
  });

  it("keyset 游标补页：哨兵进入视口后 afterId=上一页末条 id；短页停刷", async () => {
    const page1 = Array.from({ length: PAGE_LIMIT }, (_, i) => makeAsset(i + 1));
    const page2 = [makeAsset(101), makeAsset(102)];
    assetsPageMock.mockImplementation(async (_id, afterId) =>
      afterId === 0 ? page1 : afterId === PAGE_LIMIT ? page2 : [],
    );
    renderDetail();

    await screen.findAllByTestId("gallery-tile");
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenCalledWith(1, PAGE_LIMIT, PAGE_LIMIT, expect.objectContaining({ subgroupIsNull: true })),
    );
    // 第二页短页（< limit）→ hasMore=false：不再有更多请求
    await waitFor(() =>
      expect(assetsPageMock.mock.calls.some(([, afterId]) => afterId === 102)).toBe(false),
    );
  });

  it("相册不在 album_list（已删除）：名称兜底、计数按本地资产", async () => {
    albumListMock.mockResolvedValue([]);
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    renderDetail(9);

    await screen.findAllByTestId("gallery-tile");
    expect(screen.getByTestId("album-detail-name")).toHaveTextContent("相册 #9");
    expect(screen.getByTestId("album-detail-count")).toHaveTextContent("0 张");
  });

  it("空相册空态 + 筛选空态区分", async () => {
    renderDetail();
    expect(await screen.findByTestId("album-detail-empty")).toHaveTextContent("相册还没有照片");
  });
});

describe("相册详情页：从相册移除入口", () => {
  it("多选操作条在相册上下文多「从相册移除」：移除后重置重拉 + 计数刷新", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
    const user = userEvent.setup();
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(tileOf(2));
    expect(screen.getByTestId("selection-bar")).toHaveAttribute("data-count", "2");

    // 相册上下文专属按钮
    await user.click(screen.getByTestId("selection-remove-album"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith(1, [1, 2]));

    // 重置重拉（reloadToken → 首页 0 重新请求）+ album_list 计数刷新
    await waitFor(() => expect(assetsPageMock.mock.calls.filter(([id, afterId]) => id === 1 && afterId === 0).length).toBeGreaterThanOrEqual(2));
    expect(albumListMock.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it("右键菜单在相册上下文多「从相册移除」（danger）；多选语义作用于全部选中", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2), makeAsset(3)]);
    const user = userEvent.setup();
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(checkOf(2));
    fireEvent.contextMenu(tileOf(1), { clientX: 100, clientY: 100 });
    const menu = await screen.findByTestId("album-asset-context-menu");
    const removeItem = within(menu).getByTestId("album-asset-context-menu-item-remove-album");
    expect(removeItem).toHaveTextContent("从相册移除（2 张）");

    await user.click(removeItem);
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith(1, [1, 2]));
  });
});

describe("相册详情页：页头与筛选", () => {
  it("相册名点击行内重命名：album_rename 成功更新页头；重名错误行内提示", async () => {
    renameMock
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" })
      .mockResolvedValueOnce({ ok: true });
    const user = userEvent.setup();
    renderDetail();
    await screen.findByTestId("album-detail-empty"); // 首屏加载完成

    await user.click(screen.getByTestId("album-detail-name"));
    const input = screen.getByTestId("album-detail-rename-input");
    expect(input).toHaveValue("青海湖 2026");

    await user.clear(input);
    await user.type(input, "新名字");
    await user.click(screen.getByTestId("album-detail-rename-confirm"));
    expect(await screen.findByTestId("album-detail-rename-error")).toHaveTextContent("同名相册已存在");

    await user.clear(input);
    await user.type(input, "改名后");
    await user.click(screen.getByTestId("album-detail-rename-confirm"));
    await waitFor(() => expect(renameMock).toHaveBeenLastCalledWith(1, "改名后"));
    expect(screen.getByTestId("album-detail-name")).toHaveTextContent("改名后");
  });

  it("FilterPanel 筛选透传（防抖后 filters 进 album_assets_page）；相册维度隐藏", async () => {
    vi.useFakeTimers();
    try {
      renderDetail();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });

      fireEvent.click(screen.getByTestId("album-detail-filter-toggle"));
      const panel = screen.getByTestId("search-filter-panel");
      // 相册上下文：不渲染所属相册维度（本页已在相册内）
      expect(within(panel).queryByTestId("search-album-button")).not.toBeInTheDocument();
      // 类型=照片 → kinds=[photo, raw] 透传
      fireEvent.click(within(panel).getByTestId("search-kind-photo"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(400);
      });

      const last = assetsPageMock.mock.calls[assetsPageMock.mock.calls.length - 1];
      expect(last?.[0]).toBe(1);
      expect(last?.[1]).toBe(0);
      expect(last?.[3]).toMatchObject({ kinds: ["photo", "raw"] });
    } finally {
      vi.useRealTimers();
    }
  });

  it("「添加照片」入口：v1 提示到图库多选加入；一键去图库", async () => {
    const user = userEvent.setup();
    renderDetail();
    await screen.findByTestId("album-detail-empty"); // 首屏加载完成

    await user.click(screen.getByTestId("album-add-photos"));
    const hint = await screen.findByTestId("album-add-photos-hint");
    expect(hint).toHaveTextContent("到图库多选照片");
    // 目录化归入引导（追加包）：日期根未归册照片加入本相册时默认归入（移动文件）
    expect(screen.getByTestId("album-add-photos-claim-hint")).toHaveTextContent("归入");

    await user.click(screen.getByTestId("album-add-photos-go-gallery"));
    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
  });

  it("多选「加入相册」（加入其他相册）：弹窗确定后 album_add_assets", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    albumListMock.mockResolvedValue([
      makeAlbum(1, "青海湖 2026", 2),
      makeAlbum(5, "备份册", 0),
    ]);
    addMock.mockResolvedValue(1);
    const user = userEvent.setup();
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    await user.click(checkOf(1));
    await user.click(screen.getByTestId("selection-add-album"));
    const dialog = await screen.findByTestId("add-to-album-dialog");
    // 相册列表含本相册自身；选择另一相册（备份册 id=5）
    const option5 = within(dialog)
      .getAllByTestId("add-to-album-option")
      .find((o) => o.getAttribute("data-album-id") === "5");
    if (!option5) throw new Error("album option 5 not rendered");
    await user.click(option5);
    await user.click(within(dialog).getByTestId("add-to-album-confirm"));

    await waitFor(() => expect(addMock).toHaveBeenCalledWith(5, [1]));
    await waitFor(() => expect(screen.queryByTestId("add-to-album-dialog")).not.toBeInTheDocument());
  });
});

// --- 原片/成片四态分段（B2） -------------------------------------------------------------

describe("相册详情：原片/成片分段（B2）", () => {
  it("工具条渲染四态分段（默认全部=不传）；点「只看成片」→ album_assets_page 透传 filters.groupRole", async () => {
    const user = userEvent.setup();
    renderDetail();

    expect(await screen.findByTestId("album-detail-page")).toBeInTheDocument();
    const segment = screen.getByTestId("album-grouprole");
    expect(segment).toBeInTheDocument();
    expect(screen.getByTestId("album-grouprole-all")).toHaveAttribute("aria-checked", "true");

    await user.click(screen.getByTestId("album-grouprole-derived_only"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenCalledWith(
        expect.anything(),
        expect.anything(),
        expect.anything(),
        expect.objectContaining({ groupRole: "derived_only" }),
      ),
    );
  });
});

// --- 子分组（B4 定案：相册内任意命名文件夹层） ---------------------------------------------

describe("相册详情页：子分组", () => {
  it("根视图渲染子分组文件夹卡（名称+张数）；点击进入子分组视图（面包屑+数据源 filters.subgroup）", async () => {
    subgroupsMock.mockResolvedValue([
      { name: "原片", itemCount: 5 },
      { name: "成片", itemCount: 2 },
    ]);
    assetsPageMock.mockResolvedValue([]);
    renderDetail();

    // 子分组卡横排（仅根视图）
    const cards = await screen.findAllByTestId("album-subgroup-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]).toHaveAttribute("data-subgroup", "原片");
    expect(within(cards[0]).getByTestId("album-subgroup-name")).toHaveTextContent("原片");
    expect(within(cards[0]).getByTestId("album-subgroup-count")).toHaveTextContent("5");

    // 点击进入：面包屑 + 数据源切到 filters.subgroup 精确名
    const user = userEvent.setup();
    await user.click(cards[0]);
    expect(screen.getByTestId("album-subgroup-breadcrumb")).toHaveTextContent("青海湖 2026");
    expect(screen.getByTestId("album-subgroup-breadcrumb")).toHaveTextContent("原片");
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(1, 0, PAGE_LIMIT, expect.objectContaining({ subgroup: "原片" })),
    );

    // 返回根：subgroupIsNull=true 回归
    await user.click(screen.getByTestId("album-subgroup-back"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(1, 0, PAGE_LIMIT, expect.objectContaining({ subgroupIsNull: true })),
    );
    expect(screen.queryByTestId("album-subgroup-bar")).not.toBeInTheDocument();
  });

  it("子分组内多选 → 操作条「移到子分组…」输入新名即建 → album_item_move_subgroup + 重拉", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    const user = userEvent.setup();
    await user.click(checkOf(1));
    await user.click(tileOf(2));

    await user.click(screen.getByTestId("selection-subgroup-move"));
    const menu = screen.getByTestId("selection-subgroup-menu");
    await user.type(within(menu).getByTestId("selection-subgroup-name"), "精选");
    await user.click(within(menu).getByTestId("selection-subgroup-confirm"));

    await waitFor(() => expect(moveMock).toHaveBeenCalledWith(1, [1, 2], "精选"));
    // 移组后重置重拉 + 退出多选
    await waitFor(() => expect(screen.queryByTestId("selection-bar")).not.toBeInTheDocument());
  });

  it("根视图移组弹窗不出现「移到相册根」；输入留空时确认禁用", async () => {
    subgroupsMock.mockResolvedValue([{ name: "原片", itemCount: 1 }]);
    assetsPageMock.mockResolvedValue([makeAsset(1)]);
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    const user = userEvent.setup();
    await user.click(checkOf(1));
    await user.click(screen.getByTestId("selection-subgroup-move"));
    const menu = screen.getByTestId("selection-subgroup-menu");
    // 根视图没有「移到相册根」
    expect(within(menu).queryByTestId("selection-subgroup-root")).not.toBeInTheDocument();
    // 输入留空 → 确认禁用
    expect(within(menu).getByTestId("selection-subgroup-confirm")).toBeDisabled();
  });

  it("子分组视图移回根：操作条出现「移到相册根」→ album_item_move_subgroup(id, ids, null)", async () => {
    subgroupsMock.mockResolvedValue([{ name: "原片", itemCount: 2 }]);
    assetsPageMock.mockResolvedValue([makeAsset(1), makeAsset(2)]);
    renderDetail();
    await screen.findAllByTestId("gallery-tile");

    const user = userEvent.setup();
    await user.click((await screen.findAllByTestId("album-subgroup-card"))[0]);
    // 进入子分组会重拉：等第二次请求发出并结算（loading 撤下）后瓦片重挂
    // 数据源已切到 filters.subgroup（IO 桩可能追加一次补页调用，故用 some 而非 last）
    await waitFor(() =>
      expect(
        assetsPageMock.mock.calls.some(([, , , filters]) => (filters as AssetFilters)?.subgroup === "原片"),
      ).toBe(true),
    );
    await screen.findAllByTestId("gallery-tile");
    await user.click(checkOf(1));
    await user.click(tileOf(2));

    await user.click(screen.getByTestId("selection-subgroup-move"));
    await user.click(screen.getByTestId("selection-subgroup-root"));
    await waitFor(() => expect(moveMock).toHaveBeenCalledWith(1, [1, 2], null));
  });

  it("工具条「导入成片」按钮 → 打开导入成片对话框", async () => {
    renderDetail();
    await screen.findByTestId("album-detail-page");
    const user = userEvent.setup();
    await user.click(screen.getByTestId("album-import-derived"));
    expect(screen.getByTestId("import-derived-dialog")).toBeInTheDocument();
  });
});
