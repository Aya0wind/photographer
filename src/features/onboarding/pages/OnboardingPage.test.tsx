import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router";
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
import "@/i18n";
import { DEFAULT_SETTINGS, clone, useSettingsStore } from "@/stores/settingsStore";

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

describe("OnboardingPage 向导", () => {
  beforeEach(() => {
    primeStore();
    ipcMock.mockClear();
    openMock.mockReset();
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
    expect(await screen.findByText("开始使用 Smart Photo")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "开始使用 Smart Photo" }));

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
    fireEvent.click(await screen.findByRole("button", { name: "开始使用 Smart Photo" }));

    await waitFor(() => expect(ipcMock).toHaveBeenCalledWith("settings_set", expect.anything()));
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries).toHaveLength(1);
    expect(settings.libraries[0].id).toBe("lib-x");
    expect(settings.libraries[0].configured).toBe(true);
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
