import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

// ipc mock：settingsStore.save 断言 payload 用
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
const ipcMock = vi.mocked(ipc);

import i18n from "@/i18n";
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
  dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
  importSubdir: "SmartPhoto",
  configured: true,
streams: 4,
};
const LIB_B: Library = {
  id: "lib-2",
  name: "工作库",
  dbDir: "E:\\db2",
  photoRoot: "E:\\照片",
  dirTemplate: "{YYYY}/{MM}",
  importSubdir: "",
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
    expect(screen.getByTestId("settings-language")).toBeDisabled();
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

    // AI：M4 前全部禁用但可见
    await switchTab(user, "ai");
    expect(screen.getByText(/将在 AI 里程碑开放/)).toBeInTheDocument();
    expect(screen.getByLabelText("语义搜索")).toBeDisabled();
    expect(screen.getByLabelText("人脸识别")).toBeDisabled();
    expect(screen.getByLabelText("GPU 加速")).toBeDisabled();
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

  it("当前库信息只读：名称/照片目录/数据库目录/导入收纳区/模板", async () => {
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
    expect(within(current).getByText("D:\\Photos\\SmartPhoto")).toBeInTheDocument();
    expect(within(current).getByText("{YYYY}/{MM-DD}/{原文件名}")).toBeInTheDocument();
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
