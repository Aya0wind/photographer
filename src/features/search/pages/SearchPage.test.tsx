import { beforeAll, beforeEach, afterEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import SearchPage, { dateToRfc3339, quickRange } from "./SearchPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  assetThumbGet,
  assetsByIds,
  assetsPage,
  cameraList,
  indexKickNow,
  indexStatus,
  searchSemantic,
  type AssetDto,
} from "@/ipc/api";
import { useAiStore } from "@/stores/aiStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetThumbGet: vi.fn(),
    cameraList: vi.fn(),
    searchSemantic: vi.fn(),
    assetsByIds: vi.fn(),
    indexStatus: vi.fn(),
    indexKickNow: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const assetsPageMock = vi.mocked(assetsPage);
const thumbMock = vi.mocked(assetThumbGet);
const cameraListMock = vi.mocked(cameraList);
const convertMock = vi.mocked(convertFileSrc);
const indexStatusMock = vi.mocked(indexStatus);
const indexKickNowMock = vi.mocked(indexKickNow);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(id: number, date: string | null, kind: AssetDto["kind"] = "photo"): AssetDto {
  const ext = kind === "raw" ? "CR3" : kind === "video" ? "MP4" : "JPG";
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\2026\\IMG_${String(id).padStart(4, "0")}.${ext}`,
    name: `IMG_${String(id).padStart(4, "0")}.${ext}`,
    kind,
    capturedAt: date === null ? null : `${date}T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
  };
}

function SettingsProbe() {
  return <div data-testid="settings-probe">SETTINGS</div>;
}

function renderSearch() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/search"]}>
        <Routes>
          <Route path="/search" element={<SearchPage />} />
          <Route path="/settings" element={<SettingsProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// IntersectionObserver stub（jsdom 缺失；本套件不触发哨兵，仅防挂载报错）
beforeAll(() => {
  (window as unknown as Record<string, unknown>).IntersectionObserver = class {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  };
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  vi.useRealTimers();
  localStorage.clear();
  assetsPageMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue(null);
  convertMock.mockReset().mockReturnValue("");
  cameraListMock.mockReset().mockResolvedValue([]);
  vi.mocked(searchSemantic).mockReset();
  vi.mocked(assetsByIds).mockReset().mockResolvedValue([]);
  indexStatusMock.mockReset().mockResolvedValue(null);
  indexKickNowMock.mockReset().mockResolvedValue(undefined);
  useAiStore.getState().resetForTests();
  resetThumbPipelineForTests();
});

afterEach(() => {
  vi.useRealTimers();
});

// --- 条件负载组装 -------------------------------------------------------------------

describe("搜索：filters 负载组装（camelCase 平铺）", () => {
  it("初始无条件查询：assetsPage(0, 100, {})", async () => {
    renderSearch();

    await screen.findByTestId("search-page");
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100, {}));
  });

  it("类型+日期范围+相机勾选组合成完整 filters（防抖后单次查询）", async () => {
    vi.useFakeTimers();
    cameraListMock.mockResolvedValue([
      { camera: "Canon EOS R5", count: 12 },
      { camera: "Apple iPhone 15", count: 3 },
    ]);
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-01-15", "raw")]);
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    // 组合条件：照片档（=photo+raw）+ 日期范围（RFC3339 本地日界）+ 相机勾选
    fireEvent.click(screen.getByTestId("search-kind-photo"));
    fireEvent.change(screen.getByTestId("search-from"), { target: { value: "2026-01-01" } });
    fireEvent.change(screen.getByTestId("search-to"), { target: { value: "2026-02-01" } });
    fireEvent.click(screen.getByTestId("search-camera-button"));
    const menu = screen.getByTestId("search-camera-menu");
    const options = within(menu).getAllByTestId("search-camera-option");
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveAttribute("data-camera", "Canon EOS R5");
    expect(options[0]).toHaveTextContent("12");
    fireEvent.click(within(options[0]).getByRole("checkbox"));

    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });

    // fake timers 下 waitFor 不推进：advanceTimersByTimeAsync 已同刷微任务，直接断言
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      kinds: ["photo", "raw"],
      capturedAfter: dateToRfc3339("2026-01-01"),
      capturedBefore: dateToRfc3339("2026-02-01", true),
      camera: "Canon EOS R5",
    });
    // 防抖后只有两查询：初始 + 组合条件（中间态不发起）
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
  });

  it("类型两档映射 kinds：照片=photo+raw、视频=video、全部=不传", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    fireEvent.click(screen.getByTestId("search-kind-photo"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { kinds: ["photo", "raw"] });

    fireEvent.click(screen.getByTestId("search-kind-video"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { kinds: ["video"] });

    fireEvent.click(screen.getByTestId("search-kind-all"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {});
  });

  it("日期快捷段：负载为 RFC3339 端点（本地日界转 UTC，后端 parse_from_rfc3339 可解析）", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });

    fireEvent.click(screen.getByTestId("search-quick-recent7"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    const [r7From, r7To] = quickRange("recent7");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: dateToRfc3339(r7From),
      capturedBefore: dateToRfc3339(r7To, true),
    });

    fireEvent.click(screen.getByTestId("search-quick-lastYear"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(300);
    });
    const [lyFrom, lyTo] = quickRange("lastYear");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: dateToRfc3339(lyFrom),
      capturedBefore: dateToRfc3339(lyTo, true),
    });
  });
});

// --- 防抖 -------------------------------------------------------------------------

describe("搜索：300ms 防抖", () => {
  it("连续变更只查一次（299ms 内不查，300ms 后以最终值查）", async () => {
    vi.useFakeTimers();
    renderSearch();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    // 快速连点快捷段（条件连续变更）
    fireEvent.click(screen.getByTestId("search-quick-recent7"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.click(screen.getByTestId("search-quick-recent30"));
    await act(async () => {
      await vi.advanceTimersByTimeAsync(100);
    });
    fireEvent.click(screen.getByTestId("search-quick-thisYear"));
    // 最后一击后 299ms：仍未查询
    await act(async () => {
      await vi.advanceTimersByTimeAsync(299);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(1);

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(assetsPageMock).toHaveBeenCalledTimes(2);
    const [thisFrom, thisTo] = quickRange("thisYear");
    expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {
      capturedAfter: dateToRfc3339(thisFrom),
      capturedBefore: dateToRfc3339(thisTo, true),
    });
    // 无任何中间态查询
    const withDate = assetsPageMock.mock.calls.filter(
      (call) => (call[2] as { capturedAfter?: string }).capturedAfter,
    );
    expect(withDate).toHaveLength(1);
  });
});

// --- 相机勾选下拉 -------------------------------------------------------------------

describe("搜索：相机勾选（cameraList 清单）", () => {
  it("「全部相机」默认态；列表渲染计数徽标；勾选过滤、多选先传第一个、清空回无条件", async () => {
    cameraListMock.mockResolvedValue([
      { camera: "Canon EOS R5", count: 12 },
      { camera: "Apple iPhone 15", count: 3 },
    ]);
    renderSearch();
    await screen.findByTestId("search-page");
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 100, {}));

    // 默认按钮=全部相机；打开下拉
    expect(screen.getByTestId("search-camera-button")).toHaveTextContent("全部相机");
    fireEvent.click(screen.getByTestId("search-camera-button"));
    const options = screen.getAllByTestId("search-camera-option");
    expect(options).toHaveLength(2);
    expect(options[0]).toHaveAttribute("data-camera", "Canon EOS R5");
    expect(options[0]).toHaveTextContent("12");
    expect(options[1]).toHaveTextContent("Apple iPhone 15");
    expect(options[1]).toHaveTextContent("3");

    // 勾选 Canon → 过滤
    fireEvent.click(within(options[0]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon EOS R5" }),
    );

    // 再勾 iPhone：多选 OR（后端数组契约未到位 → 仍传第一个）
    fireEvent.click(within(options[1]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Canon EOS R5" }),
    );

    // 取消 Canon → 剩 iPhone 成为第一个
    fireEvent.click(within(options[0]).getByRole("checkbox"));
    await waitFor(() =>
      expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, { camera: "Apple iPhone 15" }),
    );

    // 全部取消 → 回无条件
    fireEvent.click(within(options[1]).getByRole("checkbox"));
    await waitFor(() => expect(assetsPageMock).toHaveBeenLastCalledWith(0, 100, {}));
    expect(screen.getByTestId("search-camera-button")).toHaveTextContent("全部相机");
  });

  it("相机清单为空：下拉显示空态文案（不过滤）", async () => {
    renderSearch();
    await waitFor(() => expect(cameraListMock).toHaveBeenCalled());

    fireEvent.click(screen.getByTestId("search-camera-button"));
    expect(screen.getByTestId("search-camera-menu")).toHaveTextContent("库内还没有相机信息");
  });
});

// --- 结果渲染 ---------------------------------------------------------------------

describe("搜索：结果与状态", () => {
  it("结果复用画廊网格（同一虚拟化+缩略图管线）：分组/瓦片/计数徽标/内容区居中", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(1, "2026-09-18"),
      makeAsset(2, "2026-09-18"),
      makeAsset(3, "2026-09-17"),
    ]);
    renderSearch();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(3);
    expect(screen.getByTestId("search-count")).toHaveTextContent("3 个结果");
    // 复用 AssetGrid 的组头与滚动容器；内容区与画廊同款居中容器
    expect(screen.getAllByTestId("gallery-group")).toHaveLength(2);
    expect(screen.getByTestId("search-grid-scroll")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-chips")).not.toBeInTheDocument();
    const content = screen.getByTestId("search-content");
    expect(content.className).toContain("mx-auto");
    expect(content.className).toContain("max-w-[1600px]");
    expect(content.className).toContain("px-6");
  });

  it("RAW+JPG 合并展示与画廊同开关：pairId 成对合并 + RAW+JPG 角标", async () => {
    assetsPageMock.mockResolvedValue([
      { ...makeAsset(1, "2026-09-18"), pairId: 5 },
      { ...makeAsset(2, "2026-09-18", "raw"), pairId: 5 },
    ]);
    renderSearch();

    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(1);
    expect(tiles[0]).toHaveAttribute("data-asset-id", "1"); // 代表=JPG
    expect(within(tiles[0]).getByTestId("gallery-pair-badge")).toHaveTextContent("RAW+JPG");
  });

  it("点击结果瓦片打开查看器（?asset=）", async () => {
    assetsPageMock.mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderSearch();

    fireEvent.click((await screen.findAllByTestId("gallery-tile"))[0]);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
  });

  it("空态：无匹配文案", async () => {
    assetsPageMock.mockResolvedValue([]);
    renderSearch();

    expect(await screen.findByTestId("search-empty")).toHaveTextContent("没有匹配的照片");
    expect(screen.getByTestId("search-count")).toHaveTextContent("0 个结果");
  });
});


// --- 语义搜索（M4） ----------------------------------------------------------------

describe("搜索：语义模式", () => {
  function switchToSemantic(user: ReturnType<typeof import("@testing-library/user-event").default.setup>) {
    return user.click(screen.getByTestId("search-mode-semantic"));
  }

  it("模式切换：语义模式显示输入/搜索按钮，条件控件隐藏；切回条件恢复", async () => {
    const user = userEvent.setup();
    renderSearch();

    await screen.findByTestId("search-page");
    expect(screen.getByTestId("search-mode-filters")).toHaveAttribute("aria-checked", "true");
    expect(screen.queryByTestId("semantic-input")).not.toBeInTheDocument();

    await switchToSemantic(user);
    expect(screen.getByTestId("semantic-input")).toBeInTheDocument();
    expect(screen.getByTestId("semantic-run")).toBeInTheDocument();
    expect(screen.queryByTestId("search-kind")).not.toBeInTheDocument();
    expect(screen.queryByTestId("search-from")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("search-mode-filters"));
    expect(screen.getByTestId("search-kind")).toBeInTheDocument();
  });

  it("语义搜索：searchSemantic 负载 + assets_by_ids 回填 + 相似度百分比角标", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([
      { assetId: 1, score: 0.93 },
      { assetId: 2, score: 0.51 },
    ]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18"), makeAsset(2, "2026-09-17")]);
    renderSearch();
    await screen.findByTestId("search-page");

    await user.click(screen.getByTestId("search-mode-semantic"));
    await user.type(screen.getByTestId("semantic-input"), "海边日落");
    await user.click(screen.getByTestId("semantic-run"));

    await screen.findAllByTestId("gallery-tile");
    expect(searchSemantic).toHaveBeenCalledWith("海边日落", 100, undefined);
    expect(assetsByIds).toHaveBeenCalledWith([1, 2]);
    const tiles = screen.getAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    // 命中序保持（分数降序语义由后端保证）：首个是 asset 1，角标 93%
    expect(tiles[0]).toHaveAttribute("data-asset-id", "1");
    expect(within(tiles[0]).getByTestId("search-score-badge")).toHaveTextContent("93%");
    expect(within(tiles[1]).getByTestId("search-score-badge")).toHaveTextContent("51%");
  });

  it("模型未就绪：引导卡片（约 630MB）+ 去设置跳转", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockRejectedValue("语义模型未就绪");
    renderSearch();
    await screen.findByTestId("search-page");

    await user.click(screen.getByTestId("search-mode-semantic"));
    await user.type(screen.getByTestId("semantic-input"), "猫");
    await user.click(screen.getByTestId("semantic-run"));

    const card = await screen.findByTestId("semantic-model-notready");
    expect(card).toHaveTextContent("约 630MB");
    await user.click(screen.getByTestId("semantic-gosettings"));
    expect(await screen.findByTestId("settings-probe")).toBeInTheDocument();
  });

  it("索引进度：indexTaskProgress(kind=ai) 驱动「正在建立语义索引（N/M）」", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.8 }]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderSearch();
    await screen.findByTestId("search-page");

    await user.click(screen.getByTestId("search-mode-semantic"));
    await user.type(screen.getByTestId("semantic-input"), "日落");
    await user.click(screen.getByTestId("semantic-run"));
    await screen.findAllByTestId("gallery-tile");

    act(() => {
      useAiStore.getState().handleAppEvent({ type: "indexTaskProgress", kind: "ai", done: 3, total: 10 });
    });
    expect(await screen.findByTestId("semantic-indexing")).toHaveTextContent("3/10");

    // 完成后提示消失
    act(() => {
      useAiStore.getState().handleAppEvent({ type: "indexTaskProgress", kind: "ai", done: 10, total: 10 });
    });
    await waitFor(() => expect(screen.queryByTestId("semantic-indexing")).not.toBeInTheDocument());
  });
});

// --- 语义查询历史（M4 二轮） ---------------------------------------------------------

describe("搜索：语义查询历史", () => {
  async function typeAndRun(
    user: ReturnType<typeof import("@testing-library/user-event").default.setup>,
    query: string,
  ): Promise<void> {
    const input = screen.getByTestId("semantic-input");
    await user.clear(input);
    await user.type(input, query);
    await user.click(screen.getByTestId("semantic-run"));
  }

  it("记录历史（新→旧 chips）；点击 chip 重搜并去重置顶", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([{ assetId: 1, score: 0.8 }]);
    vi.mocked(assetsByIds).mockResolvedValue([makeAsset(1, "2026-09-18")]);
    renderSearch();
    await screen.findByTestId("search-page");
    await user.click(screen.getByTestId("search-mode-semantic"));

    await typeAndRun(user, "海边日落");
    await typeAndRun(user, "猫");

    const chips = screen.getAllByTestId("semantic-history-item");
    expect(chips.map((chip) => chip.textContent)).toEqual(["猫", "海边日落"]);

    // 点击历史 chip 重搜
    vi.mocked(searchSemantic).mockClear();
    await user.click(chips[1]);
    await waitFor(() =>
      expect(searchSemantic).toHaveBeenLastCalledWith("海边日落", 100, undefined),
    );
    // 重复查询去重置顶
    expect(screen.getAllByTestId("semantic-history-item").map((chip) => chip.textContent)).toEqual([
      "海边日落",
      "猫",
    ]);
  });

  it("历史上限 5 条（最旧的被挤出）", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([]); // 空结果即可（历史记录与结果无关）
    renderSearch();
    await screen.findByTestId("search-page");
    await user.click(screen.getByTestId("search-mode-semantic"));

    for (const q of ["一", "二", "三", "四", "五", "六"]) {
      await typeAndRun(user, q);
    }
    const chips = screen.getAllByTestId("semantic-history-item");
    expect(chips).toHaveLength(5);
    expect(chips[0]).toHaveTextContent("六");
    expect(chips.map((chip) => chip.textContent)).not.toContain("一");
  });
});

// --- 相似度角标分档（M4 二轮） ---------------------------------------------------------

describe("搜索：相似度角标分档", () => {
  it(">75% high(accent) / 60–75% mid(muted) / <60% low(灰)", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([
      { assetId: 1, score: 0.9 },
      { assetId: 2, score: 0.65 },
      { assetId: 3, score: 0.4 },
    ]);
    vi.mocked(assetsByIds).mockResolvedValue([
      makeAsset(1, "2026-09-18"),
      makeAsset(2, "2026-09-18"),
      makeAsset(3, "2026-09-18"),
    ]);
    renderSearch();
    await screen.findByTestId("search-page");
    await user.click(screen.getByTestId("search-mode-semantic"));
    await user.type(screen.getByTestId("semantic-input"), "日落");
    await user.click(screen.getByTestId("semantic-run"));

    const tiles = await screen.findAllByTestId("gallery-tile");
    const badgeOf = (tile: HTMLElement) => within(tile).getByTestId("search-score-badge");
    expect(badgeOf(tiles[0])).toHaveAttribute("data-score-tier", "high");
    expect(badgeOf(tiles[1])).toHaveAttribute("data-score-tier", "mid");
    expect(badgeOf(tiles[2])).toHaveAttribute("data-score-tier", "low");
    // 分档样式：high 档 accent 底、low 档灰字
    expect(badgeOf(tiles[0]).className).toContain("bg-accent/90");
    expect(badgeOf(tiles[1]).className).toContain("text-white/85");
    expect(badgeOf(tiles[2]).className).toContain("text-white/45");
  });
});

// --- 空结果 × 语义索引建立中提示（M4 二轮） -------------------------------------------

describe("搜索：空结果提示（语义索引未建完）", () => {
  function aiStatus(done: number, total: number): import("@/ipc/api").IndexStatus {
    return {
      thumb: { pending: 0, done: 0, failed: 0 },
      exif: { pending: 0, done: 0, failed: 0 },
      ai: { pending: total - done, done, total },
    };
  }

  async function runSemanticQuery(
    user: ReturnType<typeof import("@testing-library/user-event").default.setup>,
  ): Promise<void> {
    await screen.findByTestId("search-page");
    await user.click(screen.getByTestId("search-mode-semantic"));
    await user.type(screen.getByTestId("semantic-input"), "日落");
    await user.click(screen.getByTestId("semantic-run"));
    await screen.findByTestId("semantic-empty");
  }

  it("索引未建完（70/100）→ 提示建立中 + 立即索引按钮（index_kick_now）", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([]);
    indexStatusMock.mockResolvedValue(aiStatus(70, 100));
    renderSearch();
    await runSemanticQuery(user);

    const hint = await screen.findByTestId("semantic-empty-indexing");
    expect(hint).toHaveTextContent("语义索引建立中（70/100）");

    await user.click(screen.getByTestId("semantic-empty-kick"));
    await waitFor(() => expect(indexKickNowMock).toHaveBeenCalledWith("ai"));
    expect(await screen.findByTestId("semantic-empty-kicked")).toBeInTheDocument();
  });

  it("索引已完成（100/100）→ 仅空结果文案，无建立中提示", async () => {
    const user = userEvent.setup();
    vi.mocked(searchSemantic).mockResolvedValue([]);
    indexStatusMock.mockResolvedValue(aiStatus(100, 100));
    renderSearch();
    await runSemanticQuery(user);

    await waitFor(() =>
      expect(screen.queryByTestId("semantic-empty-indexing")).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("semantic-empty")).toHaveTextContent("没有语义匹配的照片");
  });
});