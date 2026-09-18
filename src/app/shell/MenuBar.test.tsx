import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import MenuBar from "./MenuBar";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
} from "@/stores/settingsStore";

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(),
}));

const getCurrentWindowMock = vi.mocked(getCurrentWindow);

function GalleryProbe() {
  return <div data-testid="gallery-probe" />;
}

function ImportProbe() {
  return <div data-testid="import-probe" />;
}

function TasksProbe() {
  return <div data-testid="tasks-probe" />;
}

function SettingsProbe() {
  return <div data-testid="settings-probe" />;
}

function PickerProbe() {
  return <div data-testid="picker-probe" />;
}

function OnboardingProbe() {
  return <div data-testid="onboarding-probe">ONBOARDING</div>;
}

function renderMenu(initialPath = "/gallery") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <MenuBar />
        <Routes>
          <Route path="/gallery" element={<GalleryProbe />} />
          <Route path="/import" element={<ImportProbe />} />
          <Route path="/tasks" element={<TasksProbe />} />
          <Route path="/settings" element={<SettingsProbe />} />
          <Route path="/library-picker" element={<PickerProbe />} />
          <Route path="/onboarding" element={<OnboardingProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  getCurrentWindowMock.mockReset();
  getCurrentWindowMock.mockImplementation(
    () =>
      ({
        minimize: vi.fn().mockResolvedValue(undefined),
        toggleMaximize: vi.fn().mockResolvedValue(undefined),
        close: vi.fn().mockResolvedValue(undefined),
        isMaximized: vi.fn().mockResolvedValue(false),
        onResized: vi.fn().mockResolvedValue(() => {}),
      }) as unknown as ReturnType<typeof getCurrentWindow>,
  );
  useSettingsStore.setState({ settings: clone(DEFAULT_SETTINGS), loaded: true });
});

describe("菜单栏交互（工业软件惯例）", () => {
  it("四个顶级菜单；点击打开下拉，再点关闭", async () => {
    renderMenu();
    const user = userEvent.setup();

    for (const label of ["文件", "查看", "工具", "帮助"] as const) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }

    await user.click(screen.getByRole("button", { name: "文件" }));
    expect(screen.getByTestId("menu-panel-file")).toBeInTheDocument();
    expect(screen.getByTestId("menu-item-newLibrary")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "文件" }));
    await waitFor(() =>
      expect(screen.queryByTestId("menu-panel-file")).not.toBeInTheDocument(),
    );
  });

  it("已打开时 hover 相邻菜单直接切换", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    expect(screen.getByTestId("menu-panel-file")).toBeInTheDocument();

    fireEvent.mouseEnter(screen.getByRole("button", { name: "查看" }));
    await waitFor(() => {
      expect(screen.queryByTestId("menu-panel-file")).not.toBeInTheDocument();
    });
    expect(await screen.findByTestId("menu-panel-view")).toBeInTheDocument();
  });

  it("点击外部关闭；Escape 关闭", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    expect(screen.getByTestId("menu-panel-file")).toBeInTheDocument();

    // 点击菜单栏外部（body 追加的节点）
    const outside = document.createElement("button");
    document.body.appendChild(outside);
    fireEvent.mouseDown(outside);
    await waitFor(() =>
      expect(screen.queryByTestId("menu-panel-file")).not.toBeInTheDocument(),
    );
    outside.remove();

    // Escape
    await user.click(screen.getByRole("button", { name: "帮助" }));
    expect(screen.getByTestId("menu-panel-help")).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByTestId("menu-panel-help")).not.toBeInTheDocument(),
    );
  });

  it("禁用菜单项：帮助→关于 Smart Photo 不可点击", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "帮助" }));
    const about = screen.getByTestId("menu-item-about");
    expect(about).toBeDisabled();
    await user.click(about);
    // 面板仍开（未触发动作）
    expect(screen.getByTestId("menu-panel-help")).toBeInTheDocument();
  });

  it("菜单项带快捷键提示位（右对齐灰色，仅展示）", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    const item = screen.getByTestId("menu-item-newLibrary");
    expect(item).toHaveTextContent("新建库…");
    expect(item).toHaveTextContent("Ctrl+N");
    expect(within(item).getByText("Ctrl+N").className).toContain("text-text-muted");
  });
});

describe("菜单导航", () => {
  it("文件→导入照片… 导航 /import 并关闭菜单", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    await user.click(screen.getByTestId("menu-item-importPhotos"));

    expect(await screen.findByTestId("import-probe")).toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByTestId("menu-panel-file")).not.toBeInTheDocument(),
    );
  });

  it("文件→打开库… 导航 /library-picker", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    await user.click(screen.getByTestId("menu-item-openLibrary"));

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
  });

  it("查看菜单勾选当前页（画廊），切换后勾选跟随", async () => {
    renderMenu("/gallery");
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "查看" }));
    const gallery = screen.getByTestId("menu-item-gallery");
    expect(gallery).toBeEnabled();
    // 勾选位渲染 ✓（画廊当前页）
    expect(within(gallery).getByText("画廊")).toBeInTheDocument();
    expect(gallery.textContent).toContain("画廊");
    // 其他页不勾选：勾选标记数量为 1
    const panel = screen.getByTestId("menu-panel-view");
    const checks = panel.querySelectorAll("svg");
    expect(checks.length).toBe(1);

    await user.click(screen.getByTestId("menu-item-settings"));
    expect(await screen.findByTestId("settings-probe")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "查看" }));
    expect(screen.getByTestId("menu-panel-view").querySelectorAll("svg").length).toBe(1);
  });

  it("工具→任务中心 / 导入日志 均到 /tasks", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "工具" }));
    expect(screen.getByRole("separator")).toBeInTheDocument();
    await user.click(screen.getByTestId("menu-item-taskCenter"));
    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
  });

  it("文件→退出 调 window.close()（后端按 closeToTray 决定）", async () => {
    const win = {
      minimize: vi.fn().mockResolvedValue(undefined),
      toggleMaximize: vi.fn().mockResolvedValue(undefined),
      close: vi.fn().mockResolvedValue(undefined),
      isMaximized: vi.fn().mockResolvedValue(false),
      onResized: vi.fn().mockResolvedValue(() => {}),
    };
    getCurrentWindowMock.mockImplementation(
      () => win as unknown as ReturnType<typeof getCurrentWindow>,
    );
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    await user.click(screen.getByTestId("menu-item-quit"));

    expect(win.close).toHaveBeenCalledTimes(1);
  });
});

describe("新建库对话框（与设置页共用）", () => {
  it("文件→新建库… 打开可复用对话框，创建后进向导补完", async () => {
    renderMenu();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "文件" }));
    await user.click(screen.getByTestId("menu-item-newLibrary"));

    const dialog = await screen.findByTestId("new-library-dialog");
    expect(within(dialog).getByLabelText("库名称")).toHaveValue("主库");
    expect(within(dialog).getByTestId("new-library-submit")).toBeInTheDocument();

    await user.click(within(dialog).getByTestId("new-library-submit"));

    // 创建 configured=false 的库并激活，随即进入 /onboarding?library=<id> 补完
    expect(await screen.findByTestId("onboarding-probe")).toBeInTheDocument();
    const settings = useSettingsStore.getState().settings;
    expect(settings.libraries).toHaveLength(1);
    expect(settings.libraries[0].configured).toBe(false);
    expect(settings.activeLibraryId).toBe(settings.libraries[0].id);
    await waitFor(() =>
      expect(screen.queryByTestId("new-library-dialog")).not.toBeInTheDocument(),
    );
  });
});
