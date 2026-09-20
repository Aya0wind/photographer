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
  dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
  importSubdir: "SmartPhoto",
  configured: true,
streams: 4,
};

/** 本次会话由 NewLibraryDialog 新建、尚未配置完成的空库 */
const FRESH_LIB: Library = {
  id: "lib-fresh",
  name: "新库",
  dbDir: "D:\\db2",
  photoRoot: "D:\\新照片",
  dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
  importSubdir: "SmartPhoto",
  configured: false,
streams: 4,
};

/** 存量未配置库（从选择器点进来补完的老库） */
const OLD_UNCONFIGURED: Library = { ...FRESH_LIB, id: "lib-old", name: "老库" };

describe("OnboardingPage 向导", () => {
  beforeEach(() => {
    primeStore();
    ipcMock.mockClear();
    openMock.mockReset();
    sessionStorage.removeItem(NEW_LIBRARY_DRAFT_KEY);
  });

  it("步骤1 预填本机默认值（主库 / I:\\SmartPhoto\\主库 / Y:\\照片 / SmartPhoto）", () => {
    renderWizard();
    expect((screen.getByLabelText("库名称") as HTMLInputElement).value).toBe("主库");
    expect((screen.getByLabelText("数据库目录") as HTMLInputElement).value).toBe(
      "I:\\SmartPhoto\\主库",
    );
    expect((screen.getByLabelText("照片存储目录") as HTMLInputElement).value).toBe("Y:\\照片");
    expect((screen.getByLabelText("导入子目录") as HTMLInputElement).value).toBe("SmartPhoto");
    expect(screen.getByRole("button", { name: "下一步" })).toBeEnabled();
  });

  it("照片存储目录为空时禁止下一步", () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "" } });
    expect(screen.getByRole("button", { name: "下一步" })).toBeDisabled();
    fireEvent.change(screen.getByLabelText("照片存储目录"), { target: { value: "D:\\照片" } });
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

  it("完整流程：走完四步并以正确负载提交设置", async () => {
    renderWizard();

    // 步骤1 → 步骤2（AnimatePresence mode="wait"：切换有 180ms 退场动画，异步等待）
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    const templateInput = await screen.findByLabelText("目录命名模板");

    // 未知令牌：警告 + 禁止下一步（易错路径覆盖）
    fireEvent.change(templateInput, { target: { value: "{BAD}" } });
    expect(screen.getByRole("alert").textContent).toContain("{BAD}");
    expect(screen.getByRole("button", { name: "下一步" })).toBeDisabled();
    fireEvent.change(templateInput, {
      target: { value: "{YYYY}/{MM-DD}/{原文件名}" },
    });
    expect(screen.getByRole("button", { name: "下一步" })).toBeEnabled();

    // 步骤2 → 步骤3：选"仅语义搜索"
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    fireEvent.click(await screen.findByText("仅语义搜索"));
    expect(screen.getByText("仅语义搜索").closest("button")?.getAttribute("aria-pressed")).toBe(
      "true",
    );

    // 步骤3 → 步骤4：确认摘要后提交
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect(await screen.findByText("开始使用 Photo Hub")).toBeDefined();
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
    expect(libraries[0]["dbDir"]).toBe("I:\\SmartPhoto\\主库");
    expect(libraries[0]["photoRoot"]).toBe("Y:\\照片");
    // 库级导入整理规则（新架构：随库走）
    expect(libraries[0]["dirTemplate"]).toBe("{YYYY}/{MM-DD}/{原文件名}");
    expect(libraries[0]["importSubdir"]).toBe("SmartPhoto");
    expect(libraries[0]["configured"]).toBe(true);
    const ai = settings["ai"] as Record<string, unknown>;
    expect(ai["enableClip"]).toBe(true);
    expect(ai["enableFace"]).toBe(false);
    expect(ai["enableSceneTags"]).toBe(false);
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
            dirTemplate: "{YYYY}/{MM}",
            importSubdir: "Import",
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
    expect((screen.getByLabelText("导入子目录") as HTMLInputElement).value).toBe("Import");

    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect((await screen.findByLabelText("目录命名模板") as HTMLInputElement).value).toBe(
      "{YYYY}/{MM}",
    );
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
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    const templateInput = await screen.findByLabelText("目录命名模板");
    fireEvent.change(templateInput, { target: { value: "{YYYY}/{MM}" } });

    fireEvent.click(screen.getByRole("button", { name: "上一步" }));

    // 回到步骤1：名称草稿保留；再前进：模板草稿保留（draft 常驻内存不重置）
    expect((await screen.findByLabelText("库名称") as HTMLInputElement).value).toBe("旅行库");
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    expect((await screen.findByLabelText("目录命名模板") as HTMLInputElement).value).toBe(
      "{YYYY}/{MM}",
    );
  });

  it("步骤指示器：已完成步可点击跳回，当前/未来步不可点；跳回草稿保留", async () => {
    renderWizard();
    fireEvent.change(screen.getByLabelText("库名称"), { target: { value: "旅行库" } });
    fireEvent.click(screen.getByRole("button", { name: "下一步" }));
    await screen.findByLabelText("目录命名模板");

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
