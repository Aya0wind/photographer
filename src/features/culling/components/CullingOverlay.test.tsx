import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import CullingOverlay from "./CullingOverlay";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import {
  assetDetail,
  assetsByIds,
  cullAiPrescan,
  cullDecisionApply,
  cullSessionFinish,
  cullSessionOpen,
  type AssetDetailDto,
  type AssetDto,
  type CullSessionDto,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetDetail: vi.fn(),
    assetsByIds: vi.fn(),
    cullAiPrescan: vi.fn(),
    cullDecisionApply: vi.fn(),
    cullSessionFinish: vi.fn(),
    cullSessionOpen: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
  isTauri: vi.fn(() => false),
}));

const openMock = vi.mocked(cullSessionOpen);
const applyMock = vi.mocked(cullDecisionApply);
const finishMock = vi.mocked(cullSessionFinish);
const prescanMock = vi.mocked(cullAiPrescan);
const assetsByIdsMock = vi.mocked(assetsByIds);
const detailMock = vi.mocked(assetDetail);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(id: number, kind: AssetDto["kind"] = "photo"): AssetDto {
  return {
    id,
    path: `Y:\\照片\\${id}.jpg`,
    name: `IMG_000${id}.JPG`,
    kind,
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 1024,
  };
}

function makeSession(overrides: Partial<CullSessionDto> = {}): CullSessionDto {
  return {
    id: 7,
    name: "婚礼 · 初选",
    scope: { kind: "album", albumId: 3, subgroup: null },
    total: 4,
    accepted: 1,
    rejected: 1,
    undecided: 2,
    createdAt: "2026-09-28T10:00:00Z",
    updatedAt: "2026-09-28T10:00:00Z",
    finishedAt: null,
    ...overrides,
  };
}

/** 会话打开载荷：4 张，1 已选（id=1，手动）、1 已剔除（id=2，AI 预标记）、
 *  2 未定（id=3/4 为同连拍组 burst=11） */
function openPayload() {
  return {
    session: makeSession(),
    items: [
      { assetId: 1, decision: "accepted" as const, origin: "manual" as const, burstId: null, burstSize: 1 },
      { assetId: 2, decision: "rejected" as const, origin: "ai" as const, burstId: null, burstSize: 1 },
      { assetId: 3, decision: null, origin: "manual" as const, burstId: 11, burstSize: 2 },
      { assetId: 4, decision: null, origin: "manual" as const, burstId: 11, burstSize: 2 },
    ],
  };
}

function detailPayload(id: number, ai?: AssetDetailDto["aiAnalysis"]): AssetDetailDto | null {
  return {
    id,
    path: `Y:\\照片\\${id}.jpg`,
    filename: `IMG_000${id}.JPG`,
    size: 1024,
    kind: "photo",
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    createdAt: null,
    dupCount: 0,
    aiAnalysis: ai ?? null,
  };
}

function renderOverlay(onClose = vi.fn(), onFinished = vi.fn(), onFinishFailed = vi.fn()) {
  render(
    <I18nextProvider i18n={i18n}>
      <CullingOverlay
        session={makeSession()}
        onClose={onClose}
        onFinished={onFinished}
        onFinishFailed={onFinishFailed}
      />
    </I18nextProvider>,
  );
  return { onClose, onFinished, onFinishFailed };
}

beforeEach(() => {
  resetThumbPipelineForTests();
  openMock.mockReset().mockResolvedValue(openPayload());
  applyMock.mockReset().mockImplementation(async (_sessionId, decisions) => {
    // 回传最新计数（模拟后端真值派生）
    const applied = decisions[0]?.decision ?? null;
    return makeSession(
      applied === "accepted"
        ? { accepted: 2, undecided: 1 }
        : applied === "rejected"
          ? { rejected: 2, undecided: 1 }
          : { accepted: 0, rejected: 0, undecided: 4 },
    );
  });
  finishMock.mockReset().mockResolvedValue({ appliedFlag: 2, appliedRating: 0, rejected: 2 });
  prescanMock.mockReset().mockResolvedValue(null);
  assetsByIdsMock.mockReset().mockImplementation(async (ids) => ids.map((id) => makeAsset(id)));
  detailMock.mockReset().mockResolvedValue(null);
});

describe("全屏选片层（CullingOverlay V1）", () => {
  it("打开会话：断点续选到首个未定项，进度计数来自决定表", async () => {
    renderOverlay();

    await screen.findByTestId("culling-overlay");
    expect(openMock).toHaveBeenCalledWith(7);
    // 1 已选、2 已剔除 → 续选位 = 第 3 张（id=3）
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    const progress = screen.getByTestId("culling-progress");
    expect(progress).toHaveAttribute("data-accepted", "1");
    expect(progress).toHaveAttribute("data-rejected", "1");
    expect(progress).toHaveAttribute("data-undecided", "2");
    // 顶栏拖拽层（全屏浮层铁律）
    expect(screen.getByTestId("culling-overlay").querySelector("[data-tauri-drag-region]")).not.toBeNull();
    // 胶片条角标（AI 小徽仅在「AI 预标记 + 已有决定」时显示——id=2 origin=ai 已剔除）
    expect(screen.getAllByTestId("cull-film-accepted").length).toBe(1);
    expect(screen.getAllByTestId("cull-film-rejected").length).toBe(1);
    expect(screen.getAllByTestId("cull-film-ai").length).toBe(1);
  });

  it("键盘流：空格选入（乐观+落库+翻页）· X 剔除 · U 回未定（不翻页）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    // 空格 = 选入当前（id=3）并翻到下一张
    fireEvent.keyDown(document.body, { key: " ", code: "Space" });
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 3, decision: "accepted" }]);
    });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    });
    // 乐观进度：已选 2 / 未定 1
    await waitFor(() => {
      expect(screen.getByTestId("culling-progress")).toHaveAttribute("data-accepted", "2");
    });

    // X = 剔除当前（id=4，末张停住）
    fireEvent.keyDown(document.body, { key: "x" });
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: "rejected" }]);
    });
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    await waitFor(() => {
      expect(screen.getByTestId("culling-progress")).toHaveAttribute("data-rejected", "2");
    });

    // U = 回未定（不翻页）
    fireEvent.keyDown(document.body, { key: "u" });
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: null }]);
    });
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
  });

  it("←/→ 翻片（首尾禁用由按键消费；决定不丢）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    fireEvent.keyDown(document.body, { key: "ArrowLeft" });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("2 / 4");
    });
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    });
    // 越界按键：停住
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    expect(applyMock).not.toHaveBeenCalled(); // 纯翻片不写决定
  });

  it("决定写入失败 → 回滚乐观更新并提示", async () => {
    applyMock.mockResolvedValue(null);
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    fireEvent.keyDown(document.body, { key: " " });
    await waitFor(() => {
      expect(screen.getByTestId("culling-rollback")).toBeInTheDocument();
    });
    // 回滚：已选仍是 1（原决定表值）
    await waitFor(() => {
      expect(screen.getByTestId("culling-progress")).toHaveAttribute("data-accepted", "1");
    });
    // 翻页仍发生（键盘流不被失败阻塞）
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
  });

  it("Z 按住放大（keydown 进 / keyup 出，位置跟随鼠标），Esc 退出无确认", async () => {
    const { onClose } = renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    const stage = screen.getByTestId("culling-stage");
    fireEvent.keyDown(document.body, { key: "z" });
    expect(stage).toHaveAttribute("data-zoomed", "true");
    // 位置跟随：mousemove 更新 transform-origin（不抛错即可）
    fireEvent.mouseMove(stage, { clientX: 100, clientY: 60 });
    fireEvent.keyUp(document.body, { key: "z" });
    expect(stage).toHaveAttribute("data-zoomed", "false");

    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("V2 放大跨图保持：翻片同位置同倍率；松开恢复常态；再按 Z 回上次位置（会话内记忆）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    const stage = screen.getByTestId("culling-stage");
    // 首次按 Z：无记忆 → 居中锚点
    fireEvent.keyDown(document.body, { key: "z" });
    expect(stage).toHaveAttribute("data-zoomed", "true");
    expect(stage).toHaveAttribute("data-zoom-at", "0.500,0.500");
    // 移动锚点（jsdom rect 全 0 → 收夹到 1,1）
    fireEvent.mouseMove(stage, { clientX: 100, clientY: 60 });
    expect(stage).toHaveAttribute("data-zoom-at", "1.000,1.000");
    // 按住 Z 翻片：放大态与锚点跨图保持（同构图直查谁更锐）
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    expect(stage).toHaveAttribute("data-zoomed", "true");
    expect(stage).toHaveAttribute("data-zoom-at", "1.000,1.000");
    // 松开 Z：恢复常态
    fireEvent.keyUp(document.body, { key: "z" });
    expect(stage).toHaveAttribute("data-zoomed", "false");
    expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    // 再按 Z：回到上次锚点（会话内记忆），新图同位复放
    fireEvent.keyDown(document.body, { key: "z" });
    expect(stage).toHaveAttribute("data-zoomed", "true");
    expect(stage).toHaveAttribute("data-zoom-at", "1.000,1.000");
    fireEvent.keyUp(document.body, { key: "z" });
  });

  it("键盘接管：选片键 stopPropagation 压过全局快捷键（Ctrl 组合不接管）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    const seen: string[] = [];
    const globalSpy = (e: KeyboardEvent) => seen.push(e.key);
    window.addEventListener("keydown", globalSpy); // 冒泡阶段（模拟 AppShell 全局监听）

    fireEvent.keyDown(document.body, { key: " " });
    expect(seen).toEqual([]); // 被捕获层拦截
    fireEvent.keyDown(document.body, { key: "x", ctrlKey: true });
    expect(seen).toEqual(["x"]); // 修饰键组合让位
    window.removeEventListener("keydown", globalSpy);
  });

  it("AI 行内提示 chips：闭眼/失焦按 asset_detail 渲染", async () => {
    detailMock.mockImplementation(async (id) =>
      detailPayload(
        id,
        id === 3
          ? { eyes: { value: "closed", score: 88 }, blur: { value: "soft", score: 70 } }
          : null,
      ),
    );
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await waitFor(() => {
      const chips = screen.getAllByTestId("culling-ai-chip");
      expect(chips.map((c) => c.getAttribute("data-kind"))).toEqual(["eyesClosed", "blurSoft"]);
    });
  });

  it("决定按钮（鼠标等价物）：选入/未定/剔除", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-accept"));
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 3, decision: "accepted" }]);
    });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("4 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-undecided"));
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: null }]);
    });

    await userEvent.click(screen.getByTestId("culling-reject"));
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: "rejected" }]);
    });
  });
});

describe("收尾弹窗（CullingOverlay + CullFinishDialog）", () => {
  it("三开关组装 finish 载荷 → 结果摘要回传", async () => {
    const { onFinished } = renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-finish"));
    const dialog = await screen.findByTestId("cull-finish-dialog");

    // 默认建议：旗标开 + 拒绝开，星级关
    expect(within(dialog).getByTestId("cull-finish-flag")).toHaveAttribute("data-checked", "true");
    expect(within(dialog).getByTestId("cull-finish-rating")).toHaveAttribute("data-checked", "false");
    expect(within(dialog).getByTestId("cull-finish-reject")).toHaveAttribute("data-checked", "true");

    // 开星级并选 4 星
    await userEvent.click(within(dialog).getByTestId("cull-finish-rating-switch"));
    await userEvent.click(
      within(dialog).getAllByTestId("cull-finish-rating-star").find((s) => s.getAttribute("data-value") === "4")!,
    );

    await userEvent.click(within(dialog).getByTestId("cull-finish-confirm"));
    await waitFor(() => {
      expect(finishMock).toHaveBeenCalledWith(7, {
        acceptedFlag: true,
        acceptedRating: 4,
        rejectRejected: true,
      });
    });
    await waitFor(() => {
      expect(onFinished).toHaveBeenCalledWith(
        { appliedFlag: 2, appliedRating: 0, rejected: 2 },
        "婚礼 · 初选",
      );
    });
  });

  it("finish 失败 → onFinishFailed（会话未归档不误报成功）", async () => {
    finishMock.mockResolvedValue(null);
    const { onFinished, onFinishFailed } = renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-finish"));
    await userEvent.click(await screen.findByTestId("cull-finish-confirm"));

    await waitFor(() => {
      expect(onFinishFailed).toHaveBeenCalledTimes(1);
    });
    expect(onFinished).not.toHaveBeenCalled();
  });

  it("弹窗打开时浮层键盘让位（Esc 关弹窗不退浮层）", async () => {
    const { onClose } = renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-finish"));
    await screen.findByTestId("cull-finish-dialog");

    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.queryByTestId("cull-finish-dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("culling-overlay")).toBeInTheDocument();
  });
});

describe("对比视图（V2）", () => {
  it("C 进对比：默认同连拍组同屏（当前片+组内兄弟），焦点在当前片", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    fireEvent.keyDown(document.body, { key: "c" });
    const grid = await screen.findByTestId("culling-compare-grid");
    expect(grid).toHaveAttribute("data-count", "2");
    expect(screen.getByTestId("culling-overlay")).toHaveAttribute("data-mode", "compare");
    const panes = screen.getAllByTestId("culling-compare-pane");
    // 组员 = 当前片（id=3）+ 同连拍组兄弟（id=4，burst=11）
    expect(panes.map((p) => p.getAttribute("data-asset-id"))).toEqual(["3", "4"]);
    expect(panes[0]).toHaveAttribute("data-focused", "true");
    // 对比模式无单图操作条（决定入口在张上）
    expect(screen.queryByTestId("culling-actions")).not.toBeInTheDocument();
  });

  it("键盘流：2 选焦 → 空格选入焦点张（不翻页）；U 回未定", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    fireEvent.keyDown(document.body, { key: "c" });
    await screen.findByTestId("culling-compare-grid");

    fireEvent.keyDown(document.body, { key: "2" });
    const panes = screen.getAllByTestId("culling-compare-pane");
    expect(panes[1]).toHaveAttribute("data-focused", "true");

    fireEvent.keyDown(document.body, { key: " " });
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: "accepted" }]);
    });
    await waitFor(() => {
      expect(panes[1]).toHaveAttribute("data-decision", "accepted");
    });

    // U = 焦点张回未定
    fireEvent.keyDown(document.body, { key: "u" });
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: null }]);
    });

    // C 回单图：进出对比保持单图位置（仍在第 3 张）
    fireEvent.keyDown(document.body, { key: "c" });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    expect(screen.queryByTestId("culling-compare-grid")).not.toBeInTheDocument();
  });

  it("张上按钮独立标记（点击 ✓/✗ = 鼠标等价物）；←/→ 切焦", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    fireEvent.keyDown(document.body, { key: "c" });
    await screen.findByTestId("culling-compare-grid");

    // ←/→ 切焦
    fireEvent.keyDown(document.body, { key: "ArrowRight" });
    expect(screen.getAllByTestId("culling-compare-pane")[1]).toHaveAttribute("data-focused", "true");
    fireEvent.keyDown(document.body, { key: "ArrowLeft" });
    expect(screen.getAllByTestId("culling-compare-pane")[0]).toHaveAttribute("data-focused", "true");

    // 点 pane 1 的 ✗ 剔除（独立于焦点张）
    await userEvent.click(screen.getAllByTestId("culling-compare-reject")[1]);
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 4, decision: "rejected" }]);
    });
    await userEvent.click(screen.getAllByTestId("culling-compare-accept")[0]);
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [{ assetId: 3, decision: "accepted" }]);
    });
  });

  it("同屏张数 2/3/4 可切：组员不足补相邻（按距离序）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    fireEvent.keyDown(document.body, { key: "c" });
    await screen.findByTestId("culling-compare-grid");

    // 4 张：当前(idx2) + 同组兄弟(idx3) + 相邻 idx1、idx0
    const count4 = screen
      .getAllByTestId("culling-compare-count")
      .find((b) => b.getAttribute("data-count") === "4")!;
    await userEvent.click(count4);
    await waitFor(() => {
      expect(screen.getByTestId("culling-compare-grid")).toHaveAttribute("data-count", "4");
    });
    expect(
      screen.getAllByTestId("culling-compare-pane").map((p) => p.getAttribute("data-asset-id")),
    ).toEqual(["3", "4", "2", "1"]);

    // 3 张：截前三个
    const count3 = screen
      .getAllByTestId("culling-compare-count")
      .find((b) => b.getAttribute("data-count") === "3")!;
    await userEvent.click(count3);
    await waitFor(() => {
      expect(screen.getByTestId("culling-compare-grid")).toHaveAttribute("data-count", "3");
    });
    expect(
      screen.getAllByTestId("culling-compare-pane").map((p) => p.getAttribute("data-asset-id")),
    ).toEqual(["3", "4", "2"]);
  });
});

describe("连拍组集成（V2）", () => {
  it("组徽标 + 本组只留这张：当前 accepted + 组内未定 rejected（一次批量 decision_apply）", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    // 当前片 id=3（burst=11，组 2 张）
    const burst = screen.getByTestId("culling-burst");
    expect(burst).toHaveAttribute("data-size", "2");
    expect(screen.getByTestId("culling-burst-badge")).toHaveTextContent("组 2 张");

    await userEvent.click(screen.getByTestId("culling-burst-keep"));
    await waitFor(() => {
      expect(applyMock).toHaveBeenCalledWith(7, [
        { assetId: 3, decision: "accepted" },
        { assetId: 4, decision: "rejected" },
      ]);
    });
    // 不翻页（留在保留张上复查）
    expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    // 当前决定角标转已选
    await waitFor(() => {
      expect(screen.getByTestId("culling-decision-badge")).toHaveAttribute("data-decision", "accepted");
    });
  });

  it("非连拍组片不显示组徽标", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });
    fireEvent.keyDown(document.body, { key: "ArrowLeft" });
    fireEvent.keyDown(document.body, { key: "ArrowLeft" });
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("1 / 4");
    });
    expect(screen.queryByTestId("culling-burst")).not.toBeInTheDocument();
  });
});

describe("AI 预标记区分 + AI 挑图（V3）", () => {
  it("origin=ai 决定角标带 AI 小标；手动翻转后转 manual 样式", async () => {
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    // 上一张 = id=2（AI 预标记剔除）
    fireEvent.keyDown(document.body, { key: "ArrowLeft" });
    await waitFor(() => {
      const badge = screen.getByTestId("culling-decision-badge");
      expect(badge).toHaveAttribute("data-decision", "rejected");
      expect(badge).toHaveAttribute("data-origin", "ai");
    });
    expect(screen.getByTestId("culling-decision-ai")).toBeInTheDocument();
    expect(screen.getAllByTestId("cull-film-ai").length).toBe(1);

    // 空格手动翻转（选入）→ origin 转 manual（胶片条 AI 小徽随之消失）
    fireEvent.keyDown(document.body, { key: " " });
    await waitFor(() => {
      expect(screen.queryAllByTestId("cull-film-ai").length).toBe(0);
    });
  });

  it("AI 挑图：规则默认 → 预览摘要 → 调规则重预览 → 应用（apply=true 同规则）→ toast + 决定表刷新", async () => {
    prescanMock.mockResolvedValue({
      suggestedAccepted: 2,
      suggestedRejected: 3,
      skippedManual: 2,
      exemptedGroup: 1,
    });
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-ai-open"));
    const dialog = await screen.findByTestId("cull-ai-dialog");

    // 默认规则：闭眼/失焦开（标准档）、留最锐关、豁免 0、上限不限
    await userEvent.click(within(dialog).getByTestId("cull-ai-preview"));
    const defaultRules = {
      eyes: { enabled: true, sensitivity: "normal" },
      blur: { enabled: true, sensitivity: "normal" },
      burstKeepSharpest: false,
      groupExemptFaces: 0,
      maxAccepted: null,
    };
    await waitFor(() => {
      expect(prescanMock).toHaveBeenCalledWith(7, defaultRules, false);
    });
    const summary = await within(dialog).findByTestId("cull-ai-summary");
    expect(summary).toHaveAttribute("data-accepted", "2");
    expect(summary).toHaveAttribute("data-rejected", "3");
    expect(summary).toHaveAttribute("data-skipped", "2");
    expect(summary).toHaveAttribute("data-exempted", "1");

    // 调整规则：闭眼强档 + 精选上限 30 → 旧摘要作废，重新预览
    await userEvent.click(
      within(dialog)
        .getAllByTestId("cull-ai-eyes-sens-opt")
        .find((b) => b.getAttribute("data-value") === "strong")!,
    );
    await userEvent.type(within(dialog).getByTestId("cull-ai-max-input"), "30");
    expect(within(dialog).queryByTestId("cull-ai-summary")).not.toBeInTheDocument();
    await userEvent.click(within(dialog).getByTestId("cull-ai-preview"));
    const tunedRules = {
      eyes: { enabled: true, sensitivity: "strong" },
      blur: { enabled: true, sensitivity: "normal" },
      burstKeepSharpest: false,
      groupExemptFaces: 0,
      maxAccepted: 30,
    };
    await waitFor(() => {
      expect(prescanMock).toHaveBeenCalledWith(7, tunedRules, false);
    });

    // 应用建议（apply=true，同规则）
    await userEvent.click(within(dialog).getByTestId("cull-ai-apply"));
    await waitFor(() => {
      expect(prescanMock).toHaveBeenCalledWith(7, tunedRules, true);
    });
    // 应用后：弹窗关 + toast + 重开决定表刷新（保持当前位）
    await waitFor(() => {
      expect(screen.queryByTestId("cull-ai-dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("culling-ai-toast")).toHaveAttribute("data-rejected", "3");
    await waitFor(() => {
      expect(openMock).toHaveBeenCalledTimes(2);
    });
    expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
  });

  it("预览失败提示后端未连接；无摘要时应用禁用", async () => {
    prescanMock.mockResolvedValue(null);
    renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-ai-open"));
    const dialog = await screen.findByTestId("cull-ai-dialog");
    await userEvent.click(within(dialog).getByTestId("cull-ai-preview"));

    expect(await within(dialog).findByTestId("cull-ai-failed")).toBeInTheDocument();
    expect(within(dialog).getByTestId("cull-ai-apply")).toBeDisabled();
  });

  it("AI 弹窗打开时浮层键盘让位（Esc 关弹窗不退浮层）", async () => {
    const { onClose } = renderOverlay();
    await waitFor(() => {
      expect(screen.getByTestId("culling-index")).toHaveTextContent("3 / 4");
    });

    await userEvent.click(screen.getByTestId("culling-ai-open"));
    await screen.findByTestId("cull-ai-dialog");

    fireEvent.keyDown(document.body, { key: "Escape" });
    expect(onClose).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(screen.queryByTestId("cull-ai-dialog")).not.toBeInTheDocument();
    });
    expect(screen.getByTestId("culling-overlay")).toBeInTheDocument();
  });
});
