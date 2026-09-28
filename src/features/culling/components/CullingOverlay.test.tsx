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

/** 会话打开载荷：4 张，1 已选（id=1）、1 已剔除（id=2）、2 未定（id=3/4） */
function openPayload() {
  return {
    session: makeSession(),
    items: [
      { assetId: 1, decision: "accepted" as const, origin: "manual" as const },
      { assetId: 2, decision: "rejected" as const, origin: "manual" as const },
      { assetId: 3, decision: null, origin: "manual" as const },
      { assetId: 4, decision: null, origin: "ai" as const },
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
    // 胶片条角标（AI 小徽仅在「AI 预标记 + 已有决定」时显示——id=4 origin=ai 但未定）
    expect(screen.getAllByTestId("cull-film-accepted").length).toBe(1);
    expect(screen.getAllByTestId("cull-film-rejected").length).toBe(1);
    expect(screen.queryAllByTestId("cull-film-ai").length).toBe(0);
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
