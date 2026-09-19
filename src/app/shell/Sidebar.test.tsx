import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "./Sidebar";
import { peopleList, type PersonCluster } from "@/ipc/api";

// 人物徽标数据源（默认空清单 → 无徽标，不影响既有用例的精确可访问名断言）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    peopleList: vi.fn(),
  };
});

const peopleListMock = vi.mocked(peopleList);

const NAV_LABELS = ["画廊", "搜索", "导入", "人物", "智能相册", "任务", "设置"] as const;

function renderSidebar(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Sidebar />
        <Routes>
          <Route path="/gallery" element={<div>GALLERY_CONTENT</div>} />
          <Route path="/search" element={<div>SEARCH_CONTENT</div>} />
          <Route path="/people" element={<div>PEOPLE_CONTENT</div>} />
          <Route path="/albums" element={<div>ALBUMS_CONTENT</div>} />
          <Route path="/import" element={<div>IMPORT_CONTENT</div>} />
          <Route path="/tasks" element={<div>TASKS_CONTENT</div>} />
          <Route path="/settings" element={<div>SETTINGS_CONTENT</div>} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

describe("Sidebar", () => {
  beforeEach(() => {
    peopleListMock.mockReset().mockResolvedValue([]);
  });

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

  it("品牌区让位顶部标题栏：侧栏不再展示应用标识", () => {
    renderSidebar("/gallery");
    // 「Smart Photo」品牌已上移整窗顶部 TitleBar（侧栏单独渲染时不可见）
    expect(screen.queryByText("Smart Photo")).not.toBeInTheDocument();
  });

  // --- 人物入口徽标（M4 二轮） -----------------------------------------------------

  it("人物入口显示聚类人脸总数徽标（faceCount 求和）", async () => {
    const people: PersonCluster[] = [
      { clusterId: 0, name: "张三", faceCount: 8, coverAssetId: 1 },
      { clusterId: 1, name: null, faceCount: 4, coverAssetId: 2 },
    ];
    peopleListMock.mockResolvedValue(people);
    renderSidebar("/gallery");

    const badge = await screen.findByTestId("sidebar-people-badge");
    expect(badge).toHaveTextContent("12");
    // 徽标在人物导航项内
    const nav = screen.getByRole("navigation", { name: "primary" });
    expect(within(within(nav).getByRole("link", { name: /人物/ })).getByTestId("sidebar-people-badge")).toBe(badge);
  });

  it("无人脸数据（空清单/后端未就绪）→ 人物入口无徽标", async () => {
    renderSidebar("/gallery");
    await waitFor(() => expect(peopleListMock).toHaveBeenCalled());
    expect(screen.queryByTestId("sidebar-people-badge")).not.toBeInTheDocument();
  });
});
