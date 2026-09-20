import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "./Sidebar";
import { peopleList, type PersonCluster } from "@/ipc/api";

// 人物徽标数据源（默认空清单 → 无徽标，不影响既有用例的可访问名断言）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    peopleList: vi.fn(),
  };
});

const peopleListMock = vi.mocked(peopleList);

/** 分组 → 导航项（含禁用占位）的期望结构 */
const EXPECTED_SECTIONS: Array<{ section: string; links: string[]; disabled: string[] }> = [
  { section: "浏览", links: ["图库", "最近浏览"], disabled: ["收藏"] },
  { section: "组织", links: ["相册", "人物", "媒体类型", "标签"], disabled: [] },
  { section: "工具", links: ["导入"], disabled: ["相似照片"] },
  { section: "系统", links: ["设置"], disabled: [] },
];

function renderSidebar(initialPath: string) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[initialPath]}>
        <Sidebar />
        <Routes>
          <Route path="/gallery" element={<div>GALLERY_CONTENT</div>} />
          <Route path="/recent" element={<div>RECENT_CONTENT</div>} />
          <Route path="/albums" element={<div>ALBUMS_CONTENT</div>} />
          <Route path="/albums/:tag" element={<div>TAG_CONTENT</div>} />
          <Route path="/people" element={<div>PEOPLE_CONTENT</div>} />
          <Route path="/media" element={<div>MEDIA_CONTENT</div>} />
          <Route path="/import" element={<div>IMPORT_CONTENT</div>} />
          <Route path="/tasks" element={<div>TASKS_CONTENT</div>} />
          <Route path="/settings" element={<div>SETTINGS_CONTENT</div>} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  peopleListMock.mockReset().mockResolvedValue([]);
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

  it("禁用占位（收藏/重复检查）：不可导航 + 「即将支持」小字", () => {
    renderSidebar("/gallery");

    const disabled = screen.getAllByTestId("nav-disabled");
    expect(disabled).toHaveLength(2);
    for (const item of disabled) {
      expect(item).toHaveAttribute("aria-disabled", "true");
      expect(item.querySelector("a")).toBeNull(); // 无链接——不可导航
      expect(within(item).getByTestId("nav-disabled-soon")).toHaveTextContent("即将支持");
    }
  });

  it("当前路由项带激活标记（aria-current=page 与激活样式）", () => {
    renderSidebar("/recent");

    const active = screen.getByRole("link", { name: /最近浏览/ });
    expect(active).toHaveAttribute("aria-current", "page");
    expect(active.className).toContain("text-accent");
    for (const label of ["图库", "导入", "设置"] as const) {
      expect(screen.getByRole("link", { name: new RegExp(label) })).not.toHaveAttribute("aria-current");
    }
  });

  it("点击导航项跳转对应路由（含新路由 /recent 与 /media）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    await user.click(screen.getByRole("link", { name: /最近浏览/ }));
    expect(screen.getByText("RECENT_CONTENT")).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: /媒体类型/ }));
    expect(screen.getByText("MEDIA_CONTENT")).toBeInTheDocument();

    await user.click(screen.getByRole("link", { name: /相册/ }));
    expect(screen.getByText("ALBUMS_CONTENT")).toBeInTheDocument();
  });

  it("「标签」入口指向 /albums#tags（与相册同路由锚点）", async () => {
    renderSidebar("/gallery");
    const user = userEvent.setup();

    const tagsLink = screen.getByRole("link", { name: /^标签$/ });
    expect(tagsLink).toHaveAttribute("href", "/albums#tags");
    await user.click(tagsLink);
    expect(screen.getByText("ALBUMS_CONTENT")).toBeInTheDocument();
  });

  it("品牌区让位顶部标题栏：侧栏不再展示应用标识", () => {
    renderSidebar("/gallery");
    expect(screen.queryByText("Smart Photo")).not.toBeInTheDocument();
  });

  // --- 人物入口徽标（保留） ----------------------------------------------------------

  it("人物入口显示聚类人脸总数徽标（faceCount 求和）", async () => {
    const people: PersonCluster[] = [
      { clusterId: 0, name: "张三", faceCount: 8, coverAssetId: 1 },
      { clusterId: 1, name: null, faceCount: 4, coverAssetId: 2 },
    ];
    peopleListMock.mockResolvedValue(people);
    renderSidebar("/gallery");

    const badge = await screen.findByTestId("sidebar-people-badge");
    expect(badge).toHaveTextContent("12");
    const nav = screen.getByRole("navigation", { name: "primary" });
    expect(within(within(nav).getByRole("link", { name: /人物/ })).getByTestId("sidebar-people-badge")).toBe(badge);
  });

  it("无人脸数据（空清单/后端未就绪）→ 人物入口无徽标", async () => {
    renderSidebar("/gallery");
    await waitFor(() => expect(peopleListMock).toHaveBeenCalled());
    expect(screen.queryByTestId("sidebar-people-badge")).not.toBeInTheDocument();
  });
});
