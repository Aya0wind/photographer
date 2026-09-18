import { describe, expect, it } from "vitest";

import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import AppShell from "./AppShell";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";

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

    // AnimatePresence mode="wait"：旧页退出动画（180ms）完成后才挂载新页，需异步等待
    expect(
      await screen.findByText("OUTLET_SETTINGS", {}, { timeout: 2000 }),
    ).toBeInTheDocument();
    expect(screen.queryByText("OUTLET_GALLERY")).not.toBeInTheDocument();
  });

  it("全局导入进度卡随壳挂载：有活跃任务时任意页面可见", async () => {
    resetImportStoreForTests();
    renderShell("/gallery");

    expect(screen.queryByTestId("import-card")).not.toBeInTheDocument();
    act(() => {
      useImportStore.getState().handleAppEvent({
        type: "importSessionStarted",
        jobId: 7,
        totalFiles: 10,
        totalBytes: 1000,
      });
    });

    const card = await screen.findByTestId("import-card");
    expect(card).toBeInTheDocument();
    expect(within(card).getByText("复制任务 #7")).toBeInTheDocument();
  });
});
