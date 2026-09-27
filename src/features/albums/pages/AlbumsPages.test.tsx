import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "@/app/shell/Sidebar";
import PeoplePage from "@/features/people/pages/PeoplePage";
import {
  AlbumsIndexPage,
  AlbumEntryPage,
  SMART_ALBUM_TAGS,
} from "./AlbumsPages";
import { HIDDEN_ALBUM_TAGS_KEY } from "../lib/hiddenTags";
import {
  aiModelsStatus,
  albumAssetsPage,
  albumCoverSet,
  albumCreate,
  albumDelete,
  albumList,
  albumRename,
  assetThumbGet,
  assetsByIds,
  indexStatus,
  searchSemantic,
  type AlbumDto,
} from "@/ipc/api";
import type { AiModelStatus, IndexStatus } from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    searchSemantic: vi.fn(),
    assetsByIds: vi.fn(),
    assetThumbGet: vi.fn(),
    aiModelsStatus: vi.fn(),
    indexStatus: vi.fn(),
    albumList: vi.fn(),
    albumCreate: vi.fn(),
    albumRename: vi.fn(),
    albumDelete: vi.fn(),
    albumCoverSet: vi.fn(),
    albumAssetsPage: vi.fn(),
  };
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const searchSemanticMock = vi.mocked(searchSemantic);
const assetsByIdsMock = vi.mocked(assetsByIds);
const thumbMock = vi.mocked(assetThumbGet);
const aiModelsStatusMock = vi.mocked(aiModelsStatus);
const indexStatusMock = vi.mocked(indexStatus);
const albumListMock = vi.mocked(albumList);
const albumCreateMock = vi.mocked(albumCreate);
const albumRenameMock = vi.mocked(albumRename);
const albumDeleteMock = vi.mocked(albumDelete);
const albumCoverSetMock = vi.mocked(albumCoverSet);
const albumAssetsPageMock = vi.mocked(albumAssetsPage);
const convertMock = vi.mocked(convertFileSrc);

function gateModel(
  id: string,
  feature: "semantic" | "face",
  state: AiModelStatus["state"],
): AiModelStatus {
  return {
    id,
    installed: state === "done",
    bytesTotal: 1024,
    downloadedBytes: state === "done" ? 1024 : 0,
    version: null,
    feature,
    state,
  };
}

function readyModels(): AiModelStatus[] {
  return [
    gateModel("siglip2-visual", "semantic", "done"),
    gateModel("siglip2-text", "semantic", "done"),
    gateModel("siglip2-tokenizer", "semantic", "done"),
    gateModel("scrfd", "face", "done"),
    gateModel("arcface", "face", "done"),
  ];
}

/** 语义三件缺 text → 门禁 models 分支 */
function gatedModels(): AiModelStatus[] {
  return readyModels().map((m) =>
    m.id === "siglip2-text" ? { ...m, state: "idle" as const, installed: false } : m,
  );
}

function builtIndex(): IndexStatus {
  return {
    thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 100 },
    exif: { pending: 0, running: 0, done: 0, failed: 0, total: 100 },
    ai: { pending: 0, running: 0, done: 100, failed: 0, total: 100 },
    face: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
  };
}

function makeAsset(id: number, name: string) {
  return {
    id,
    path: `Y:\\照片\\${name}`,
    name,
    kind: "photo" as const,
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    sizeBytes: 1,
  };
}

function renderRoutes(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Sidebar />
        <Routes>
          <Route path="/people" element={<PeoplePage />} />
          <Route path="/albums" element={<AlbumsIndexPage />} />
          {/* 与 routes.tsx 一致：数字参数=手工相册详情，其余=智能标签结果 */}
          <Route path="/albums/:tag" element={<AlbumEntryPage />} />
          <Route path="/settings" element={<div data-testid="settings-probe" />} />
          <Route path="/gallery" element={<div data-testid="gallery-probe" />} />
          <Route path="/search" element={<div data-testid="search-probe" />} />
          <Route path="/import" element={<div data-testid="import-probe" />} />
          <Route path="/tasks" element={<div data-testid="tasks-probe" />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  // jsdom 无布局：虚拟网格视口为空会一行都不渲染
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  localStorage.clear();
  searchSemanticMock.mockReset().mockResolvedValue([]);
  assetsByIdsMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  albumListMock.mockReset().mockResolvedValue([]);
  albumCreateMock.mockReset();
  albumRenameMock.mockReset();
  albumDeleteMock.mockReset().mockResolvedValue(true);
  albumCoverSetMock.mockReset().mockResolvedValue(true);
  albumAssetsPageMock.mockReset().mockResolvedValue([]);
  // 语义门禁默认就绪（模型全装 + 索引已建）；各用例按需覆写为被拦态
  aiModelsStatusMock.mockReset().mockResolvedValue(readyModels());
  indexStatusMock.mockReset().mockResolvedValue(builtIndex());
  useAiStore.getState().resetForTests();
});

describe("M4 路由与侧栏", () => {
  it("侧栏「人物」「相册」入口，点击导航对应路由", async () => {
    const user = userEvent.setup();
    renderRoutes("/gallery");

    const nav = screen.getByRole("navigation", { name: "primary" });
    const peopleLink = within(nav).getByRole("link", { name: /人物/ });
    await user.click(peopleLink);
    expect(await screen.findByTestId("people-page")).toBeInTheDocument();

    const albumsLink = within(nav).getByRole("link", { name: /相册/ });
    await user.click(albumsLink);
    expect(await screen.findByTestId("albums-page")).toBeInTheDocument();
  });

  it("人物页 v1 空态：说明 + 占位网格", async () => {
    renderRoutes("/people");

    const page = await screen.findByTestId("people-page");
    expect(page).toHaveTextContent("人脸聚类将在索引完成后自动生成");
    expect(screen.getAllByTestId("people-placeholder-card").length).toBeGreaterThanOrEqual(6);
  });

  it("标签卡片墙：渲染 40 个预置标签（M4.5 B1），点击跳 /albums/:tag", async () => {
    const user = userEvent.setup();
    renderRoutes("/albums");

    const tags = await screen.findAllByTestId("albums-tag");
    expect(SMART_ALBUM_TAGS).toHaveLength(40); // 词表扩到 40（含原 11 个）
    expect(tags).toHaveLength(40);
    expect(tags.map((t) => t.getAttribute("data-tag"))).toContain("日落");
    expect(tags.map((t) => t.getAttribute("data-tag"))).toContain("猫");
    expect(tags.map((t) => t.getAttribute("data-tag"))).toContain("森林");
    await user.click(screen.getAllByTestId("albums-tag")[0]);
    expect(await screen.findByTestId("album-tag-page")).toBeInTheDocument();
  });

  it("标签页：自动语义搜索该词（searchSemantic 负载）+ 结果相似度角标", async () => {
    searchSemanticMock.mockResolvedValue([
      { assetId: 5, score: 0.87 },
      { assetId: 6, score: 0.42 },
    ]);
    assetsByIdsMock.mockResolvedValue([makeAsset(5, "SUN_1.JPG"), makeAsset(6, "SUN_2.JPG")]);
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderRoutes("/albums/%E6%97%A5%E8%90%BD");

    // 自动执行（无手动触发）
    await screen.findAllByTestId("gallery-tile");
    expect(searchSemanticMock).toHaveBeenCalledWith("日落", 100, undefined);
    expect(assetsByIdsMock).toHaveBeenCalledWith([5, 6]);
    expect(screen.getByTestId("album-tag-page")).toHaveTextContent("智能相册 · 日落");

    const tiles = screen.getAllByTestId("gallery-tile");
    expect(tiles[0]).toHaveAttribute("data-asset-id", "5");
    expect(within(tiles[0]).getByTestId("search-score-badge")).toHaveTextContent("87%");
  });

  it("标签页改词重搜：输入新词点搜索 → 新负载", async () => {
    const user = userEvent.setup();
    renderRoutes("/albums/%E6%97%A5%E8%90%BD");
    await screen.findByTestId("album-tag-page");
    await waitFor(() => expect(searchSemanticMock).toHaveBeenCalledTimes(1));

    // 语义输入为独立受控框（空起点 + 占位示例）：输入新词即搜新词
    await user.type(screen.getByTestId("semantic-input"), "雪");
    await user.click(screen.getByTestId("semantic-run"));

    await waitFor(() => expect(searchSemanticMock).toHaveBeenLastCalledWith("雪", 100, undefined));
  });
});

// --- 标签封面（M4 二轮）：首条语义命中缩略图，后台批量预取，失败静默占位 -------------

describe("智能相册标签封面", () => {
  it("命中标签显示封面 img（首条命中缩略图）；未命中标签显示占位", async () => {
    searchSemanticMock.mockImplementation((q: string) =>
      q === "日落" ? Promise.resolve([{ assetId: 9, score: 0.9 }]) : Promise.resolve([]),
    );
    thumbMock.mockResolvedValue({ status: "ready", path: "D:\\cache\\9.jpg" });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderRoutes("/albums");

    const img = await screen.findByTestId("albums-tag-cover-img");
    expect(img).toHaveAttribute("data-tag", "日落");
    expect(img).toHaveAttribute("src", "asset://D:\\cache\\9.jpg");
    expect(searchSemanticMock).toHaveBeenCalledWith("日落", 1);
    expect(thumbMock).toHaveBeenCalledWith(9, 240);

    const fallbacks = await screen.findAllByTestId("albums-tag-cover-fallback");
    expect(fallbacks).toHaveLength(SMART_ALBUM_TAGS.length - 1);
  });

  it("缩略图缺失 → 全部静默占位（无 img）", async () => {
    searchSemanticMock.mockImplementation((q: string) =>
      q === "日落" ? Promise.resolve([{ assetId: 9, score: 0.9 }]) : Promise.resolve([]),
    );
    renderRoutes("/albums");
    await screen.findByTestId("albums-page");

    await waitFor(() =>
      expect(screen.getAllByTestId("albums-tag-cover-fallback")).toHaveLength(
        SMART_ALBUM_TAGS.length,
      ),
    );
    expect(screen.queryByTestId("albums-tag-cover-img")).not.toBeInTheDocument();
    // 渐变底占位带标签名（无封面也能识别）
    const fallbacks = screen.getAllByTestId("albums-tag-cover-fallback");
    expect(fallbacks[0].className).toContain("bg-gradient-to-br");
    expect(fallbacks[0]).toHaveTextContent(String(fallbacks[0].getAttribute("data-tag")));
  });
});

// --- 标签隐藏（M4 二轮）：设置页勾选写入 localStorage，相册页过滤 ---------------------

describe("智能相册标签隐藏", () => {
  it("localStorage 记录的隐藏标签不渲染", async () => {
    localStorage.setItem(HIDDEN_ALBUM_TAGS_KEY, JSON.stringify(["夜景", "美食"]));
    renderRoutes("/albums");

    const tags = await screen.findAllByTestId("albums-tag");
    expect(tags).toHaveLength(SMART_ALBUM_TAGS.length - 2);
    const shown = tags.map((tag) => tag.getAttribute("data-tag"));
    expect(shown).not.toContain("夜景");
    expect(shown).not.toContain("美食");
    expect(shown).toContain("日落");
  });

  it("全部隐藏 → 提示文案（无标签网格）", async () => {
    localStorage.setItem(HIDDEN_ALBUM_TAGS_KEY, JSON.stringify([...SMART_ALBUM_TAGS]));
    renderRoutes("/albums");

    expect(await screen.findByTestId("albums-all-hidden")).toBeInTheDocument();
    expect(screen.queryByTestId("albums-tag")).not.toBeInTheDocument();
  });
});

// --- 语义门禁（模型未齐/索引未建 → 点相册不导航、标签页不发查询） ----------------------

describe("智能相册：语义搜索前置门禁", () => {
  it("模型未齐：点相册不导航，行内提示 + 一键跳设置", async () => {
    const user = userEvent.setup();
    aiModelsStatusMock.mockResolvedValue(gatedModels());
    // 同步预置（挂载即拦，不等异步 refresh；封面预取仍会调 searchSemantic，不算入口查询）
    useAiStore.setState({ models: gatedModels(), modelsLoaded: true, indexStatus: builtIndex() });
    renderRoutes("/albums");

    const tags = await screen.findAllByTestId("albums-tag");
    await user.click(tags[0]);

    // 被拦：不进入 /albums/:tag，仍在标签墙
    expect(await screen.findByTestId("albums-gate-notice")).toHaveTextContent("语义模型未下载");
    expect(screen.getByTestId("albums-page")).toBeInTheDocument();
    expect(screen.queryByTestId("album-tag-page")).not.toBeInTheDocument();

    // 一键跳设置（AI tab 深链）
    await user.click(screen.getByTestId("albums-gate-notice-gosettings"));
    expect(await screen.findByTestId("settings-probe")).toBeInTheDocument();
  });

  it("索引从未建立（模型已齐 + ai.total==0 + 库内有资产）：同样拦截", async () => {
    const user = userEvent.setup();
    indexStatusMock.mockResolvedValue({
      ...builtIndex(),
      ai: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
    });
    renderRoutes("/albums");

    await user.click((await screen.findAllByTestId("albums-tag"))[0]);
    expect(await screen.findByTestId("albums-gate-notice")).toHaveTextContent("语义索引未建立");
    expect(screen.queryByTestId("album-tag-page")).not.toBeInTheDocument();
  });

  it("标签页直链：门禁未过不自动执行、输入框回车不发查询（文字保留）", async () => {
    const user = userEvent.setup();
    aiModelsStatusMock.mockResolvedValue(gatedModels());
    // 同步预置：标签页自动执行在挂载 effect 里同步判定，异步 refresh 来不及
    useAiStore.setState({ models: gatedModels(), modelsLoaded: true, indexStatus: builtIndex() });
    renderRoutes("/albums/%E6%97%A5%E8%90%BD");

    expect(await screen.findByTestId("album-tag-gate-notice")).toHaveTextContent("语义模型未下载");
    // 自动执行被拦
    await waitFor(() => expect(useAiStore.getState().modelsLoaded).toBe(true));
    expect(searchSemanticMock).not.toHaveBeenCalled();

    // 输入框回车同样被拦：文字保留、不发查询
    await user.type(screen.getByTestId("semantic-input"), "海边");
    fireEvent.keyDown(screen.getByTestId("semantic-input"), { key: "Enter" });
    expect(searchSemanticMock).not.toHaveBeenCalled();
    expect(screen.getByTestId("semantic-input")).toHaveValue("海边");
  });

  it("就绪放行：模型全装 + 索引已建 → 点相册正常进入并自动执行", async () => {
    const user = userEvent.setup();
    searchSemanticMock.mockResolvedValue([]);
    renderRoutes("/albums");

    await user.click((await screen.findAllByTestId("albums-tag"))[0]);
    expect(await screen.findByTestId("album-tag-page")).toBeInTheDocument();
    await waitFor(() => expect(searchSemanticMock).toHaveBeenCalled());
    expect(screen.queryByTestId("albums-gate-notice")).not.toBeInTheDocument();
  });
});

// --- 手工相册区（2026-09 相册重构：手工 + 智能同页两区） ---------------------------------

function makeAlbum(id: number, name: string, itemCount: number): AlbumDto {
  return { id, name, coverAssetId: null, itemCount, createdAt: "2026-09-01T00:00:00" };
}

describe("相册页两区：手工相册 + 智能相册", () => {
  it("album_list 渲染手工区卡片（名称+张数+空封面占位），智能区 40 标签照旧", async () => {
    albumListMock.mockResolvedValue([makeAlbum(1, "青海湖 2026", 12), makeAlbum(2, "空相册", 0)]);
    renderRoutes("/albums");

    // 手工区：两分区头 + 两张卡片
    expect(await screen.findByTestId("albums-manual-section")).toBeInTheDocument();
    expect(screen.getByTestId("albums-manual-header")).toHaveTextContent("手工相册");
    const cards = await screen.findAllByTestId("albums-manual-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]).toHaveAttribute("data-album-id", "1");
    expect(screen.getAllByTestId("albums-manual-name").map((n) => n.textContent)).toEqual([
      "青海湖 2026",
      "空相册",
    ]);
    expect(screen.getAllByTestId("albums-manual-count").map((c) => c.textContent)).toEqual([
      "12",
      "0",
    ]);
    // 未指定封面且未取到首张 → 占位图形
    expect(screen.getAllByTestId("albums-manual-cover-fallback")).toHaveLength(2);

    // 智能区：同款分区头 + 40 标签墙（点击行为照旧）
    expect(screen.getByTestId("albums-smart-header")).toHaveTextContent("智能相册");
    expect(screen.getAllByTestId("albums-tag")).toHaveLength(SMART_ALBUM_TAGS.length);
  });

  it("手工相册封面：coverAssetId 指定 → 该资产缩略图；未指定 → 取列表第一张", async () => {
    albumListMock.mockResolvedValue([
      { ...makeAlbum(1, "有封面", 5), coverAssetId: 9 },
      makeAlbum(2, "无封面", 3),
    ]);
    thumbMock.mockImplementation((assetId: number) =>
      Promise.resolve(
        assetId === 9
          ? { status: "ready", path: "D:\\cache\\9.jpg" }
          : { status: "pending" },
      ),
    );
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    albumAssetsPageMock.mockResolvedValue([makeAsset(11, "FIRST.JPG")]);
    renderRoutes("/albums");

    const img = await screen.findByTestId("albums-manual-cover");
    expect(img).toHaveAttribute("src", "asset://D:\\cache\\9.jpg");
    // 未指定封面：album_assets_page 取首条 → 该资产缩略图
    await waitFor(() => expect(albumAssetsPageMock).toHaveBeenCalledWith(2, 0, 1));
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(11, 240));
  });

  it("点击手工相册卡片 → /albums/:id 详情页（数字参数分发）", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([makeAlbum(7, "旅行", 4)]);
    renderRoutes("/albums");

    await user.click((await screen.findAllByTestId("albums-manual-card"))[0]);
    expect(await screen.findByTestId("album-detail-page")).toHaveAttribute("data-album-id", "7");
  });

  it("新建相册：成功即时插入卡片；重名后端错误行内提示且不新增", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([]);
    albumCreateMock
      .mockResolvedValueOnce({ ok: true, album: makeAlbum(1, "新相册", 0) })
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" });
    renderRoutes("/albums");

    // 展开 → 输入 → 提交：成功后卡片出现、表单收起
    await user.click(await screen.findByTestId("albums-new-button"));
    await user.type(screen.getByTestId("albums-new-name"), "新相册");
    await user.click(screen.getByTestId("albums-new-submit"));
    expect(await screen.findAllByTestId("albums-manual-card")).toHaveLength(1);
    expect(screen.queryByTestId("albums-new-name")).not.toBeInTheDocument();

    // 再次新建同名：后端重名错误行内透传
    await user.click(screen.getByTestId("albums-new-button"));
    await user.type(screen.getByTestId("albums-new-name"), "新相册");
    await user.click(screen.getByTestId("albums-new-submit"));
    expect(await screen.findByTestId("albums-new-error")).toHaveTextContent("同名相册已存在");
    expect(screen.getByTestId("albums-new-name")).toBeInTheDocument(); // 表单保留可改
    expect(screen.getAllByTestId("albums-manual-card")).toHaveLength(1);
  });

  it("手工区空态文案（album_list 为空）", async () => {
    renderRoutes("/albums");
    expect(await screen.findByTestId("albums-manual-empty")).toHaveTextContent("还没有相册");
    expect(screen.queryByTestId("albums-manual-card")).not.toBeInTheDocument();
  });

  it("卡片右键菜单：重命名 / 设为封面 / 删除；删除红色确认弹窗文案「仅移除引用，N 张照片保留在图库」", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([makeAlbum(3, "待整理", 8)]);
    renderRoutes("/albums");

    const card = (await screen.findAllByTestId("albums-manual-card"))[0];
    fireEvent.contextMenu(card, { clientX: 120, clientY: 80 });
    const menu = await screen.findByTestId("albums-card-context-menu");
    expect(within(menu).getByTestId("albums-card-context-menu-item-rename")).toHaveTextContent("重命名");
    expect(within(menu).getByTestId("albums-card-context-menu-item-cover")).toHaveTextContent("设为封面");
    expect(within(menu).getByTestId("albums-card-context-menu-item-delete")).toHaveTextContent("删除相册");

    // 删除 → 红色确认弹窗（引用语义文案）
    await user.click(within(menu).getByTestId("albums-card-context-menu-item-delete"));
    const dialog = await screen.findByTestId("album-delete-dialog");
    expect(within(dialog).getByTestId("album-delete-hint")).toHaveTextContent(
      "仅移除引用，8 张照片保留在图库",
    );

    // 确认 → album_delete(3) + 卡片移除
    await user.click(within(dialog).getByTestId("album-delete-confirm"));
    await waitFor(() => expect(albumDeleteMock).toHaveBeenCalledWith(3));
    await waitFor(() => expect(screen.queryByTestId("albums-manual-card")).not.toBeInTheDocument());
  });

  it("悬浮 ⋯ 菜单与右键同一菜单；重命名弹窗：重名错误行内提示，成功更新卡片名", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([makeAlbum(5, "旧名", 2)]);
    albumRenameMock
      .mockResolvedValueOnce({ ok: false, error: "同名相册已存在" })
      .mockResolvedValueOnce({ ok: true });
    renderRoutes("/albums");

    await user.click((await screen.findAllByTestId("albums-card-menu"))[0]);
    await user.click(
      within(await screen.findByTestId("albums-card-context-menu")).getByTestId(
        "albums-card-context-menu-item-rename",
      ),
    );

    const dialog = await screen.findByTestId("album-rename-dialog");
    const input = within(dialog).getByTestId("album-rename-input");
    expect(input).toHaveValue("旧名");

    // 重名错误行内提示（不关闭弹窗）
    await user.clear(input);
    await user.type(input, "新名");
    await user.click(within(dialog).getByTestId("album-rename-confirm"));
    expect(await within(dialog).findByTestId("album-rename-error")).toHaveTextContent("同名相册已存在");

    // 修正后成功：卡片名更新
    await user.clear(input);
    await user.type(input, "更新名");
    await user.click(within(dialog).getByTestId("album-rename-confirm"));
    await waitFor(() =>
      expect(screen.getByTestId("albums-manual-name")).toHaveTextContent("更新名"),
    );
    expect(albumRenameMock).toHaveBeenLastCalledWith(5, "更新名");
  });

  it("设为封面：进入选择弹窗，点选照片调 album_cover_set", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([makeAlbum(6, "选封面", 4)]);
    albumAssetsPageMock.mockResolvedValue([makeAsset(21, "A.JPG"), makeAsset(22, "B.JPG")]);
    renderRoutes("/albums");

    await user.click((await screen.findAllByTestId("albums-card-menu"))[0]);
    await user.click(
      within(await screen.findByTestId("albums-card-context-menu")).getByTestId(
        "albums-card-context-menu-item-cover",
      ),
    );

    const picker = await screen.findByTestId("album-cover-dialog");
    const options = await within(picker).findAllByTestId("album-cover-option");
    expect(options).toHaveLength(2);
    await user.click(options[1]);
    await waitFor(() => expect(albumCoverSetMock).toHaveBeenCalledWith(6, 22));
  });
});
