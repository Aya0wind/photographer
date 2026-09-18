import { beforeEach, describe, expect, it } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

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
};
const LIB_B: Library = {
  id: "lib-2",
  name: "工作库",
  dbDir: "E:\\db2",
  photoRoot: "E:\\照片",
  dirTemplate: "{YYYY}/{MM}",
  importSubdir: "",
  configured: true,
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

beforeEach(() => {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
});

describe("SettingsPage 库管理区", () => {
  it("无激活库时信息为空，仍有前往选择器入口", () => {
    renderSettingsPage();

    expect(screen.getByText("设置")).toBeInTheDocument();
    expect(screen.queryByText("主库")).not.toBeInTheDocument();
    expect(screen.queryByText("D:\\Photos")).not.toBeInTheDocument();
    expect(screen.getByTestId("settings-goto-picker")).toBeInTheDocument();
  });

  it("当前库信息只读：名称/照片目录/数据库目录/导入收纳区/模板", () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB_A], activeLibraryId: "lib-1" },
    }));
    renderSettingsPage();

    const current = screen.getByTestId("settings-current-library");
    expect(within(current).getByText("主库")).toBeInTheDocument();
    expect(within(current).getByText("D:\\Photos")).toBeInTheDocument();
    expect(within(current).getByText("D:\\SmartPhoto\\db")).toBeInTheDocument();
    expect(within(current).getByText("D:\\Photos\\SmartPhoto")).toBeInTheDocument();
    expect(within(current).getByText("{YYYY}/{MM-DD}/{原文件名}")).toBeInTheDocument();
  });

  it("库列表渲染并高亮激活库（切换统一走选择器，不在原地切换）", () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB_A, LIB_B], activeLibraryId: "lib-1" },
    }));
    renderSettingsPage();

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

    await user.click(screen.getByTestId("settings-goto-picker"));

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
  });
});
