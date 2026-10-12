import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "./Sidebar";
import {
  peopleList,
  sidebarCounts,
  subscribeAppEvents,
} from "@/ipc/api";

// 保留计数接口桩，验证导航不再发起相关查询。
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    peopleList: vi.fn(),
    sidebarCounts: vi.fn(),
    subscribeAppEvents: vi.fn(),
  };
});

const peopleListMock = vi.mocked(peopleList);
const sidebarCountsMock = vi.mocked(sidebarCounts);
const subscribeAppEventsMock = vi.mocked(subscribeAppEvents);

/** 分组 → 导航项（含禁用占位）的期望结构 */
const EXPECTED_SECTIONS: Array<{ section: string; links: string[]; disabled: string[] }> = [
  { section: "浏览", links: ["图库", "最近浏览"], disabled: [] },
  { section: "组织", links: ["相册", "那年今天", "人物", "器材统计"], disabled: [] },
  { section: "工具", links: ["添加照片", "相似照片", "回收站"], disabled: [] },
  { section: "系统", links: ["照片库", "设置"], disabled: [] },
];

function renderSidebar(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Sidebar />
        <Routes>
          <Route path="/gallery" element={<div>GALLERY_CONTENT</div>} />
          <Route path="/recent" element={<div>RECENT_CONTENT</div>} />
          <Route path="/trash" element={<div>TRASH_CONTENT</div>} />
          <Route path="/memories" element={<div>MEMORIES_CONTENT</div>} />
          <Route path="/gear" element={<div>GEAR_CONTENT</div>} />
          <Route path="/similar" element={<div>SIMILAR_CONTENT</div>} />
          <Route path="/albums" element={<div>ALBUMS_CONTENT</div>} />
          <Route path="/albums/:tag" element={<div>TAG_CONTENT</div>} />
          <Route path="/people" element={<div>PEOPLE_CONTENT</div>} />
          <Route path="/import" element={<div>IMPORT_CONTENT</div>} />
          <Route path="/tasks" element={<div>TASKS_CONTENT</div>} />
          <Route path="/storage" element={<div>STORAGE_CONTENT</div>} />
          <Route path="/settings" element={<div>SETTINGS_CONTENT</div>} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  peopleListMock.mockReset().mockResolvedValue([]);
  sidebarCountsMock.mockReset().mockResolvedValue(null);
  subscribeAppEventsMock.mockReset().mockResolvedValue(() => {});
});

describe("Sidebar（M4.5 A3 分组信息架构）", () => {
  it("四组渲染（浏览/组织/工具/系统）；搜索入口已移除", () => {
    renderSidebar("/gallery");

    const nav = screen.getByRole("navigation", { name: "primary" });
    const sections = within(nav).getAllByTestId("nav-section");
    expect(sections.map((s) => s.getAttribute("data-section"))).toEqual(
      EXPECTED_SECTIONS.map((s) => `nav.section.${s.section === "浏览" ? "browse" : s.section === "组织" ? "organize" : s.section === "工具" ? "tools" : "system"}`),
    );

    for (const { section, links, disabled } of EXPECTED_SECTIONS) {
      const el = sections.find((s) => s.textContent?.includes(section));
      expect(el).toBeDefined();
      for (const label of links) {
        expect(within(el as HTMLElement).getByRole("link", { name: new RegExp(label) })).toBeInTheDocument();
      }
      for (const label of disabled) {
        expect(within(el as HTMLElement).getAllByTestId("nav-disabled").map((d) => d.textContent)).toContainEqual(expect.stringContaining(label));
      }
    }

    // 搜索已移除（全局搜索框承担）
    expect(screen.queryByRole("link", { name: "搜索" })).not.toBeInTheDocument();
  });

  it("收藏不再占用侧栏入口", () => {
    renderSidebar("/gallery");
    expect(screen.queryByTestId("nav-disabled")).not.toBeInTheDocument();
  });

  it("当前路由项带激活标记（aria-current=page 与激活样式）", () => {
    renderSidebar("/recent");

    const active = screen.getByRole("link", { name: /最近浏览/ });
    expect(active).toHaveAttribute("aria-current", "page");
    expect(active.className).toContain("text-accent");
    for (const label of ["图库", "添加照片", "设置"] as const) {
      expect(screen.getByRole("link", { name: new RegExp(label) })).not.toHaveAttribute("aria-current");
    }
  });

  it("点击导航项跳转对应路由", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    await user.click(screen.getByRole("link", { name: /最近浏览/ }));
    expect(screen.getByText("RECENT_CONTENT")).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: /相册/ }));
    expect(screen.getByText("ALBUMS_CONTENT")).toBeInTheDocument();
  });

  it("M7 新入口：那年今天→/memories（浏览组）、器材统计→/gear（组织组）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    const memories = screen.getByRole("link", { name: /那年今天/ });
    const gear = screen.getByRole("link", { name: /器材统计/ });
    const browse = screen.getAllByTestId("nav-section").find((s) => s.textContent?.includes("浏览"));
    const organize = screen.getAllByTestId("nav-section").find((s) => s.textContent?.includes("组织"));
    expect(browse).not.toContain(memories);
    expect(organize).toContain(memories);
    expect(organize).toContain(gear);

    await user.click(memories);
    expect(screen.getByText("MEMORIES_CONTENT")).toBeInTheDocument();
    await user.click(gear);
    expect(screen.getByText("GEAR_CONTENT")).toBeInTheDocument();
  });

  it("B1 回收站入口（工具组，指 /trash，无计数徽标）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    const trash = screen.getByRole("link", { name: /回收站/ });
    const tools = screen.getAllByTestId("nav-section").find((s) => s.textContent?.includes("工具"));
    expect(tools).toContain(trash);
    expect(trash).toHaveAttribute("href", "/trash");
    expect(within(trash).queryByTestId("sidebar-count-badge")).not.toBeInTheDocument();

    await user.click(trash);
    expect(screen.getByText("TRASH_CONTENT")).toBeInTheDocument();
  });

  it("F8 相似照片入口已启用（工具组，指 /similar，不再是占位禁用）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    const similar = screen.getByRole("link", { name: /相似照片/ });
    const tools = screen.getAllByTestId("nav-section").find((s) => s.textContent?.includes("工具"));
    expect(tools).toContain(similar);
    expect(similar).toHaveAttribute("href", "/similar");
    expect(similar).not.toHaveAttribute("aria-disabled");

    await user.click(similar);
    expect(screen.getByText("SIMILAR_CONTENT")).toBeInTheDocument();
  });

  it("M3 存储入口（系统组，指 /storage，照片库登记管理）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    const storage = screen.getByRole("link", { name: /照片库/ });
    const system = screen.getAllByTestId("nav-section").find((s) => s.textContent?.includes("系统"));
    expect(system).toContain(storage);
    expect(storage).toHaveAttribute("href", "/storage");

    await user.click(storage);
    expect(screen.getByText("STORAGE_CONTENT")).toBeInTheDocument();
  });

  it("品牌区让位顶部标题栏：侧栏不再展示应用标识", () => {
    renderSidebar("/gallery");
    expect(screen.queryByText("Photographer")).not.toBeInTheDocument();
  });


});


// --- 侧栏计数徽标（⑥：sidebar_counts 契约铺开） -----------------------------------------


it("导航不显示计数且不请求计数数据",()=>{renderSidebar("/gallery");expect(screen.queryByTestId("sidebar-count-badge")).not.toBeInTheDocument();expect(screen.queryByTestId("sidebar-people-badge")).not.toBeInTheDocument();expect(sidebarCountsMock).not.toHaveBeenCalled();expect(peopleListMock).not.toHaveBeenCalled();});
