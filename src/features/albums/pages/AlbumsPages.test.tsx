import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "@/app/shell/Sidebar";
import PeoplePage from "@/features/people/pages/PeoplePage";
import { AlbumsIndexPage, AlbumTagPage, SMART_ALBUM_TAGS } from "./AlbumsPages";
import { HIDDEN_ALBUM_TAGS_KEY } from "../lib/hiddenTags";
import { searchSemantic, assetsByIds, assetThumbGet, aiModelsStatus, indexStatus } from "@/ipc/api";
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
    // #tags 锚点区块（侧栏「标签」入口指向 /albums#tags）
    expect(screen.getByTestId("albums-tags-section")).toHaveAttribute("id", "tags");

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
