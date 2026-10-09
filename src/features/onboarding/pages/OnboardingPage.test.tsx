import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：store 模块依赖；settings_set 断言用
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
const ipcMock = vi.mocked(ipc);

// plugin-dialog mock：目录选择器
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
import { open as openDialog } from "@tauri-apps/plugin-dialog";
const openMock = vi.mocked(openDialog);

import OnboardingPage from "./OnboardingPage";
import { NEW_LIBRARY_DRAFT_KEY } from "@/features/library/NewLibraryDialog";
import "@/i18n";
import { useAiStore } from "@/stores/aiStore";
import type { AiModelStatus } from "@/ipc/api";

function readyModels(): AiModelStatus[] {
  return ["siglip2-visual", "siglip2-text", "siglip2-tokenizer", "siglip2-visual-fp16", "siglip2-text-fp16", "scrfd", "scrfd-10g", "arcface", "facemesh", "open-closed-eye"].map((id) => ({ id, state: "done", installed: true, bytesTotal: 100, downloadedBytes: 100, version: "1", feature: id.startsWith("siglip2") ? "semantic" : ["facemesh", "open-closed-eye"].includes(id) ? "selection" : "face" }));
}
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
  type Library,
} from "@/stores/settingsStore";

function primeStore() {
  useSettingsStore.setState({ settings: clone(DEFAULT_SETTINGS), loaded: true });
}

function renderWizard() {
  return render(
    <MemoryRouter initialEntries={["/onboarding"]}>
      <OnboardingPage />
    </MemoryRouter>,
  );
}

function GalleryProbe() {
  return <div data-testid="gallery-probe" />;
}

function PickerProbe() {
  return <div data-testid="picker-probe">PICKER</div>;
}

/** 带探针路由：取消/完成等导航断言用 */
function renderWizardAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/onboarding" element={<OnboardingPage />} />
        <Route path="/gallery" element={<GalleryProbe />} />
        <Route path="/library-picker" element={<PickerProbe />} />
      </Routes>
    </MemoryRouter>,
  );
}

const CONFIGURED_LIB: Library = {
  id: "lib-ok",
  name: "主库",
  dbDir: "D:\\db",
  photoRoot: "D:\\照片",
  configured: true,
streams: 4,
};

/** 本次会话由 NewLibraryDialog 新建、尚未配置完成的空库 */
const FRESH_LIB: Library = {
  id: "lib-fresh",
  name: "新库",
  dbDir: "D:\\db2",
  photoRoot: "D:\\新照片",
  configured: false,
streams: 4,
};

/** 存量未配置库（从选择器点进来补完的老库） */
const OLD_UNCONFIGURED: Library = { ...FRESH_LIB, id: "lib-old", name: "老库" };

describe("OnboardingPage 向导", () => {
  beforeEach(() => {
    primeStore();
    ipcMock.mockReset().mockImplementation(async (cmd) => cmd === "ai_models_status" ? readyModels() : undefined);
    useAiStore.getState().resetForTests();
    useAiStore.setState({ models: readyModels(), modelsLoaded: true });
    openMock.mockReset();
    sessionStorage.removeItem(NEW_LIBRARY_DRAFT_KEY);
  });

  it("步骤1 只预填库名；目录留空待用户自选（2026-09-28 用户定规：无默认路径）", () => {
    renderWizard();
    expect((screen.getByLabelText("库名称") as HTMLInputElement).value).toBe("主库");
    expect((screen.getByLabelText("数据库目录") as HTMLInputElement).value).toBe("");
    expect((screen.getByLabelText("照片存储目录") as HTMLInputElement).value).toBe("");
    expect(screen.getByRole("button", { name: "下一步" })).toBeDisabled();
  });

  it("两个目录都填后才允许下一步（dbDir 为空同样拦截）", () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\\照片" } });
    expect(screen.getByRole("button", { name: "下一步" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("数据库目录"), {
      target: { value: "D:\\SmartPhoto\\db" },
    });
    expect(screen.getByRole("button", { name: "下一步" })).toBeEnabled();
  });

  it("浏览按钮回填所选目录", async () => {
    openMock.mockResolvedValue("D:\\新照片库");
    renderWizard();
    const browseButtons = screen.getAllByText("浏览…");
    fireEvent.click(browseButtons[browseButtons.length - 1]); // 最后一项 = 照片存储目录
    await waitFor(() =>
      expect((screen.getByLabelText("照片存储目录") as HTMLInputElement).value).toBe(
        "D:\\新照片库",
      ),
    );
  });

  it.each([
    ["fast", "快速"], ["normal", "普通"], ["accurate", "精确"],
  ])("完整流程：选择 %s 方案并以正确负载提交设置", async (tier, tierLabel) => {
    renderWizard();

    // 目录不再预填（2026-09-28）：手填两目录后才能进下一步
    fireEvent.change(screen.getByLabelText("数据库目录"), {
      target: { value: "D:\\SmartPhoto\\db" },
    });
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\\照片" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    // 目录布局已固定（dirTemplate 配置退役）：只读展示公式，无模板输入
    expect(await screen.findByText("目录布局（固定）")).toBeDefined();
    expect(screen.getByText("{相册创建年}\\{相册创建月}\\{相册目录}")).toBeInTheDocument();
    expect(screen.queryByLabelText("目录命名模板")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();

    // 步骤2 → 步骤3：选"仅语义搜索"
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    fireEvent.click(await screen.findByText("仅语义搜索"));
    expect(screen.getByRole("radio", { name: /^普通/ })).toBeChecked();
    fireEvent.click(screen.getByRole("radio", { name: new RegExp(`^${tierLabel}`) }));
    // 暂时关闭 AI 不丢失所选方案，重新开启时保留。
    fireEvent.click(screen.getByText("全部关闭"));
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("仅语义搜索"));
    expect(screen.getByRole("radio", { name: new RegExp(`^${tierLabel}`) })).toBeChecked();
    expect(screen.getByText("仅语义搜索").closest("button")?.getAttribute("aria-pressed")).toBe(
      "true",
    );

    // 步骤3 → 步骤4：确认摘要后提交
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect(await screen.findByText("开始使用 Photo Hub")).toBeDefined();
    expect(screen.queryByText("目录布局（固定）")).not.toBeInTheDocument();
    expect(screen.getByText(tierLabel)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "开始使用 Photo Hub" }));

    await waitFor(() => expect(ipcMock).toHaveBeenCalledWith("settings_set", expect.anything()));
    const payload = ipcMock.mock.calls.find(([cmd]) => cmd === "settings_set")?.[1] as {
      settings: Record<string, unknown>;
    };
    const settings = payload.settings;
    expect(settings["onboardingCompleted"]).toBe(true);
    expect(typeof settings["activeLibraryId"]).toBe("string");
    const libraries = settings["libraries"] as Array<Record<string, string | boolean>>;
    expect(libraries).toHaveLength(1);
    expect(libraries[0]["name"]).toBe("主库");
    expect(libraries[0]["dbDir"]).toBe("D:\\SmartPhoto\\db");
    expect(libraries[0]["photoRoot"]).toBe("D:\\照片");
    // 目录布局固定（dirTemplate/importSubdir 均已退役不再落库）
    expect(libraries[0]["dirTemplate"]).toBeUndefined();
    expect(libraries[0]["importSubdir"]).toBeUndefined();
    expect(libraries[0]["configured"]).toBe(true);
    expect(libraries[0]["aiQualityTier"]).toBe(tier);
    const ai = settings["ai"] as Record<string, unknown>;
    expect(ai["enableClip"]).toBe(true);
    expect(ai["enableFace"]).toBe(false);
    expect(ai["enableSceneTags"]).toBe(false);
    expect(ai["qualityTier"]).toBe(tier);
    expect((settings["import"] as Record<string, unknown>)["notifyMilestones"]).toBeUndefined();
    // store 本地状态同步（守卫放行依赖它）
    expect(useSettingsStore.getState().settings.onboardingCompleted).toBe(true);
    expect(useSettingsStore.getState().settings.activeLibraryId).toBe(
      settings["activeLibraryId"] as string,
    );
    // 会话内已选库标记（达芬奇式门）
    expect(useSettingsStore.getState().libraryChosen).toBe(true);
  });

  it("补完模式（?library=<id>）：预填既有库并在提交时更新而非新增", async () => {
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        libraries: [
          {
            id: "lib-x",
            name: "旧库",
            dbDir: "I:\\SmartPhoto\\旧库",
            photoRoot: "Z:\\旧照片",
            configured: false,
            streams: 3,
          },
        ],
        activeLibraryId: null,
      },
    }));

    render(
      <MemoryRouter initialEntries={["/onboarding?library=lib-x"]}>
        <OnboardingPage />
      </MemoryRouter>,
    );

    expect((screen.getByLabelText("库名称") as HTMLInputElement).value).toBe("旧库");
    expect((screen.getByLabelText("照片存储目录") as HTMLInputElement).value).toBe("Z:\\旧照片");

    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    // 步骤2 只读展示固定目录布局（模板输入已退役）
    expect(await screen.findByText("目录布局（固定）")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    fireEvent.click(await screen.findByRole("button", { name: "下一步" }));
    fireEvent.click(await screen.findByRole("button", { name: "开始使用 Photo Hub" }));

    await waitFor(() => expect(ipcMock).toHaveBeenCalledWith("settings_set", expect.anything()));
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries).toHaveLength(1);
    expect(settings.libraries[0].id).toBe("lib-x");
    expect(settings.libraries[0].configured).toBe(true);
    // 并发流数是库属性：补完提交保留库既有值（不被默认 4 覆盖）
    expect(settings.libraries[0].streams).toBe(3);
    expect(settings.activeLibraryId).toBe("lib-x");
  });

  it("已完成引导的用户也可再次进入向导（新建库场景）", () => {
    useSettingsStore.setState({
      settings: { ...clone(DEFAULT_SETTINGS), onboardingCompleted: true },
      loaded: true,
    });
    renderWizard();
    // 新建库配置链任何时候可进（不再按 onboardingCompleted 重定向）
    expect(screen.getByLabelText("库名称")).toBeInTheDocument();
  });

  it("渲染自绘标题栏（主壳外全屏页：窗口可拖动/控制）", () => {
    primeStore();
    renderWizard();

    expect(screen.getByTestId("titlebar")).toBeInTheDocument();
    expect(screen.getByTestId("titlebar-drag-region")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByTestId("titlebar-close")).toBeInTheDocument();
    expect(screen.queryByTestId("menubar")).not.toBeInTheDocument();
  });
});

describe("退出与回退（取消 / 上一步 / 步骤指示器）", () => {
  beforeEach(() => {
    primeStore();
    ipcMock.mockClear();
    openMock.mockReset();
    sessionStorage.removeItem(NEW_LIBRARY_DRAFT_KEY);
  });

  it("上一步回退保留已填草稿（步骤1↔2 往返）", async () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText("库名称"), { target: { value: "旅行库" } });
    fireEvent.change(screen.getByLabelText("数据库目录"), { target: { value: "D:\SmartPhoto\db" } });
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\照片" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await screen.findByText("目录布局（固定）");
    fireEvent.click(screen.getByRole("radio", { name: "重命名导入（追加 _1 后缀）" }));

    fireEvent.click(screen.getByRole("button", { name: "上一步" }));

    // 回到步骤1：名称草稿保留；再前进：查重策略草稿保留（draft 常驻内存不重置）
    expect((await screen.findByLabelText("库名称") as HTMLInputElement).value).toBe("旅行库");
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect(
      await screen.findByRole("radio", { name: "重命名导入（追加 _1 后缀）" }),
    ).toBeChecked();
  });

  it("步骤指示器：已完成步可点击跳回，当前/未来步不可点；跳回草稿保留", async () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText("库名称"), { target: { value: "旅行库" } });
    fireEvent.change(screen.getByLabelText("数据库目录"), { target: { value: "D:\SmartPhoto\db" } });
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\照片" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await screen.findByText("目录布局（固定）");

    // 步骤2（index 1）：步骤0 已完成可点；当前步与未来步禁用
    expect(screen.getByTestId("onboarding-step-0")).toBeEnabled();
    expect(screen.getByTestId("onboarding-step-1")).toBeDisabled();
    expect(screen.getByTestId("onboarding-step-2")).toBeDisabled();
    expect(screen.getByTestId("onboarding-step-3")).toBeDisabled();

    fireEvent.click(screen.getByTestId("onboarding-step-0"));
    expect((await screen.findByLabelText("库名称") as HTMLInputElement).value).toBe("旅行库");
  });

  it("新建流取消（有其他已配置库）：删除空库、恢复激活库并回画廊", async () => {
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        libraries: [CONFIGURED_LIB, FRESH_LIB],
        activeLibraryId: "lib-fresh",
      },
      libraryChosen: true,
    }));
    sessionStorage.setItem(NEW_LIBRARY_DRAFT_KEY, "lib-fresh");
    renderWizardAt("/onboarding?library=lib-fresh");

    fireEvent.click(screen.getByTestId("onboarding-cancel"));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries.map((lib) => lib.id)).toEqual(["lib-ok"]);
    expect(settings.activeLibraryId).toBe("lib-ok");
    expect(useSettingsStore.getState().libraryChosen).toBe(true);
    // 删除动作落盘 + 会话新建标记清除
    await waitFor(() => expect(ipcMock).toHaveBeenCalledWith("settings_set", expect.anything()));
    expect(sessionStorage.getItem(NEW_LIBRARY_DRAFT_KEY)).toBeNull();
  });

  it("新建流取消（无其他库）：删除空库、回选择器并复位选库标志", async () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [FRESH_LIB], activeLibraryId: "lib-fresh" },
      libraryChosen: true,
    }));
    sessionStorage.setItem(NEW_LIBRARY_DRAFT_KEY, "lib-fresh");
    renderWizardAt("/onboarding?library=lib-fresh");

    fireEvent.click(screen.getByTestId("onboarding-cancel"));

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries).toHaveLength(0);
    expect(settings.activeLibraryId).toBeNull();
    expect(useSettingsStore.getState().libraryChosen).toBe(false);
    expect(sessionStorage.getItem(NEW_LIBRARY_DRAFT_KEY)).toBeNull();
  });

  it("存量未配置库取消：库保留不动（不落盘）、退出回选择器重选", async () => {
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        libraries: [OLD_UNCONFIGURED, CONFIGURED_LIB],
        activeLibraryId: "lib-old",
      },
      libraryChosen: true,
    }));
    // 无「本次会话新建」标记：从选择器点进来的老库，取消不删
    renderWizardAt("/onboarding?library=lib-old");

    fireEvent.click(screen.getByTestId("onboarding-cancel"));

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries).toHaveLength(2);
    expect(settings.activeLibraryId).toBe("lib-old");
    expect(ipcMock).not.toHaveBeenCalled();
    expect(useSettingsStore.getState().libraryChosen).toBe(false);
  });

  it("防呆：会话标记匹配但库已配置完成 → 取消不删库直接退出", async () => {
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        libraries: [CONFIGURED_LIB],
        activeLibraryId: "lib-ok",
      },
      libraryChosen: true,
    }));
    // 极端场景：标记残留（正常在配置完成时已清除）
    sessionStorage.setItem(NEW_LIBRARY_DRAFT_KEY, "lib-ok");
    renderWizardAt("/onboarding?library=lib-ok");

    fireEvent.click(screen.getByTestId("onboarding-cancel"));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(useSettingsStore.getState().settings.libraries).toHaveLength(1);
    expect(sessionStorage.getItem(NEW_LIBRARY_DRAFT_KEY)).toBeNull();
  });
});


it("开启 AI 时下载所选档位所需资源，下载未完成不能建库", async () => {
  primeStore();
  let catalog = readyModels().map((m) => m.id === "siglip2-text" ? { ...m, state: "idle" as const, installed: false, downloadedBytes: 0 } : m);
  useAiStore.setState({ models: catalog, modelsLoaded: true });
  ipcMock.mockReset().mockImplementation(async (cmd) => cmd === "ai_models_status" ? catalog : undefined);
  renderWizardAt("/onboarding");
  fireEvent.change(screen.getByLabelText("数据库目录"), { target: { value: "D:\\db" } });
  fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\\photos" } });
  fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByText("仅语义搜索"));
  fireEvent.click(screen.getByRole("button", { name: "下载 AI 资源" }));
  await waitFor(() => expect(ipcMock).toHaveBeenCalledWith("ai_model_download", { id: "siglip2-text" }));
  expect(ipcMock.mock.calls.filter(([cmd]) => cmd === "ai_model_download").map(([, args]) => args))
    .toEqual([{ id: "siglip2-text" }]); // 复用已装 visual/tokenizer，不重复下载。
  expect(screen.getByRole("button", { name: "正在准备…" })).toBeDisabled();
  expect(screen.getByRole("progressbar", { name: "语义搜索模型" })).toBeInTheDocument();
  expect(ipcMock.mock.calls.some(([cmd]) => cmd === "settings_set")).toBe(false);
  catalog = readyModels();
  await useAiStore.getState().refresh();
  fireEvent.click(await screen.findByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByRole("button", { name: "开始使用 Photo Hub" }));
  await screen.findByTestId("gallery-probe");
});

it("未开启 AI 可直接完成建库，不要求下载模型", async () => {
  primeStore();
  useAiStore.setState({ models: [], modelsLoaded: true });
  ipcMock.mockReset().mockImplementation(async () => undefined);
  renderWizardAt("/onboarding");
  fireEvent.change(screen.getByLabelText("数据库目录"), { target: { value: "D:\\db" } });
  fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\\photos" } });
  fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByText("全部关闭"));
  fireEvent.click(screen.getByRole("button", { name: "下一步" }));
  fireEvent.click(await screen.findByRole("button", { name: "开始使用 Photo Hub" }));
  await screen.findByTestId("gallery-probe");
  expect(ipcMock.mock.calls.some(([cmd]) => cmd === "ai_model_download")).toBe(false);
});
