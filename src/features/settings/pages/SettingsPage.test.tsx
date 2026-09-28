import { useImportStore } from "@/stores/importStore";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

// ipc mock：settingsStore.save 断言 payload 用
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
const ipcMock = vi.mocked(ipc);

// M4 AI：模型命令 mock（ai_models_status 走 useAiStore.refresh）
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
  aiFaceDataClear,
  aiModelCancel,
  aiModelDelete,
  aiModelDownload,
  aiModelsStatus,
  burstStats,
  indexKickNow,
  indexRebuild,
  indexStatus,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";
import { SMART_ALBUM_TAGS } from "@/features/albums/pages/AlbumsPages";
import { loadSmartTags } from "@/features/albums/lib/smartTags";

const aiModelsStatusMock = vi.mocked(aiModelsStatus);
const aiModelDownloadMock = vi.mocked(aiModelDownload);
const aiModelCancelMock = vi.mocked(aiModelCancel);
const aiModelDeleteMock = vi.mocked(aiModelDelete);
const aiFaceDataClearMock = vi.mocked(aiFaceDataClear);
const indexStatusMock = vi.mocked(indexStatus);
const indexKickNowMock = vi.mocked(indexKickNow);
const indexRebuildMock = vi.mocked(indexRebuild);
const burstStatsMock = vi.mocked(burstStats);

function aiModel(
  id: string,
  state: "idle" | "downloading" | "verifying" | "done" | "failed",
  feature: "semantic" | "face" | "selection",
  tier: "fast" | "normal" | "accurate" | null = null,
  bytesTotal = 150 * 1024 * 1024,
) {
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

function allModels(
  overrides: Array<Partial<ReturnType<typeof aiModel> & { id: string; state: "idle" | "downloading" | "verifying" | "done" | "failed" }>> = [],
) {
  const base = [
    aiModel("siglip2-visual", "done", "semantic", "normal"),
    aiModel("siglip2-text", "done", "semantic", "normal", 250 * 1024 * 1024),
    aiModel("siglip2-tokenizer", "done", "semantic", null, 34 * 1024 * 1024),
    aiModel("scrfd", "idle", "face", "normal", 2 * 1024 * 1024),
    aiModel("arcface", "idle", "face", null, 230 * 1024 * 1024),
  ];
  return base.map((m) => {
    const o = overrides.find((x) => (x as { id: string }).id === m.id);
    return o ? { ...m, ...o } : m;
  }) as Array<import("@/ipc/api").AiModelStatus>;
}

import i18n, { setAppLanguage } from "@/i18n";
import { initAppLanguage } from "@/i18n/settingsLanguage";
import SettingsPage from "./SettingsPage";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
  type Library,
} from "@/stores/settingsStore";

const LIB_A: Library = {
  id: "lib-1",
  name: "主库",
  dbDir: "D:\\SmartPhoto\\db",
  photoRoot: "D:\\Photos",
  configured: true,
streams: 4,
};
const LIB_B: Library = {
  id: "lib-2",
  name: "工作库",
  dbDir: "E:\\db2",
  photoRoot: "E:\\照片",
  configured: true,
streams: 4,
};

function PickerProbe() {
  return <div data-testid="picker-probe">PICKER</div>;
}

function renderSettingsPage() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/settings"]}>
        <Routes>
          <Route path="/settings" element={<SettingsPage />} />
          <Route path="/library-picker" element={<PickerProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

async function switchTab(user: ReturnType<typeof userEvent.setup>, key: string): Promise<void> {
  await user.click(screen.getByTestId(`settings-tab-${key}`));
}

beforeEach(() => {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
  ipcMock.mockClear();
  // AiTab 索引状态区默认不可用（各用例按需覆写）
  indexStatusMock.mockReset().mockResolvedValue(null);
  indexKickNowMock.mockReset().mockResolvedValue(undefined);
  burstStatsMock.mockReset().mockResolvedValue(null);
  useAiStore.getState().resetForTests();
  useImportStore.setState({ activeJobs: {} });
  localStorage.clear();
});

it("五种语言可选，切换立即更新界面并保存全局偏好", async () => {
  const stop = initAppLanguage();
  try {
    const user = userEvent.setup();
    renderSettingsPage();
    const select = screen.getByTestId("settings-language");
    expect(within(select).getAllByRole("option").map((option) => option.getAttribute("value")))
      .toEqual(["zh", "zh-TW", "en", "ja", "es"]);
    await user.selectOptions(select, "en");
    await screen.findByText("Language");
    expect(useSettingsStore.getState().settings.system.language).toBe("en");
    expect(ipcMock).toHaveBeenCalledWith("settings_set", {
      settings: expect.objectContaining({ system: expect.objectContaining({ language: "en" }) }),
    });
  } finally {
    stop();
    await act(async () => { await setAppLanguage("zh"); });
  }
});

describe("选项卡", () => {
  it("四个选项卡；切换渲染对应分组", async () => {
    const user = userEvent.setup();
    renderSettingsPage();

    for (const label of ["常规", "导入", "库", "AI"] as const) {
      expect(screen.getByRole("tab", { name: label })).toBeInTheDocument();
    }

    // 默认常规：关闭行为/开机自启/语言
    expect(screen.getByTestId("settings-row-close-behavior")).toBeInTheDocument();
    expect(screen.getByLabelText("开机自启")).toBeInTheDocument();
    expect(screen.getByTestId("settings-language")).toBeEnabled();
    expect(screen.queryByTestId("settings-current-library")).not.toBeInTheDocument();

    // 导入
    await switchTab(user, "import");
    expect(screen.getByLabelText("设备接入弹窗")).toBeInTheDocument();
    expect(screen.getByLabelText("跳过已导入文件（按内容指纹）")).toBeInTheDocument();
    expect(screen.getByTestId("settings-duplicate-policy")).toBeInTheDocument();
    expect(screen.getByText("双目的地导入")).toBeInTheDocument();
    expect(screen.getByText("即将支持")).toBeInTheDocument();

    // 库
    await switchTab(user, "libraries");
    expect(screen.getByTestId("settings-current-library")).toBeInTheDocument();
    expect(screen.getByTestId("settings-goto-picker")).toBeInTheDocument();

    // AI：M4 实化——模型状态区（未接后端时显示未连接提示）+ 功能开关区
    await switchTab(user, "ai");
    expect(screen.getByTestId("ai-model-list")).toBeInTheDocument();
    expect(screen.getByText(/所有 AI 处理 100% 在本机完成/)).toBeInTheDocument();
    expect(screen.getByText("模型管理")).toBeInTheDocument();
    // ④：GPU/调度收进「高级」（默认收起）；场景标签仍待后续里程碑
    expect(screen.queryByLabelText("GPU 加速")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-schedule")).not.toBeInTheDocument();
    expect(screen.getByLabelText("场景标签")).toBeDisabled();
  });

  it("常规：关闭行为切换即存（settings_set payload 断言）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();

    await user.click(screen.getByRole("radio", { name: "退出应用" }));

    expect(useSettingsStore.getState().settings.system.closeToTray).toBe(false);
    expect(ipcMock).toHaveBeenLastCalledWith(
      "settings_set",
      expect.objectContaining({
        settings: expect.objectContaining({
          system: expect.objectContaining({ closeToTray: false }),
        }),
      }),
    );
    // 选中态跟随
    expect(screen.getByRole("radio", { name: "退出应用" })).toHaveAttribute("aria-checked", "true");
  });

  it("导入：开关与查重策略修改即存", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "import");

    await user.click(screen.getByLabelText("设备接入弹窗"));
    expect(useSettingsStore.getState().settings.import.promptOnDevice).toBe(false);
    expect(ipcMock).toHaveBeenLastCalledWith(
      "settings_set",
      expect.objectContaining({
        settings: expect.objectContaining({
          import: expect.objectContaining({ promptOnDevice: false }),
        }),
      }),
    );

    await user.selectOptions(screen.getByTestId("settings-duplicate-policy"), "rename");
    expect(useSettingsStore.getState().settings.import.duplicatePolicy).toBe("rename");
  });

  it("开机自启开关修改即存（v1 仅存设置，后端接线生效）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();

    await user.click(screen.getByLabelText("开机自启"));
    expect(useSettingsStore.getState().settings.system.launchAtLogin).toBe(true);
    expect(ipcMock).toHaveBeenCalledTimes(1);
  });
});

describe("「库」选项卡（保留库管理能力）", () => {
  async function openLibraryTab(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    await switchTab(user, "libraries");
  }

  it("无激活库时信息为空，仍有前往选择器入口", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    expect(screen.getByText("设置")).toBeInTheDocument();
    expect(screen.queryByText("主库")).not.toBeInTheDocument();
    expect(screen.queryByText("D:\\Photos")).not.toBeInTheDocument();
    expect(screen.getByTestId("settings-goto-picker")).toBeInTheDocument();
  });

  it("当前库信息只读：名称/照片目录/数据库目录（模板/收纳区行已退役）", async () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB_A], activeLibraryId: "lib-1" },
    }));
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    const current = screen.getByTestId("settings-current-library");
    expect(within(current).getByText("主库")).toBeInTheDocument();
    expect(within(current).getByText("D:\\Photos")).toBeInTheDocument();
    expect(within(current).getByText("D:\\SmartPhoto\\db")).toBeInTheDocument();
    // dirTemplate/importSubdir 配置退役：不再展示库模板与导入收纳区（后者=照片目录恒重复）
    expect(within(current).queryByText("目录模板")).not.toBeInTheDocument();
    expect(within(current).queryByText("导入收纳区")).not.toBeInTheDocument();
  });

  it("并发流数（库属性）：分段展示当前值，改即存并落盘", async () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB_A], activeLibraryId: "lib-1" },
    }));
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    const group = screen.getByTestId("settings-library-streams");
    // LIB_A.streams=4：默认选中 4，2 未选
    expect(within(group).getByRole("radio", { name: "4" })).toHaveAttribute("aria-checked", "true");
    expect(within(group).getByRole("radio", { name: "2" })).toHaveAttribute("aria-checked", "false");

    await user.click(within(group).getByRole("radio", { name: "2" }));

    expect(useSettingsStore.getState().settings.libraries[0].streams).toBe(2);
    expect(ipcMock).toHaveBeenLastCalledWith(
      "settings_set",
      expect.objectContaining({
        settings: expect.objectContaining({
          libraries: [expect.objectContaining({ id: "lib-1", streams: 2 })],
        }),
      }),
    );
    expect(within(group).getByRole("radio", { name: "2" })).toHaveAttribute("aria-checked", "true");
  });

  it("库列表渲染并高亮激活库（切换统一走选择器，不在原地切换）", async () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB_A, LIB_B], activeLibraryId: "lib-1" },
    }));
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    const items = screen.getAllByTestId("settings-library-item");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveAttribute("data-active", "true");
    expect(items[0]).toHaveTextContent("使用中");
    expect(items[1]).toHaveAttribute("data-active", "false");
    expect(items[1]).toHaveTextContent("E:\\db2");
    // 非激活项仅展示（div），不可点击切换
    expect(items[1].tagName).not.toBe("BUTTON");
  });

  it("「前往库选择器」跳转 /library-picker", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    await user.click(screen.getByTestId("settings-goto-picker"));

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
  });

  it("深链 ?tab=ai 直达 AI 选项卡（语义门禁「去设置」落点）；非法 tab 回常规", async () => {
    function renderSettings(entry: string) {
      return render(
        <I18nextProvider i18n={i18n}>
          <MemoryRouter initialEntries={[entry]}>
            <Routes>
              <Route path="/settings" element={<SettingsPage />} />
            </Routes>
          </MemoryRouter>
        </I18nextProvider>,
      );
    }
    const first = renderSettings("/settings?tab=ai");
    expect(await screen.findByTestId("ai-model-list")).toBeInTheDocument();
    expect(screen.getByTestId("settings-tab-ai")).toHaveAttribute("aria-selected", "true");
    first.unmount();

    renderSettings("/settings?tab=nonsense");
    expect(screen.getByTestId("settings-row-close-behavior")).toBeInTheDocument();
    expect(screen.getByTestId("settings-tab-general")).toHaveAttribute("aria-selected", "true");
  });

  it("「新建库」打开与菜单共用的对话框（同一 testid）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await openLibraryTab(user);

    await user.click(screen.getByTestId("settings-new-library"));

    const dialog = await screen.findByTestId("new-library-dialog");
    expect(within(dialog).getByLabelText("库名称")).toBeInTheDocument();
    await user.click(within(dialog).getByTestId("new-library-cancel"));
    await waitFor(() =>
      expect(screen.queryByTestId("new-library-dialog")).not.toBeInTheDocument(),
    );
  });
});


describe("AI tab（M4 实化）", () => {
  beforeEach(() => {
    useAiStore.getState().resetForTests();
    aiModelsStatusMock.mockReset();
    aiModelDownloadMock.mockReset().mockResolvedValue(undefined);
    aiModelDeleteMock.mockReset().mockResolvedValue(undefined);
    aiFaceDataClearMock.mockReset().mockResolvedValue(true);
  });

  async function gotoAiTab(user: ReturnType<typeof userEvent.setup>) {
    await switchTab(user, "ai");
  }

  it("语义阈值（④ 起位于「高级」内）：默认 auto（跟随模型自动）；自定义改即存并夹取；恢复自动回 null", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);
    await user.click(await screen.findByTestId("ai-advanced-toggle"));

    // 默认 auto：单选选中 + 输入禁用 + 自动标注
    expect(screen.getByTestId("ai-semantic-threshold-mode")).toHaveAttribute("data-mode", "auto");
    expect(screen.getByTestId("ai-semantic-threshold")).toBeDisabled();
    expect(screen.getByTestId("ai-semantic-threshold-auto-hint")).toHaveTextContent(
      "跟随模型自动（0.09 / fp16 标定值）",
    );

    // 切自定义：落 0.09 起始值，修改即存（settingsStore 更新 + settings_set 落盘）
    await user.click(screen.getByTestId("ai-semantic-threshold-custom"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBe(0.09),
    );
    const input = screen.getByTestId("ai-semantic-threshold") as HTMLInputElement;
    await user.clear(input);
    await user.type(input, "0.15");
    expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBe(0.15);

    // 恢复自动
    await user.click(screen.getByTestId("ai-semantic-threshold-reset"));
    expect(useSettingsStore.getState().settings.ai.semanticMinScore).toBeNull();
  });

  it("模型管理：两张组卡（整组状态）+ 组内逐模型行（tier 徽章 + 单模型操作）", async () => {
    aiModelsStatusMock.mockResolvedValue(allModels());
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    // 两组卡：语义（三件全 done → 已就绪）/ 人脸（两件未装 → 未安装）
    const semantic = await screen.findByTestId("ai-model-group-semantic");
    expect(semantic).toHaveAttribute("data-installed", "3");
    expect(semantic).toHaveAttribute("data-total", "3");
    expect(screen.getByTestId("ai-group-badge-semantic")).toHaveTextContent("已就绪");
    expect(screen.getByText("语义搜索模型")).toBeInTheDocument();
    expect(screen.getByTestId("ai-group-badge-face")).toHaveTextContent("未安装");
    expect(screen.getByTestId("ai-model-group-face")).toHaveAttribute("data-installed", "0");
    expect(screen.getByText("人脸识别模型")).toBeInTheDocument();

    expect(screen.queryByTestId("ai-model-row-siglip2-visual")).not.toBeInTheDocument();
    expect(screen.getByTestId("ai-group-delete-semantic")).toBeInTheDocument();
    expect(screen.getByTestId("ai-group-download-face")).toHaveTextContent("下载");

  });

  it("第三包选片辅助（C 阶段）：feature=selection 模型渲染独立组卡（整组状态）", async () => {
    const user = userEvent.setup();
    aiModelsStatusMock.mockReset().mockResolvedValue([
      ...allModels(),
      aiModel("eyes", "idle", "selection", null, 90 * 1024 * 1024),
    ]);
    renderSettingsPage();
    await switchTab(user, "ai");

    const pkg = await screen.findByTestId("ai-model-group-selection");
    expect(pkg).toHaveAttribute("data-total", "1");
    expect(within(pkg).getByText("选片辅助模型")).toBeInTheDocument();
    expect(screen.getByTestId("ai-group-badge-selection")).toHaveTextContent("未安装");
    expect(screen.getByTestId("ai-group-download-selection")).toBeInTheDocument();
  });


  it("组级全部下载：发起组内全部未装模型；进度事件聚合到组进度条", async () => {
    aiModelsStatusMock
      .mockResolvedValueOnce(allModels())
      .mockResolvedValue(
        allModels([
          { id: "scrfd", state: "downloading", downloadedBytes: 0 },
          {
            id: "arcface",
            state: "downloading",
            downloadedBytes: 0,
            bytesTotal: 4 * 1024 * 1024,
          },
        ]),
      );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    await user.click(await screen.findByTestId("ai-group-download-face"));
    // 两件未装模型都发起（顺序 await，逐个结算）
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(2));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("scrfd");
    expect(aiModelDownloadMock).toHaveBeenCalledWith("arcface");

    // refresh 后状态翻 downloading：组徽标 + 行内取消按钮出现
    await waitFor(() =>
      expect(screen.getByTestId("ai-group-badge-face")).toHaveTextContent("下载中"),
    );
    expect(screen.getByTestId("ai-group-cancel-face")).toBeInTheDocument();

    // 事件进度（节流 1s）聚合到组级：scrfd 1MB/2MB + arcface 2MB/4MB = 3MB/6MB
    act(() => {
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadProgress",
        id: "scrfd",
        doneBytes: 1024 * 1024,
        totalBytes: 2 * 1024 * 1024,
      });
      useAiStore.getState().handleAppEvent({
        type: "aiModelDownloadProgress",
        id: "arcface",
        doneBytes: 2 * 1024 * 1024,
        totalBytes: 4 * 1024 * 1024,
      });
    });
    expect(await screen.findByTestId("ai-group-progress-face")).toBeInTheDocument();
    expect(screen.getByTestId("ai-model-group-face")).toHaveTextContent("3.0 MB / 6.0 MB");
    // 组内单模型行进度小字仍在
    expect(screen.queryByTestId("ai-model-row-scrfd")).not.toBeInTheDocument();
  });

  it("单模型取消：行按钮取消该在途下载", async () => {
    aiModelCancelMock.mockReset().mockResolvedValue(undefined);
    aiModelsStatusMock.mockResolvedValue(
      allModels([
        { id: "scrfd", state: "downloading", downloadedBytes: 0 },
        {
          id: "arcface",
          state: "downloading",
          downloadedBytes: 0,
          bytesTotal: 4 * 1024 * 1024,
        },
      ]),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    await user.click(await screen.findByTestId("ai-group-cancel-face"));
    await waitFor(() => expect(aiModelCancelMock).toHaveBeenCalledTimes(2));
    expect(aiModelCancelMock).toHaveBeenCalledWith("scrfd");
    expect(aiModelCancelMock).toHaveBeenCalledWith("arcface");
  });

  it("单模型删除：两步确认（可取消）；确认后删除该模型并即时刷新", async () => {
    // 真机时序根因：删除无事件回执，必须等 ai_model_delete 结算后再刷新
    const deleted = new Set<string>();
    aiModelDeleteMock.mockImplementation(async (id: string) => {
      deleted.add(id);
    });
    aiModelsStatusMock.mockImplementation(async () =>
      allModels(
        (["siglip2-visual", "siglip2-text", "siglip2-tokenizer"] as const).map((id) => ({
          id,
          state: deleted.has(id) ? ("idle" as const) : ("done" as const),
        })),
      ),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    expect(await screen.findByTestId("ai-group-badge-semantic")).toHaveTextContent("已就绪");

    // 取消路径：不发删除
    await user.click(screen.getByTestId("ai-group-delete-semantic"));
    await user.click(screen.getByTestId("ai-group-delete-cancel-semantic"));
    expect(screen.queryByTestId("ai-group-delete-confirm-semantic")).not.toBeInTheDocument();
    expect(aiModelDeleteMock).not.toHaveBeenCalled();

    // 确认路径：只删该模型，结算后刷新快照 3→2
    await user.click(screen.getByTestId("ai-group-delete-semantic"));
    await user.click(screen.getByTestId("ai-group-delete-confirm-semantic"));
    await waitFor(() => expect(aiModelDeleteMock).toHaveBeenCalledWith("siglip2-visual"));

    // 不切选项卡：组卡即时翻「已安装 2/3」
    await waitFor(() => {
      expect(screen.getByTestId("ai-model-group-semantic")).toHaveAttribute("data-installed", "0");
    });
    expect(screen.getByTestId("ai-group-badge-semantic")).toHaveTextContent("未安装");
    expect(screen.getByTestId("ai-group-download-semantic")).toBeInTheDocument();
  });

  it("组级全部下载只发起未装模型（已装不重复发起）；部分安装显示 N/M", async () => {
    aiModelsStatusMock.mockResolvedValue(
      allModels([
        { id: "siglip2-text", state: "idle" },
        { id: "siglip2-tokenizer", state: "idle" },
      ]),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    expect(await screen.findByTestId("ai-group-badge-semantic")).toHaveTextContent("未安装完整");
    expect(screen.getByTestId("ai-group-download-semantic")).toHaveTextContent("下载");

    await user.click(screen.getByTestId("ai-group-download-semantic"));
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(2));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-text");
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-tokenizer");
    expect(aiModelDownloadMock).not.toHaveBeenCalledWith("siglip2-visual");
  });

  it("任一模型失败：组卡标错 + 失败提示，组级重试只发起未装模型", async () => {
    aiModelsStatusMock.mockResolvedValue(
      allModels([
        { id: "siglip2-text", state: "done" },
        { id: "siglip2-tokenizer", state: "failed" },
      ]),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    expect(await screen.findByTestId("ai-group-badge-semantic")).toHaveTextContent("部分失败");
    expect(screen.getByTestId("ai-group-failed-hint-semantic")).toHaveTextContent("重试");
    expect(screen.getByTestId("ai-group-download-semantic")).toHaveTextContent("重试下载");
    // 失败行按钮文案为「重试」
    expect(screen.queryByTestId("ai-model-download-siglip2-tokenizer")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("ai-group-download-semantic"));
    // 只重试未装的 tokenizer；visual/text 已装不发起
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(1));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-tokenizer");
    expect(aiModelDownloadMock).not.toHaveBeenCalledWith("siglip2-visual");
    expect(aiModelDownloadMock).not.toHaveBeenCalledWith("siglip2-text");
  });

  it("组级全部下载错误隔离：单个模型发起失败不中断其余", async () => {
    aiModelsStatusMock.mockResolvedValue(
      allModels([
        { id: "siglip2-visual", state: "idle" },
        { id: "siglip2-text", state: "idle" },
        { id: "siglip2-tokenizer", state: "idle" },
      ]),
    );
    // 第二个模型发起抛错（如未知模型 id）：其余仍要发起
    aiModelDownloadMock.mockImplementation(async (id: string) => {
      if (id === "siglip2-text") throw new Error("boom");
    });
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    await user.click(await screen.findByTestId("ai-group-download-semantic"));
    await waitFor(() => expect(aiModelDownloadMock).toHaveBeenCalledTimes(3));
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-visual");
    expect(aiModelDownloadMock).toHaveBeenCalledWith("siglip2-tokenizer");
  });

  it("功能门控：两个 siglip2 未 done → 语义开关禁用+「先下载模型」；都 done → 开启写设置", async () => {
    // scrfd/arcface 已装，语义模型未装 → 语义禁用、人脸可用
    aiModelsStatusMock.mockResolvedValue(
      allModels([
        { id: "scrfd", state: "done" },
        { id: "arcface", state: "done" },
        { id: "siglip2-visual", state: "idle" },
        { id: "siglip2-text", state: "idle" },
      ]),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    const semanticToggle = await screen.findByTestId("ai-toggle-semantic");
    expect(semanticToggle).toBeDisabled();
    expect(screen.getByText("先下载模型")).toBeInTheDocument();
    expect(screen.getByTestId("ai-toggle-face")).toBeEnabled();

    // 全部就绪 → 语义开关可开，开启即存（settings_set payload 带 enableClip=true）
    aiModelsStatusMock.mockResolvedValue(allModels());
    await user.click(screen.getByTestId("settings-tab-general"));
    await user.click(screen.getByTestId("settings-tab-ai"));
    ipcMock.mockClear();
    const toggle = await screen.findByTestId("ai-toggle-semantic");
    expect(toggle).toBeEnabled();
    await user.click(toggle);
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.enableClip).toBe(true),
    );
  });

  it("人脸数据一键清除：两步强确认 → ai_face_data_clear", async () => {
    aiModelsStatusMock.mockResolvedValue(allModels());
    const user = userEvent.setup();
    renderSettingsPage();
    await gotoAiTab(user);

    await user.click(await screen.findByTestId("ai-face-clear"));
    expect(screen.getByTestId("ai-face-clear-confirm")).toBeInTheDocument();
    await user.click(screen.getByTestId("ai-face-clear-confirm"));
    expect(aiFaceDataClearMock).toHaveBeenCalledTimes(1);
  });
});

// --- 画廊 tab：智能相册显示的标签（M4 二轮，localStorage 简化存储） -------------------

describe("画廊 tab：智能相册标签", () => {
  it("显示全部预置标签，不使用勾选框", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "gallery");

    const group = screen.getByTestId("settings-album-tags");
    const items = within(group).getAllByTestId("settings-album-tag");
    expect(items).toHaveLength(SMART_ALBUM_TAGS.length);
    expect(items[0]).toHaveAttribute("data-tag", SMART_ALBUM_TAGS[0]);
    expect(within(group).queryByRole("checkbox")).not.toBeInTheDocument();
  });

  it("可以移除和新增标签，并持久化", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "gallery");

    const first = within(screen.getByTestId("settings-album-tags")).getAllByTestId(
      "settings-album-tag",
    )[0];
    await user.click(within(first).getByRole("button"));
    expect(loadSmartTags()).not.toContain(SMART_ALBUM_TAGS[0]);
    await user.type(screen.getByRole("textbox", { name: "添加标签" }), "星轨");
    await user.click(screen.getByRole("button", { name: "添加标签" }));
    expect(loadSmartTags()).toContain("星轨");
  });
});

// --- AI tab：索引状态与操作（M4 二轮，index_status / index_kick_now） ------------------

describe("AI tab：索引状态与操作", () => {
  function statusOf(partial?: {
    thumb?: Partial<import("@/ipc/api").IndexCounters>;
    exif?: Partial<import("@/ipc/api").IndexCounters>;
    ai?: Partial<import("@/ipc/api").IndexCounters>;
  }): import("@/ipc/api").IndexStatus {
    const counters = (
      base: Partial<import("@/ipc/api").IndexCounters>,
      patch?: Partial<import("@/ipc/api").IndexCounters>,
    ): import("@/ipc/api").IndexCounters => ({
      pending: 0,
      running: 0,
      done: 0,
      failed: 0,
      total: 120,
      ...base,
      ...patch,
    });
    return {
      thumb: counters({ pending: 3, done: 117 }, partial?.thumb),
      exif: counters({ pending: 0, done: 120 }, partial?.exif),
      ai: counters({ pending: 0, done: 45, total: 120 }, partial?.ai),
    };
  }

  it("区块渲染三类计数；语义行附加已索引 N / 库内总数 M", async () => {
    indexStatusMock.mockResolvedValue(statusOf());
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    expect(await screen.findByTestId("index-status-thumb")).toBeInTheDocument();
    expect(screen.getByTestId("index-count-thumb")).toHaveTextContent("待处理 3");
    expect(screen.getByTestId("index-count-thumb")).toHaveTextContent("已完成 117");
    expect(screen.getByTestId("index-count-exif")).toHaveTextContent("待处理 0");
    expect(screen.getByTestId("index-count-ai")).toHaveTextContent("45 / 120");
  });

  it("导入尚未结束时，索引追上当前照片显示等待新照片", async () => {
    indexStatusMock.mockResolvedValue(statusOf({ thumb: { pending: 0, done: 120 } }));
    useImportStore.setState({ activeJobs: {
      7: { jobId: 7, status: "running", totalFiles: 200, totalBytes: 2000,
        doneFiles: 120, doneBytes: 1200, bytesPerSec: 100, currentFile: "photo.jpg" },
    } });
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    expect(await screen.findByTestId("index-kick-thumb")).toHaveTextContent("等待新照片");
    act(() => useImportStore.setState({ activeJobs: {} }));
    expect(screen.getByTestId("index-kick-thumb")).toHaveTextContent("已完成");
  });

  it("ai 行显示失败数（任务账 failed>0 时可见，失败不可再隐藏）", async () => {
    indexStatusMock.mockResolvedValue(
      statusOf({ ai: { pending: 0, done: 0, failed: 1326 } }),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    expect(await screen.findByTestId("index-count-ai")).toHaveTextContent("失败 1326");
  });

  it("index_status 不可用（null）→ 后端未连接提示（无计数行）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    expect(await screen.findByTestId("index-status-unavailable")).toBeInTheDocument();
    expect(screen.queryByTestId("index-status-thumb")).not.toBeInTheDocument();
  });

  it("立即索引：按钮负载 index_kick_now(kind)；kick 后重拉 pending>0 → 「进行中」", async () => {
    indexStatusMock
      .mockResolvedValueOnce(statusOf({ thumb: { pending: 0 } }))
      .mockResolvedValue(
        statusOf({ thumb: { pending: 0 }, ai: { pending: 119, done: 0, total: 119 } }),
      );
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await screen.findByTestId("index-status-ai");

    await user.click(screen.getByTestId("index-kick-ai"));
    await waitFor(() => expect(indexKickNowMock).toHaveBeenCalledWith("ai"));

    // kick 后重拉：持久化任务账 pending>0 → 禁用 + 「进行中」
    const kickAi = await screen.findByTestId("index-kick-ai");
    await waitFor(() => expect(kickAi).toBeDisabled());
    expect(kickAi).toHaveTextContent("进行中");
    // thumb 尚未完成可重建；EXIF 已随导入完成，不再提交空任务
    expect(screen.getByTestId("index-kick-thumb")).toBeEnabled();
    expect(screen.getByTestId("index-kick-exif")).toBeDisabled();
  });

  it("运行态从持久化任务账派生：未点击任何按钮，pending>0 直接「进行中」（切页重挂载不丢）", async () => {
    indexStatusMock.mockResolvedValue(
      statusOf({ thumb: { pending: 0 }, exif: { pending: 0 }, ai: { pending: 119, done: 0, total: 119 } }),
    );
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    const kickAi = await screen.findByTestId("index-kick-ai");
    expect(kickAi).toBeDisabled();
    expect(kickAi).toHaveTextContent("进行中");
    expect(screen.getByTestId("index-status-ai")).toHaveAttribute("data-running", "true");
    // 其余通道无待办 → 正常可点
    expect(screen.getByTestId("index-kick-thumb")).toBeEnabled();
  });

  it("ai 模型未就绪：后端 Err 文案透传显示", async () => {
    indexStatusMock.mockResolvedValue(statusOf());
    indexKickNowMock.mockRejectedValue("请先在设置中下载模型");
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await screen.findByTestId("index-status-thumb");

    await user.click(screen.getByTestId("index-kick-ai"));
    expect(await screen.findByTestId("index-kick-error")).toHaveTextContent(
      "请先在设置中下载模型",
    );
  });

  it("事件刷新：indexTaskProgress → index_status 重拉（计数更新）", async () => {
    indexStatusMock
      .mockResolvedValueOnce(statusOf())
      .mockResolvedValue(statusOf({ thumb: { pending: 0, done: 120, failed: 0 } }));
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await screen.findByTestId("index-status-thumb");
    expect(screen.getByTestId("index-count-thumb")).toHaveTextContent("待处理 3");

    act(() => {
      useAiStore.getState().handleAppEvent({
        type: "indexTaskProgress",
        kind: "thumb",
        done: 120,
        total: 120,
      });
    });

    await waitFor(() =>
      expect(screen.getByTestId("index-count-thumb")).toHaveTextContent("待处理 0"),
    );
  });
});

// --- 外观 tab（M4.5 wave-3：界面动画开关） -----------------------------------------------

describe("外观 tab：界面动画", () => {
  it("开关改即存（settings_set 带 appearance.animations）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "appearance");

    const toggle = screen.getByTestId("settings-animations");
    expect(toggle).toBeChecked(); // 默认开

    await user.click(toggle);
    expect(useSettingsStore.getState().settings.appearance.animations).toBe(false);
    expect(ipcMock).toHaveBeenLastCalledWith(
      "settings_set",
      expect.objectContaining({
        settings: expect.objectContaining({
          appearance: expect.objectContaining({ animations: false }),
        }),
      }),
    );
  });
});

// --- AI tab：索引参数与重建（M4.5 wave-3 第 7 项；④ 起收进「高级」折叠分组） --------------

describe("AI tab：索引参数与重建", () => {
  beforeEach(() => {
    indexRebuildMock.mockReset().mockResolvedValue(undefined);
  });

  /** 勾选「开发人员配置」展开高级调参区（默认收起） */
  async function enableAdvanced(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    await user.click(await screen.findByTestId("ai-advanced-toggle"));
  }

  it("高级参数默认收起：勾选「开发人员配置」才显示（④ 扩围：语义阈值 + 调度/资源）", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    // 默认收起：调参输入不在 DOM（语义阈值、调度/资源、连拍阈值/间隔、人脸阈值等）
    expect(await screen.findByTestId("ai-advanced-toggle")).not.toBeChecked();
    expect(screen.queryByTestId("ai-advanced-params")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-param-burst-hamming-max")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-param-burst-gap-ms")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-param-face-detect-threshold")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-semantic-threshold")).not.toBeInTheDocument();
    expect(screen.queryByTestId("ai-schedule")).not.toBeInTheDocument();

    await enableAdvanced(user);
    expect(screen.getByTestId("ai-advanced-params")).toBeInTheDocument();
    expect(screen.getByTestId("ai-param-burst-hamming-max")).toBeInTheDocument();
    // ④ 扩围项：语义阈值与调度/资源组进高级
    expect(screen.getByTestId("ai-semantic-threshold")).toBeInTheDocument();
    expect(screen.getByTestId("ai-schedule")).toBeInTheDocument();
    expect(screen.getByLabelText("GPU 加速")).toBeEnabled();
    // 开关状态持久化 localStorage
    expect(localStorage.getItem("smartphoto.settings.ai.advanced")).toBe("1");

    // 取消勾选即收起
    await user.click(screen.getByTestId("ai-advanced-toggle"));
    expect(screen.queryByTestId("ai-advanced-params")).not.toBeInTheDocument();
    expect(localStorage.getItem("smartphoto.settings.ai.advanced")).toBe("0");
  });

  it("「开发人员配置」开关跨会话记忆：localStorage 预置 1 → 进 tab 直接展开", async () => {
    localStorage.setItem("smartphoto.settings.ai.advanced", "1");
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    expect(await screen.findByTestId("ai-param-embed-input-size")).toBeInTheDocument();
    expect(screen.getByTestId("ai-advanced-toggle")).toBeChecked();
  });

  it("三参数输入改即存（钳制区间）；偏离默认出现「恢复默认」", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await enableAdvanced(user);

    expect(await screen.findByTestId("ai-param-embed-input-size")).toHaveValue(256);
    expect(screen.getByTestId("ai-param-face-detect-threshold")).toHaveValue(0.5);
    expect(screen.getByTestId("ai-param-face-cluster-threshold")).toHaveValue(0.4);
    expect(screen.queryByTestId("ai-param-embed-input-size-reset")).not.toBeInTheDocument();

    await user.clear(screen.getByTestId("ai-param-embed-input-size"));
    await user.type(screen.getByTestId("ai-param-embed-input-size"), "384");
    fireEvent.blur(screen.getByTestId("ai-param-embed-input-size")); // 失焦提交（编辑期走本地草稿）
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.embedInputSize).toBe(384),
    );
    expect(screen.getByTestId("ai-param-embed-input-size-reset")).toBeInTheDocument();

    await user.click(screen.getByTestId("ai-param-embed-input-size-reset"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.embedInputSize).toBe(256),
    );
  });

  it("重建：二次红色确认 → indexRebuild(kind)；取消不发", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    expect(await screen.findByTestId("ai-rebuild-semantic")).toBeInTheDocument();
    await user.click(screen.getByTestId("ai-rebuild-semantic"));
    expect(screen.getByTestId("ai-rebuild-confirm-semantic")).toBeInTheDocument();

    // 取消路径
    await user.click(screen.getByTestId("ai-rebuild-cancel-semantic"));
    expect(indexRebuildMock).not.toHaveBeenCalled();

    // 确认路径
    await user.click(screen.getByTestId("ai-rebuild-semantic"));
    await user.click(screen.getByTestId("ai-rebuild-confirm-semantic"));
    await waitFor(() => expect(indexRebuildMock).toHaveBeenCalledWith("semantic"));
  });

  it("重建失败（模型未下载等）：后端 Err 文案透传", async () => {
    indexRebuildMock.mockRejectedValue("请先在设置中下载模型");
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");

    await user.click(await screen.findByTestId("ai-rebuild-face"));
    await user.click(screen.getByTestId("ai-rebuild-confirm-face"));
    expect(await screen.findByTestId("ai-rebuild-error")).toHaveTextContent(
      "请先在设置中下载模型",
    );
  });
});

// --- AI tab：连拍分组（M6） ------------------------------------------------------------

describe("AI tab：连拍分组（④ 起位于「高级」折叠分组内）", () => {
  async function enableAdvanced(user: ReturnType<typeof userEvent.setup>): Promise<void> {
    await user.click(await screen.findByTestId("ai-advanced-toggle"));
  }

  it("三参数带默认值与「重新分组」说明；草稿失焦提交入库，恢复默认回滚", async () => {
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await enableAdvanced(user);

    expect(await screen.findByTestId("ai-param-burst-gap-ms")).toHaveValue(2000);
    expect(screen.getByTestId("ai-param-burst-hamming-max")).toHaveValue(10);
    expect(screen.getByTestId("ai-param-burst-min-size")).toHaveValue(2);
    expect(screen.getAllByText("修改后自动重新分组（不重算指纹）")).toHaveLength(3);
    expect(screen.queryByTestId("ai-param-burst-gap-ms-reset")).not.toBeInTheDocument();

    await user.clear(screen.getByTestId("ai-param-burst-gap-ms"));
    await user.type(screen.getByTestId("ai-param-burst-gap-ms"), "3500");
    fireEvent.blur(screen.getByTestId("ai-param-burst-gap-ms"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.burstGapMs).toBe(3500),
    );
    expect(screen.getByTestId("ai-param-burst-gap-ms-reset")).toBeInTheDocument();

    await user.click(screen.getByTestId("ai-param-burst-gap-ms-reset"));
    await waitFor(() =>
      expect(useSettingsStore.getState().settings.ai.burstGapMs).toBe(2000),
    );
  });

  it("burstStats 有值时展示「N 组 · 共 M 张」；null 隐藏该行", async () => {
    burstStatsMock.mockResolvedValue({ groups: 12, photosInBursts: 47 });
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await enableAdvanced(user);

    expect(await screen.findByTestId("ai-burst-stats")).toHaveTextContent("12 组 · 共 47 张");
  });

  it("burstStats 为 null（后端未实现/无数据）时统计行隐藏", async () => {
    burstStatsMock.mockResolvedValue(null);
    const user = userEvent.setup();
    renderSettingsPage();
    await switchTab(user, "ai");
    await enableAdvanced(user);

    expect(await screen.findByTestId("ai-param-burst-min-size")).toBeInTheDocument();
    expect(screen.queryByTestId("ai-burst-stats")).not.toBeInTheDocument();
  });
});

// --- LR 交接指引（B2 静态分区） -----------------------------------------------------------

describe("LR 交接指引（B2）", () => {
  it("常规 tab 渲染指引分区：工作流/导出建议/XMP 冲突/星级边界四行", () => {
    renderSettingsPage();

    const guide = screen.getByTestId("settings-lr-guide");
    const rows = within(guide).getAllByTestId("lr-guide-row");
    // 纯 XMP/导出建议文档（用户定案：无内建原片/成片语义）
    expect(rows).toHaveLength(2);
    expect(guide).toHaveTextContent("导出建议设置");
    expect(guide).toHaveTextContent("XMP 冲突");
    expect(guide).toHaveTextContent("从文件读取元数据");
    expect(guide).toHaveTextContent("_edit_v1.jpg");
    expect(guide).toHaveTextContent("元数据选「包含全部」");
  });
});
