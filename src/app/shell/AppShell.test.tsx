import { describe, expect, it } from "vitest";

import { act, createEvent, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import AppShell from "./AppShell";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import { clone, DEFAULT_SETTINGS, useSettingsStore } from "@/stores/settingsStore";

/** 设置态复位（外观开关等用例隔离） */
function resetSettingsForShell(): void {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
}

function renderShell(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Routes>
          <Route path="/" element={<AppShell />}>
            <Route path="gallery" element={<div>OUTLET_GALLERY</div>} />
            <Route path="settings" element={<div>OUTLET_SETTINGS</div>} />
          </Route>
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

describe("AppShell", () => {
  it("渲染子路由的 Outlet 内容", () => {
    renderShell("/gallery");

    expect(screen.getByText("OUTLET_GALLERY")).toBeInTheDocument();
    expect(screen.queryByText("OUTLET_SETTINGS")).not.toBeInTheDocument();
  });

  it("侧栏与内容区并存：内容渲染在 main 中，侧栏导航同时可见", () => {
    renderShell("/gallery");

    const main = screen.getByRole("main");
    expect(within(main).getByText("OUTLET_GALLERY")).toBeInTheDocument();

    const nav = screen.getByRole("navigation", { name: "primary" });
    expect(nav).toBeInTheDocument();
    // 侧栏在 main 之外（并排布局的结构保证）
    expect(main).not.toContainElement(nav);
  });

  it("路由切换后 Outlet 内容替换为新页面", async () => {
    renderShell("/gallery");
    expect(screen.getByText("OUTLET_GALLERY")).toBeInTheDocument();

    // 通过壳内侧栏导航切换路由（MemoryRouter 的 location 是内部 state，
    // rerender 新 initialEntries 不会生效，必须真实导航）
    const user = userEvent.setup();
    await user.click(screen.getByRole("link", { name: "设置" }));

    // 页面交叉淡入（popLayout）：过渡期新旧两页并存，退场页 150ms 后卸载
    expect(screen.getByText("OUTLET_SETTINGS")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByText("OUTLET_GALLERY")).not.toBeInTheDocument());
  });

  it("任务抽屉随壳常驻：TitleBar 开关徽标，打开后任意页面可见导入行（M4.5 A2）", async () => {
    resetImportStoreForTests();
    renderShell("/gallery");

    // 浮动进度卡已废除：无 import-card
    expect(screen.queryByTestId("import-card")).not.toBeInTheDocument();
    // 无任务：抽屉内容不显示（面板未开）
    expect(screen.queryByTestId("taskdrawer")).not.toBeInTheDocument();

    act(() => {
      useImportStore.getState().handleAppEvent({
        type: "importSessionStarted",
        jobId: 7,
        totalFiles: 10,
        totalBytes: 1000,
      });
    });

    // 开关徽标出现（运行中 1）
    expect(await screen.findByTestId("taskdrawer-count")).toHaveTextContent("1");

    const user = userEvent.setup();
    await user.click(screen.getByTestId("taskdrawer-toggle"));
    const drawer = await screen.findByTestId("taskdrawer");
    expect(within(drawer).getByText("复制任务 #7")).toBeInTheDocument();
  });
});

// --- M4.5 wave-3：动画开关与快捷键弹窗 ---------------------------------------------------

describe("AppShell：界面动画开关（no-motion）", () => {
  it("默认开：根节点无 no-motion；设置关闭后挂类", () => {
    resetSettingsForShell();
    renderShell("/gallery");
    // 根节点 = AppShell 最外 div（包含 titlebar）
    const root = screen.getByTestId("titlebar").parentElement as HTMLElement;
    expect(root.className).not.toContain("no-motion");

    act(() => {
      useSettingsStore.setState((s) => ({
        settings: { ...s.settings, appearance: { animations: false } },
      }));
    });
    expect(root.className).toContain("no-motion");
  });
});

describe("AppShell：? 键快捷键速查弹窗", () => {
  it("? 键开关弹窗（三组分组渲染）；Esc 关闭", async () => {
    resetSettingsForShell();
    const user = userEvent.setup();
    renderShell("/gallery");

    expect(screen.queryByTestId("shortcuts-modal")).not.toBeInTheDocument();

    fireEvent.keyDown(window, { key: "?" });
    const modal = await screen.findByTestId("shortcuts-modal");
    expect(within(modal).getAllByTestId("shortcuts-group")).toHaveLength(3);
    expect(modal).toHaveTextContent("快捷键");

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByTestId("shortcuts-modal")).not.toBeInTheDocument(),
    );
    void user;
  });

  it("输入框内按 ? 不触发弹窗", async () => {
    resetSettingsForShell();
    renderShell("/gallery");

    const input = await screen.findByTestId("globalsearch-input");
    fireEvent.keyDown(input, { key: "?" });
    await new Promise((r) => setTimeout(r, 30));
    expect(screen.queryByTestId("shortcuts-modal")).not.toBeInTheDocument();
  });
});

// --- 顶部菜单栏移除（①）与原生行为屏蔽（②） --------------------------------------------

describe("AppShell：菜单栏移除与原生行为屏蔽", () => {
  it("顶部菜单栏已移除：无 menubar 结构；标题栏/侧栏仍在", () => {
    renderShell("/gallery");

    expect(screen.queryByTestId("menubar")).not.toBeInTheDocument();
    expect(screen.getByTestId("titlebar")).toBeInTheDocument();
    expect(screen.getByRole("navigation", { name: "primary" })).toBeInTheDocument();
  });

  it("Ctrl+1..5 全局导航快捷键保留（原菜单栏能力迁入主壳）", async () => {
    renderShell("/gallery");
    expect(screen.getByText("OUTLET_GALLERY")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "5", ctrlKey: true });
    expect(await screen.findByText("OUTLET_SETTINGS")).toBeInTheDocument();
  });

  it("全局 contextmenu 被 preventDefault（右键归自定义菜单）", () => {
    renderShell("/gallery");
    const target = screen.getByTestId("titlebar");
    const event = createEvent.contextMenu(target);
    target.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("开发者工具快捷键被屏蔽：F12 / Ctrl+Shift+I/J/C preventDefault", () => {
    renderShell("/gallery");
    for (const init of [
      { key: "F12" },
      { key: "I", ctrlKey: true, shiftKey: true },
      { key: "J", ctrlKey: true, shiftKey: true },
      { key: "C", ctrlKey: true, shiftKey: true },
    ]) {
      const event = createEvent.keyDown(window, { ...init, cancelable: true });
      window.dispatchEvent(event);
      expect(event.defaultPrevented).toBe(true);
    }
  });

  it("业务快捷键不受屏蔽误伤：普通按键/方向键不 preventDefault（? 弹窗照常）", async () => {
    resetSettingsForShell();
    renderShell("/gallery");

    const plain = createEvent.keyDown(window, { key: "a", cancelable: true });
    window.dispatchEvent(plain);
    expect(plain.defaultPrevented).toBe(false);

    const arrow = createEvent.keyDown(window, { key: "ArrowLeft", cancelable: true });
    window.dispatchEvent(arrow);
    expect(arrow.defaultPrevented).toBe(false);

    // ? 仍打开快捷键速查（AppShell 业务监听未被 guard 干掉）
    fireEvent.keyDown(window, { key: "?" });
    expect(await screen.findByTestId("shortcuts-modal")).toBeInTheDocument();
  });
});
