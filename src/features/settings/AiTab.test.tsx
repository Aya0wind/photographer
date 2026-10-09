import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

// settingsStore.save → settings_set payload 断言用
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
const ipcMock = vi.mocked(ipc);

// 模型/索引命令 mock（ai_models_status 走 useAiStore.refresh）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    aiModelsStatus: vi.fn(),
    aiModelDownload: vi.fn(),
    aiModelCancel: vi.fn(),
    aiModelDelete: vi.fn(),
    aiFaceDataClear: vi.fn(),
    indexStatus: vi.fn(),
    indexKickNow: vi.fn(),
    indexRebuild: vi.fn(),
    burstStats: vi.fn(),
  };
});
import {
  aiModelCancel,
  aiModelDelete,
  aiModelDownload,
  aiModelsStatus,
  indexStatus,
  burstStats,
  type AiModelStatus,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import i18n from "@/i18n";
import AiTab from "./AiTab";
import { clone, DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";

const aiModelsStatusMock = vi.mocked(aiModelsStatus);
const aiModelDownloadMock = vi.mocked(aiModelDownload);
const aiModelCancelMock = vi.mocked(aiModelCancel);
const aiModelDeleteMock = vi.mocked(aiModelDelete);
const indexStatusMock = vi.mocked(indexStatus);
const burstStatsMock = vi.mocked(burstStats);

function aiModel(
  id: string,
  state: AiModelStatus["state"],
  feature: AiModelStatus["feature"],
  tier: AiModelStatus["tier"] = null,
  bytesTotal = 150 * 1024 * 1024,
): AiModelStatus {
  return {
    id,
    installed: state === "done",
    bytesTotal,
    downloadedBytes: state === "done" ? bytesTotal : 0,
    version: state === "done" ? "v1.0" : null,
    feature,
    state,
    tier,
  };
}

/** 全目录快照（新契约模型齐）：语义 normal 三件 + fp16 两件；人脸 scrfd/scrfd-10g/arcface */
function fullCatalog(overrides: Array<Partial<AiModelStatus> & { id: string }> = []): AiModelStatus[] {
  const base = [
    aiModel("siglip2-visual", "done", "semantic", "normal", 90 * 1024 * 1024),
    aiModel("siglip2-text", "done", "semantic", "normal", 250 * 1024 * 1024),
    aiModel("siglip2-tokenizer", "done", "semantic", null, 34 * 1024 * 1024),
    aiModel("siglip2-visual-fp16", "idle", "semantic", "accurate", 340 * 1024 * 1024),
    aiModel("siglip2-text-fp16", "idle", "semantic", "accurate", 610 * 1024 * 1024),
    aiModel("scrfd", "idle", "face", "normal", 39 * 1024 * 1024),
    aiModel("scrfd-10g", "idle", "face", "fast", 17 * 1024 * 1024),
    aiModel("arcface", "idle", "face", null, 230 * 1024 * 1024),
  ];
  return base.map((m) => {
    const o = overrides.find((x) => x.id === m.id);
    return o ? { ...m, ...o } : m;
  });
}

function renderAiTab() {
  return render(
    <I18nextProvider i18n={i18n}>
      <AiTab />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
  });
  ipcMock.mockClear();
  aiModelsStatusMock.mockReset();
  indexStatusMock.mockReset().mockResolvedValue(null);
  burstStatsMock.mockReset().mockResolvedValue(null);
  aiModelDownloadMock.mockReset().mockResolvedValue(undefined);
  aiModelCancelMock.mockReset().mockResolvedValue(undefined);
  aiModelDeleteMock.mockReset().mockResolvedValue(undefined);
  useAiStore.getState().resetForTests();
  localStorage.clear();
});

// --- 档位选择器：渲染 -------------------------------------------------------------------

describe("AI 画质档位选择器", () => {
  it("三档选项（各带一行说明）；默认普通选中；每档展示就绪状态", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue(fullCatalog());
    renderAiTab();

    const section = await screen.findByTestId("ai-quality-tier");
    expect(section).toHaveAttribute("data-tier", "normal");

    const group = screen.getByRole("radiogroup", { name: "AI 画质档位" });
    expect(within(group).getByRole("radio", { name: /快速/ })).toHaveAttribute("aria-checked", "false");
    expect(within(group).getByRole("radio", { name: /普通/ })).toHaveAttribute("aria-checked", "true");
    expect(within(group).getByRole("radio", { name: /精准/ })).toHaveAttribute("aria-checked", "false");

    // 每档一行说明
    expect(screen.getByText("速度优先 · 小模型，索引与搜索最快")).toBeInTheDocument();
    expect(screen.getByText("均衡 · 默认")).toBeInTheDocument();
    expect(screen.getByText("最高准确度 · 大模型与高清检测源")).toBeInTheDocument();

    // 就绪状态：normal 缺 scrfd 1 件；fast 缺 scrfd-10g 1 件；accurate 缺 scrfd+fp16 两件 3 件
    expect(screen.getByTestId("ai-tier-option-status-normal")).toHaveTextContent("缺 1 件");
    expect(screen.getByTestId("ai-tier-option-status-fast")).toHaveTextContent("缺 1 件");
    expect(screen.getByTestId("ai-tier-option-status-accurate")).toHaveTextContent("缺 3 件");
  });

  it("当前档所需模型清单及安装状态（含 tier 徽章与共用件）", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue(fullCatalog());
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    expect(await screen.findByTestId("ai-model-group-semantic")).toHaveAttribute("data-installed", "3");
    expect(screen.getByTestId("ai-model-group-face")).toHaveAttribute("data-installed", "0");
    expect(screen.queryByTestId("ai-tier-models")).not.toBeInTheDocument();

  });

  it("后端未连接（清单空）：仅档位选择器，无状态与清单；切档直存", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue([]);
    const user = userEvent.setup();
    renderAiTab();

    await screen.findByTestId("ai-quality-tier");
    expect(screen.queryByTestId("ai-tier-models")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-tier-option-status-normal")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("ai-tier-option-fast"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("fast"),
    );
  });
});

// --- 档位切换三态 -----------------------------------------------------------------------

describe("档位切换三态", () => {
  it("齐备直切：确认弹窗提示自动重建索引；确认后写 qualityTier", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue(
      fullCatalog([{ id: "scrfd-10g", state: "done" }]),
    );
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    await user.click(screen.getByTestId("ai-tier-option-fast"));

    // 确认弹窗：切换即生效 + 自动重建受影响通道索引
    const dialog = await screen.findByTestId("ai-tier-switch-confirm");
    expect(within(dialog).getByText(/将自动重建受影响通道的索引/)).toBeInTheDocument();

    // 取消路径：不写设置
    await user.click(within(dialog).getByTestId("ai-tier-switch-cancel"));
    expect(screen.queryByTestId("ai-tier-switch-confirm")).not.toBeInTheDocument();
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");

    // 确认路径：settings_set 带 ai.qualityTier=fast
    await user.click(screen.getByTestId("ai-tier-option-fast"));
    await user.click(screen.getByTestId("ai-tier-switch-confirm-yes"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("fast"),
    );
    expect(ipcMock).toHaveBeenLastCalledWith(
      "settings_set",
      expect.objectContaining({
        settings: expect.objectContaining({
          ai: expect.objectContaining({ qualityTier: "fast" }),
        }),
      }),
    );
    // 选择器选中态跟随
    expect(screen.getByTestId("ai-quality-tier")).toHaveAttribute("data-tier", "fast");
  });

  it("缺件弹窗：列出缺失件；「仅下载」只发起下载、不切档", async () => {
    // scrfd 已装：精准档缺口聚焦在 fp16 两件
    aiModelsStatusMock.mockReset().mockResolvedValue(
      fullCatalog([{ id: "scrfd", state: "done" }]),
    );
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    await user.click(screen.getByTestId("ai-tier-option-accurate"));

    const dialog = await screen.findByTestId("ai-tier-missing-dialog");
    expect(within(dialog).getByText(/还需下载 2 个模型/)).toBeInTheDocument();
    expect(within(dialog).getByTestId("ai-tier-missing-feature-semantic")).toBeInTheDocument();

    await user.click(within(dialog).getByTestId("ai-tier-download-only"));
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(2));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-visual-fp16");
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-text-fp16");

    // 仅下载：档位不变，也无等待横幅
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");
    expect(screen.queryByTestId("ai-tier-pending")).not.toBeInTheDocument();
  });

  it("下载并切换：发起缺件下载 → 全部就绪后自动应用档位（事件驱动）", async () => {
    const doneIds = new Set<string>([
      "siglip2-visual",
      "siglip2-text",
      "siglip2-tokenizer",
      "scrfd",
    ]);
    aiModelsStatusMock.mockReset().mockImplementation(async () =>
      fullCatalog().map((m) => (doneIds.has(m.id) ? { ...m, state: "done" as const, installed: true } : m)),
    );
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    await user.click(screen.getByTestId("ai-tier-option-accurate"));
    await user.click(screen.getByTestId("ai-tier-download-switch"));

    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(2));
    // 等待横幅：就绪计数 2/4
    const pending = await screen.findByTestId("ai-tier-pending");
    expect(pending).toHaveAttribute("data-tier", "accurate");
    expect(pending).toHaveTextContent("2/4");
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");

    // 逐件下载完成（aiModelDownloadFinished → refresh → 快照更新）
    act(() => {
      doneIds.add("siglip2-visual-fp16");
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadFinished",
        id: "siglip2-visual-fp16",
        ok: true,
      });
    });
    await waitFor(() => expect(screen.getByTestId("ai-tier-pending")).toHaveTextContent("3/4"));

    act(() => {
      doneIds.add("siglip2-text-fp16");
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadFinished",
        id: "siglip2-text-fp16",
        ok: true,
      });
    });

    // 全部就绪 → 自动应用精准档，横幅退场
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("accurate"),
    );
    await waitFor(() => expect(screen.queryByTestId("ai-tier-pending")).not.toBeInTheDocument());
    expect(screen.getByTestId("ai-quality-tier")).toHaveAttribute("data-tier", "accurate");
  });

  it("等待中可取消：横幅退场，下载完成后不切档", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue(fullCatalog());
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    await user.click(screen.getByTestId("ai-tier-option-accurate"));
    await user.click(screen.getByTestId("ai-tier-download-switch"));
    await screen.findByTestId("ai-tier-pending");

    await user.click(screen.getByTestId("ai-tier-pending-cancel"));
    expect(screen.queryByTestId("ai-tier-pending")).not.toBeInTheDocument();

    // 模型后续就绪也不会应用档位（等待已取消）
    aiModelsStatusMock.mockResolvedValue(fullCatalog([{ id: "siglip2-visual-fp16", state: "done" }, { id: "siglip2-text-fp16", state: "done" }]));
    await act(async () => {
      await useAiStore.getState().refresh();
    });
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");
  });

  it("点击当前档为空操作（无弹窗）", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue(fullCatalog());
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-quality-tier");

    await user.click(screen.getByTestId("ai-tier-option-normal"));
    expect(screen.queryByTestId("ai-tier-switch-confirm")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-tier-missing-dialog")).not.toBeInTheDocument();
  });
});

// --- 语义阈值三态 -----------------------------------------------------------------------

describe("语义阈值三态（auto / 自定义）", () => {
  async function enableAdvanced(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    await user.click(await screen.findByTestId("ai-advanced-toggle"));
  }

  it("默认 auto：单选选中、输入禁用、标注跟随模型自动", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue([]);
    const user = userEvent.setup();
    renderAiTab();
    await enableAdvanced(user);

    const mode = screen.getByTestId("ai-semantic-threshold-mode");
    expect(mode).toHaveAttribute("data-mode", "auto");
    expect(screen.getByTestId("ai-semantic-threshold-auto")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("ai-semantic-threshold-custom")).toHaveAttribute("aria-checked", "false");
    expect(screen.getByTestId("ai-semantic-threshold")).toBeDisabled();
    expect(screen.getByTestId("ai-semantic-threshold-auto-hint")).toHaveTextContent(
      "跟随模型自动（0.09 / fp16 标定值）",
    );
    // auto 态无「恢复自动」按钮
    expect(screen.queryByTestId("ai-semantic-threshold-reset")).not.toBeInTheDocument();
  });

  it("切自定义：落 0.09 起始值；输入改即存并夹取 [0,1]", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue([]);
    const user = userEvent.setup();
    renderAiTab();
    await enableAdvanced(user);

    await user.click(screen.getByTestId("ai-semantic-threshold-custom"));
    expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBe(0.09);
    expect(screen.getByTestId("ai-semantic-threshold-mode")).toHaveAttribute("data-mode", "custom");
    expect(screen.getByTestId("ai-semantic-threshold")).toBeEnabled();

    const input = screen.getByTestId("ai-semantic-threshold") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "0.15");
    expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBe(0.15);

    // 恢复自动 → null（auto 语义）
    await user.click(screen.getByTestId("ai-semantic-threshold-reset"));
    expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBeNull();
    expect(screen.getByTestId("ai-semantic-threshold-mode")).toHaveAttribute("data-mode", "auto");
  });

  it("持久化值往返：已存自定义值渲染 custom 态；已存 null 渲染 auto 态", async () => {
    aiModelsStatusMock.mockReset().mockResolvedValue([]);
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, ai: { ...s.settings.ai, semanticMinScore: 0.2 } },
    }));
    const user = userEvent.setup();
    renderAiTab();
    await enableAdvanced(user);

    expect(screen.getByTestId("ai-semantic-threshold-mode")).toHaveAttribute("data-mode", "custom");
    expect(screen.getByTestId("ai-semantic-threshold")).toHaveValue(0.2);
  });
});

// --- 模型管理：逐模型行交互 -------------------------------------------------------------

describe("模型管理（按档位和功能整组）", () => {
  it("只显示功能组，浏览另一档不会改变当前库档位", async () => {
    aiModelsStatusMock.mockResolvedValue(fullCatalog());
    const user = userEvent.setup();
    renderAiTab();
    const semantic = await screen.findByTestId("ai-model-group-semantic");
    expect(semantic).toHaveAttribute("data-total", "3");
    expect(semantic).toHaveAttribute("data-installed", "3");
    expect(screen.queryByTestId("ai-model-row-siglip2-visual")).not.toBeInTheDocument();
    await user.click(screen.getByTestId("ai-package-tier-accurate"));
    expect(screen.getByTestId("ai-model-group-semantic")).toHaveAttribute("data-installed", "1");
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");
  });
  it("整组下载仅发起所浏览档位的缺失资源", async () => {
    aiModelsStatusMock.mockResolvedValue(fullCatalog());
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-model-group-semantic");
    await user.click(screen.getByTestId("ai-package-tier-accurate"));
    await user.click(screen.getByTestId("ai-group-download-semantic"));
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(2));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-visual-fp16");
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-text-fp16");
    expect(aiModelDownloadMock).not.toHaveBeenCalledWith("siglip2-tokenizer");
  });
  it("整组删除需要确认，删除共用资源后各档状态同步刷新", async () => {
    const deleted = new Set<string>();
    aiModelDeleteMock.mockImplementation(async (id) => { deleted.add(id); });
    aiModelsStatusMock.mockImplementation(async () => fullCatalog().map((m) =>
      deleted.has(m.id) ? { ...m, state: "idle" as const, installed: false } : m));
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-model-group-semantic");
    await user.click(screen.getByTestId("ai-group-delete-semantic"));
    await user.click(screen.getByTestId("ai-group-delete-cancel-semantic"));
    expect(aiModelDeleteMock).not.toHaveBeenCalled();
    await user.click(screen.getByTestId("ai-group-delete-semantic"));
    await user.click(screen.getByTestId("ai-group-delete-confirm-semantic"));
    await waitFor(() => expect(screen.getByTestId("ai-model-group-semantic")).toHaveAttribute("data-installed", "0"));
    expect(aiModelDeleteMock).toHaveBeenCalledTimes(3);
    await user.click(screen.getByTestId("ai-package-tier-fast"));
    expect(screen.getByTestId("ai-model-group-semantic")).toHaveAttribute("data-installed", "0");
  });
  it("整组取消只取消正在下载的资源", async () => {
    aiModelsStatusMock.mockResolvedValue(fullCatalog([{ id: "siglip2-visual-fp16", state: "downloading" }]));
    const user = userEvent.setup();
    renderAiTab();
    await screen.findByTestId("ai-model-group-semantic");
    await user.click(screen.getByTestId("ai-package-tier-accurate"));
    expect(screen.getByTestId("ai-group-progress-semantic")).toBeInTheDocument();
    await user.click(screen.getByTestId("ai-group-cancel-semantic"));
    await waitFor(() => expect(aiModelCancelMock).toHaveBeenCalledWith("siglip2-visual-fp16"));
    expect(aiModelCancelMock).toHaveBeenCalledTimes(1);
  });
});
