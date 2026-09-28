import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import Sidebar from "./Sidebar";
import {
  peopleList,
  sidebarCounts,
  subscribeAppEvents,
  type AppEvent,
  type PersonCluster,
} from "@/ipc/api";

// 人物徽标数据源（默认空清单 → 无徽标，不影响既有用例的可访问名断言）；
// 侧栏计数（sidebar_counts）默认 null → 无徽标；事件订阅捕获 handler 供用例驱动
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

/** 捕获的事件 handler（importSessionFinished 重拉用例驱动） */
let eventHandlers: Array<(event: AppEvent) => void> = [];

/** 分组 → 导航项（含禁用占位）的期望结构 */
const EXPECTED_SECTIONS: Array<{ section: string; links: string[]; disabled: string[] }> = [
  { section: "浏览", links: ["图库", "最近浏览"], disabled: [] },
  { section: "组织", links: ["相册", "那年今天", "人物", "器材统计"], disabled: [] },
  { section: "工具", links: ["导入", "相似照片", "回收站"], disabled: [] },
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
          <Route path="/trash" element={<div>TRASH_CONTENT</div>} />
          <Route path="/memories" element={<div>MEMORIES_CONTENT</div>} />
          <Route path="/gear" element={<div>GEAR_CONTENT</div>} />
          <Route path="/similar" element={<div>SIMILAR_CONTENT</div>} />
          <Route path="/albums" element={<div>ALBUMS_CONTENT</div>} />
          <Route path="/albums/:tag" element={<div>TAG_CONTENT</div>} />
          <Route path="/people" element={<div>PEOPLE_CONTENT</div>} />
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
  sidebarCountsMock.mockReset().mockResolvedValue(null);
  eventHandlers = [];
  subscribeAppEventsMock.mockReset().mockImplementation(async (handler) => {
    eventHandlers.push(handler);
    return () => {};
  });
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
    for (const label of ["图库", "导入", "设置"] as const) {
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

  it("品牌区让位顶部标题栏：侧栏不再展示应用标识", () => {
    renderSidebar("/gallery");
    expect(screen.queryByText("Photo Hub")).not.toBeInTheDocument();
  });

  // --- 人物入口徽标（保留） ----------------------------------------------------------

  it("人物入口显示人物数量徽标（person 条目数，非 faceCount 求和）", async () => {
    const people: PersonCluster[] = [
      { clusterId: 0, name: "张三", faceCount: 8, coverAssetId: 1 },
      { clusterId: 1, name: null, faceCount: 4, coverAssetId: 2 },
    ];
    peopleListMock.mockResolvedValue(people);
    renderSidebar("/gallery");

    const badge = await screen.findByTestId("sidebar-people-badge");
    expect(badge).toHaveTextContent("2");
    const nav = screen.getByRole("navigation", { name: "primary" });
    expect(within(within(nav).getByRole("link", { name: /人物/ })).getByTestId("sidebar-people-badge")).toBe(badge);
  });

  it("无人脸数据（空清单/后端未就绪）→ 人物入口无徽标", async () => {
    renderSidebar("/gallery");
    await waitFor(() => expect(peopleListMock).toHaveBeenCalled());
    expect(screen.queryByTestId("sidebar-people-badge")).not.toBeInTheDocument();
  });
});


// --- 侧栏计数徽标（⑥：sidebar_counts 契约铺开） -----------------------------------------

describe("Sidebar：导航计数徽标", () => {
  it("各入口计数渲染：图库/最近浏览/那年今天/相册 + 人物照旧；0 不显示", async () => {
    peopleListMock.mockResolvedValue([
      { clusterId: 1, name: null, faceCount: 93, coverAssetId: 1 },
    ]);
    sidebarCountsMock.mockResolvedValue({
      assets: 1234,
      recentViewed: 5,
      onThisDay: 0, // 0 不显示
      tags: 7,
      albums: 40,
    });
    renderSidebar("/gallery");

    await waitFor(() =>
      expect(screen.getAllByTestId("sidebar-count-badge").length).toBeGreaterThan(0),
    );
    const badges = screen.getAllByTestId("sidebar-count-badge");
    const byKind = new Map(badges.map((b) => [b.getAttribute("data-kind"), b.textContent]));
    expect(byKind.get("assets")).toBe("1234"); // 图库
    expect(byKind.get("recentViewed")).toBe("5"); // 最近浏览
    expect(byKind.has("tags")).toBe(false); // 标签无独立导航入口
    expect(byKind.get("albums")).toBe("40"); // 相册
    expect(byKind.has("onThisDay")).toBe(false); // 0 → 不渲染
    // 人物徽标照旧（peopleList 数据源；1 个 person → 1）
    expect(await screen.findByTestId("sidebar-people-badge")).toHaveTextContent("1");

    // 徽标挂在对应导航行内（图库行）
    const galleryLink = screen.getByRole("link", { name: /图库/ });
    expect(within(galleryLink).getByTestId("sidebar-count-badge")).toHaveTextContent("1234");
  });

  it("sidebar_counts 不可用（null）：不显示任何计数徽标", async () => {
    sidebarCountsMock.mockResolvedValue(null);
    renderSidebar("/gallery");

    await screen.findAllByRole("link");
    expect(screen.queryByTestId("sidebar-count-badge")).not.toBeInTheDocument();
  });

  it("导入会话完成事件 → 重拉计数（挂载一次 + 事件一次）", async () => {
    sidebarCountsMock
      .mockResolvedValueOnce(null)
      .mockResolvedValue({ assets: 42, recentViewed: 3, onThisDay: 1, tags: 2, albums: 4 });
    renderSidebar("/gallery");
    expect(screen.queryByTestId("sidebar-count-badge")).not.toBeInTheDocument();
    expect(sidebarCountsMock).toHaveBeenCalledTimes(1);

    act(() => {
      for (const handler of eventHandlers) {
        handler({
          type: "importSessionFinished",
          jobId: 1,
          stats: {
            totalFiles: 1,
            doneFiles: 1,
            skippedDuplicates: 0,
            failedFiles: 0,
            totalBytes: 1,
            doneBytes: 1,
            elapsedMs: 1,
            bytesPerSec: 1,
            moved: 0,
            sourceDeleteFailed: 0,
          },
        });
      }
    });

    await waitFor(() => expect(sidebarCountsMock).toHaveBeenCalledTimes(2));
    await waitFor(() => {
      const assets = screen
        .getAllByTestId("sidebar-count-badge")
        .find((b) => b.getAttribute("data-kind") === "assets");
      expect(assets).toHaveTextContent("42");
    });
  });
});
