import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";

import i18n from "@/i18n";
import GlobalSearchBox from "./GlobalSearchBox";
import { useAiStore } from "@/stores/aiStore";
import { DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";
import { aiModelsStatus, indexStatus, type AiModelStatus, type IndexStatus } from "@/ipc/api";

// 门禁真值 mock：useSemanticGate 挂载即拉 ai_models_status / index_status
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, aiModelsStatus: vi.fn(), indexStatus: vi.fn() };
});

const aiModelsStatusMock = vi.mocked(aiModelsStatus);
const indexStatusMock = vi.mocked(indexStatus);

/** 落点 URL 断言探针（渲染 search 参数） */
function LocationProbe() {
  const location = useLocation();
  return (
    <div>
      <div data-testid="search-probe" />
      <span data-testid="loc">{`${location.pathname}${location.search}`}</span>
    </div>
  );
}

function renderBox() {
  return renderBoxAt("/gallery");
}

function renderBoxAt(entry: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[entry]}>
        <GlobalSearchBox />
        <Routes>
          <Route path="/gallery" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

/** 带 /settings 路由探针（门禁「去设置」落点断言） */
function renderBoxWithSettings() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <GlobalSearchBox />
        <Routes>
          <Route path="/gallery" element={<LocationProbe />} />
          <Route path="/settings" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

function aiStatus(done: number, total: number): IndexStatus {
  return {
    thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
    exif: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
    ai: { pending: total - done, running: 0, done, failed: 0, total },
    face: { pending: 0, running: 0, done: 0, failed: 0, total: 0 },
  };
}

function model(
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
    model("siglip2-visual", "semantic", "done"),
    model("siglip2-text", "semantic", "done"),
    model("siglip2-tokenizer", "semantic", "done"),
    model("scrfd", "face", "done"),
    model("arcface", "face", "done"),
  ];
}

/** 语义三件缺一件（text 未装）→ 门禁 models 分支 */
function gatedModels(): AiModelStatus[] {
  return readyModels().map((m) =>
    m.id === "siglip2-text" ? { ...m, state: "idle" as const, installed: false } : m,
  );
}

beforeEach(() => {
  // 语义搜索测试默认开启功能（关闭态另有专测）
  useSettingsStore.setState({
    settings: { ...DEFAULT_SETTINGS, ai: { ...DEFAULT_SETTINGS.ai, enableClip: true } },
    loaded: true,
  });

  useAiStore.getState().resetForTests();
  // 默认就绪：模型全装 + 语义索引已建（库内有资产）
  aiModelsStatusMock.mockReset().mockResolvedValue(readyModels());
  indexStatusMock.mockReset().mockResolvedValue({
    ...aiStatus(100, 100),
    thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 100 },
  });
});

describe("顶部全局搜索框（M4.5 A1）", () => {
  it("占位文案与过滤图标；空输入回车 → /search（条件模式）", async () => {
    renderBox();

    expect(screen.getByTestId("globalsearch-input")).toHaveAttribute(
      "placeholder",
      "输入一段描述搜索照片…",
    );
    expect(screen.getByTestId("globalsearch-filter")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });
    expect(await screen.findByTestId("loc")).toHaveTextContent("/gallery");
  });

  it("输入描述回车 → /search?mode=semantic&q=（URL 编码）", async () => {
    const user = userEvent.setup();
    renderBox();

    await user.type(screen.getByTestId("globalsearch-input"), "海边 日落");
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });

    // 回车跳 /gallery?mode=semantic&q=（URL 编码；画廊+搜索合并）
    expect(await screen.findByTestId("loc")).toHaveTextContent(
      "/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9%20%E6%97%A5%E8%90%BD",
    );
  });

  it("过滤图标 → /search", async () => {
    const user = userEvent.setup();
    renderBox();

    await user.click(screen.getByTestId("globalsearch-filter"));
    expect(await screen.findByTestId("search-probe")).toBeInTheDocument();
  });

  it("语义功能未开启：输入框禁用（占位引导去设置），不显示索引提示", () => {
    useSettingsStore.setState({
      settings: { ...DEFAULT_SETTINGS, ai: { ...DEFAULT_SETTINGS.ai, enableClip: false } },
    });
    useAiStore.setState({ indexStatus: aiStatus(30, 100) });
    renderBox();
    const input = screen.getByTestId("globalsearch-input");
    expect(input).toBeDisabled();
    expect(input).toHaveAttribute("placeholder", "未开启语义搜索（设置 → AI 中开启）");
    expect(screen.queryByTestId("globalsearch-hint")).not.toBeInTheDocument();
  });

  it("语义索引未建完 → 输入框下细提示条；建完/未知不提示", () => {
    const { rerender } = renderBox();
    expect(screen.queryByTestId("globalsearch-hint")).not.toBeInTheDocument();

    useAiStore.setState({ indexStatus: aiStatus(30, 100) });
    rerender(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery"]}>
          <GlobalSearchBox />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(screen.getByTestId("globalsearch-hint")).toHaveTextContent("语义索引建立中");

    useAiStore.setState({ indexStatus: aiStatus(100, 100) });
    rerender(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery"]}>
          <GlobalSearchBox />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(screen.queryByTestId("globalsearch-hint")).not.toBeInTheDocument();
  });
});

describe("语义搜索前置门禁（模型未齐/索引未建 → 拦截 + 行内提示）", () => {
  it("模型未齐：回车不导航、输入文字保留；下拉提示带一键跳设置", async () => {
    const user = userEvent.setup();
    aiModelsStatusMock.mockResolvedValue(gatedModels());
    renderBoxWithSettings();

    await user.type(screen.getByTestId("globalsearch-input"), "海边 日落");
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });

    // 不发查询不导航：原地停留，文字保留
    expect(await screen.findByTestId("globalsearch-gate")).toBeInTheDocument();
    expect(screen.getByTestId("globalsearch-gate-notice")).toHaveTextContent("语义模型未下载");
    expect(screen.getByTestId("loc")).toHaveTextContent("/gallery");
    expect(screen.getByTestId("globalsearch-input")).toHaveValue("海边 日落");

    // 一键跳设置（AI tab 深链）
    await user.click(screen.getByTestId("globalsearch-gate-notice-gosettings"));
    expect(await screen.findByTestId("loc")).toHaveTextContent("/settings?tab=ai");
  });

  it("索引从未建立（模型已齐 + ai.total==0 + 库内有资产）：同样拦截", async () => {
    const user = userEvent.setup();
    indexStatusMock.mockResolvedValue({
      ...aiStatus(0, 0),
      thumb: { pending: 0, running: 0, done: 0, failed: 0, total: 120 },
    });
    renderBox();

    await user.type(screen.getByTestId("globalsearch-input"), "日落");
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });

    expect(await screen.findByTestId("globalsearch-gate-notice")).toHaveTextContent(
      "语义索引未建立",
    );
    expect(screen.getByTestId("loc")).toHaveTextContent("/gallery");
  });

  it("条件解除：模型装齐（下载完成事件重拉）→ 提示自动消失、回车恢复导航", async () => {
    const user = userEvent.setup();
    // 挂载拉到未就绪；下载完成后事件驱动重拉 → 就绪
    aiModelsStatusMock.mockResolvedValueOnce(gatedModels()).mockResolvedValue(readyModels());
    renderBox();

    await user.type(screen.getByTestId("globalsearch-input"), "日落");
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });
    expect(await screen.findByTestId("globalsearch-gate")).toBeInTheDocument();

    // 模型装齐（aiModelDownloadFinished → aiStore 重拉）：提示条自动消失
    act(() => {
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadFinished",
        id: "siglip2-text",
        ok: true,
      });
    });
    await waitFor(() =>
      expect(screen.queryByTestId("globalsearch-gate")).not.toBeInTheDocument(),
    );

    // 再回车：恢复语义导航
    fireEvent.keyDown(screen.getByTestId("globalsearch-input"), { key: "Enter" });
    expect(await screen.findByTestId("loc")).toHaveTextContent(
      "/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD",
    );
  });
});


describe("全局搜索框回显（⑤：唯一语义入口）", () => {
  it("画廊语义态（URL 直达）→ 回显当前查询词", async () => {
    renderBoxAt("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");
    expect(await screen.findByTestId("globalsearch-input")).toHaveValue("日落");
  });

  it("语义态改词回车 = 按新词重搜（URL 协议）；清空回车 = 退出语义态回默认画廊", async () => {
    const user = userEvent.setup();
    renderBoxAt("/gallery?mode=semantic&q=%E6%97%A5%E8%90%BD");
    const input = await screen.findByTestId("globalsearch-input");
    expect(input).toHaveValue("日落");

    await user.clear(input);
    await user.type(input, "海边");
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByTestId("loc")).toHaveTextContent(
      "/gallery?mode=semantic&q=%E6%B5%B7%E8%BE%B9",
    );

    // 清空回车 → 无参 /gallery（画廊随之退出语义态、回显清空）
    await user.clear(input);
    fireEvent.keyDown(input, { key: "Enter" });
    expect(await screen.findByTestId("loc")).toHaveTextContent("/gallery");
  });

  it("非语义路由不回显（保持空起点）", async () => {
    renderBoxAt("/gallery?kind=raw");
    expect(await screen.findByTestId("globalsearch-input")).toHaveValue("");
  });
});
