import { describe, expect, it } from "vitest";

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "./Sidebar";

const NAV_LABELS = ["画廊", "搜索", "导入", "任务", "设置"] as const;

function renderSidebar(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Sidebar />
        <Routes>
          <Route path="/gallery" element={<div>GALLERY_CONTENT</div>} />
          <Route path="/search" element={<div>SEARCH_CONTENT</div>} />
          <Route path="/import" element={<div>IMPORT_CONTENT</div>} />
          <Route path="/tasks" element={<div>TASKS_CONTENT</div>} />
          <Route path="/settings" element={<div>SETTINGS_CONTENT</div>} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

describe("Sidebar", () => {
  it("渲染五个导航项，均为中文文案", () => {
    renderSidebar("/gallery");

    const nav = screen.getByRole("navigation", { name: "primary" });
    const links = within(nav).getAllByRole("link");
    expect(links).toHaveLength(NAV_LABELS.length);

    for (const label of NAV_LABELS) {
      expect(within(nav).getByRole("link", { name: label })).toBeInTheDocument();
    }
  });

  it("当前路由项带激活标记（aria-current=page 与激活样式）", () => {
    renderSidebar("/gallery");

    const active = screen.getByRole("link", { name: "画廊" });
    expect(active).toHaveAttribute("aria-current", "page");
    // 激活态类名含 text-accent（非激活态为 text-text-secondary）
    expect(active.className).toContain("text-accent");

    for (const label of ["搜索", "导入", "任务", "设置"] as const) {
      const link = screen.getByRole("link", { name: label });
      expect(link).not.toHaveAttribute("aria-current");
      expect(link.className).not.toContain("text-accent");
    }
  });

  it("点击导航项跳转到对应路由", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    expect(screen.queryByText("SEARCH_CONTENT")).not.toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: "搜索" }));

    expect(screen.getByText("SEARCH_CONTENT")).toBeInTheDocument();
    // 激活态随路由切换
    expect(screen.getByRole("link", { name: "搜索" })).toHaveAttribute(
      "aria-current",
      "page",
    );
    expect(screen.getByRole("link", { name: "画廊" })).not.toHaveAttribute(
      "aria-current",
    );
  });

  it("侧栏展示应用标识 Smart Photo", () => {
    renderSidebar("/gallery");
    expect(screen.getByText("Smart Photo")).toBeInTheDocument();
  });
});
