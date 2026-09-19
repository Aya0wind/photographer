import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "@/app/shell/Sidebar";
import PeoplePage from "@/features/people/pages/PeoplePage";
import { AlbumsIndexPage, AlbumTagPage, SMART_ALBUM_TAGS } from "./AlbumsPages";
import { HIDDEN_ALBUM_TAGS_KEY } from "../lib/hiddenTags";
import { searchSemantic, assetsByIds, assetThumbGet } from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    searchSemantic: vi.fn(),
    assetsByIds: vi.fn(),
    assetThumbGet: vi.fn(),
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
const convertMock = vi.mocked(convertFileSrc);

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
          <Route path="/albums/:tag" element={<AlbumTagPage />} />
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
  useAiStore.getState().resetForTests();
});

describe("M4 路由与侧栏", () => {
  it("侧栏新增「人物」「智能相册」入口，点击导航对应路由", async () => {
    const user = userEvent.setup();
    renderRoutes("/gallery");

    const nav = screen.getByRole("navigation", { name: "primary" });
    const peopleLink = within(nav).getByRole("link", { name: /人物/ });
    await user.click(peopleLink);
    expect(await screen.findByTestId("people-page")).toBeInTheDocument();

    const albumsLink = within(nav).getByRole("link", { name: /智能相册/ });
    await user.click(albumsLink);
    expect(await screen.findByTestId("albums-page")).toBeInTheDocument();
  });

  it("人物页 v1 空态：说明 + 占位网格", async () => {
    renderRoutes("/people");

    const page = await screen.findByTestId("people-page");
    expect(page).toHaveTextContent("人脸聚类将在索引完成后自动生成");
    expect(screen.getAllByTestId("people-placeholder-card").length).toBeGreaterThanOrEqual(6);
  });

  it("智能相册索引页：渲染预置标签（11 个），点击跳 /albums/:tag", async () => {
    const user = userEvent.setup();
    renderRoutes("/albums");

    const tags = await screen.findAllByTestId("albums-tag");
    expect(tags).toHaveLength(SMART_ALBUM_TAGS.length);
    expect(tags.map((t) => t.getAttribute("data-tag"))).toContain("日落");

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
