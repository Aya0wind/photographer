import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import ViewerOverlay from "./ViewerOverlay";
import { groupAssetsByDate, type AssetGroup } from "../lib/assetGroups";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import {
  assetDetail,
  assetFlagSet,
  assetRatingSet,
  assetThumbGet,
  type AssetDetailDto,
  type AssetDto,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetDetail: vi.fn(),
    assetThumbGet: vi.fn(),
    assetFlagSet: vi.fn(),
    assetRatingSet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const detailMock = vi.mocked(assetDetail);
const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);
const ratingMock = vi.mocked(assetRatingSet);
const flagMock = vi.mocked(assetFlagSet);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(id: number, kind: AssetDto["kind"], name: string): AssetDto {
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\2026\\${name}`,
    name,
    kind,
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 5 * 1024 * 1024,
  };
}

const GROUP_ASSETS: AssetDto[] = [
  makeAsset(1, "photo", "IMG_0001.JPG"),
  makeAsset(2, "photo", "IMG_0002.JPG"),
  makeAsset(3, "photo", "IMG_0003.JPG"),
];

const DETAIL: AssetDetailDto = {
  id: 1,
  path: GROUP_ASSETS[0].path,
  filename: "IMG_0001.JPG",
  size: 5242880,
  kind: "photo",
  capturedAt: "2026-09-18T10:20:30",
  camera: "Canon EOS R5",
  lens: null,
  createdAt: "2026-09-19T08:00:00",
  dupCount: 2,
};

function renderViewer(
  assets: AssetDto[] = GROUP_ASSETS,
  index = 0,
  overrides?: { onNavigate?: (i: number) => void; onClose?: () => void },
) {
  const group: AssetGroup = groupAssetsByDate(assets)[0];
  const onNavigate = vi.fn();
  const onClose = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <ViewerOverlay
        asset={assets[index]}
        group={group}
        index={index}
        onNavigate={overrides?.onNavigate ?? onNavigate}
        onClose={overrides?.onClose ?? onClose}
      />
    </I18nextProvider>,
  );
  return { onNavigate, onClose };
}

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 1200,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => 800,
  });
});

beforeEach(() => {
  detailMock.mockReset().mockResolvedValue(DETAIL);
  ratingMock.mockReset().mockResolvedValue(undefined);
  flagMock.mockReset().mockResolvedValue(undefined);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  resetThumbPipelineForTests();
});

// --- 打开与图片来源 -----------------------------------------------------------------

describe("查看器：打开与图片来源", () => {
  it("photo 优先原图 asset 协议；胶片条渲染全组并高亮当前项", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();

    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", `asset://${GROUP_ASSETS[0].path}`);
    expect(img).toHaveAttribute("data-fallback", "original");
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0001.JPG");
    expect(screen.getByTestId("viewer-index")).toHaveTextContent("1 / 3");

    const strip = screen.getByTestId("viewer-filmstrip");
    const thumbs = within(strip).getAllByTestId("viewer-filmthumb");
    expect(thumbs).toHaveLength(3);
    expect(thumbs[0]).toHaveAttribute("data-current", "true");
    expect(thumbs[1]).toHaveAttribute("data-current", "false");
  });

  it("photo 原图加载失败（TIF 等）→ 中间档 2048 回退", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (_id: number, size: number) =>
      size === 2048
        ? { status: "ready", path: "C:\\thumbs\\2048\\img1.jpg" }
        : { status: "pending" },
    );
    renderViewer();

    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("data-fallback", "original");
    fireEvent.error(img);

    // WebView2 渲染不动的原图 → 先降中间档（清晰版），而不是一步到 512
    await waitFor(() =>
      expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "mid"),
    );
    expect(screen.getByTestId("viewer-img")).toHaveAttribute("src", "asset://C:\\thumbs\\2048\\img1.jpg");
    expect(thumbMock).toHaveBeenCalledWith(1, 2048);
  });

  it("中间档确定无图（后端无 2048 档）→ 自动降 512 档", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (_id: number, size: number) =>
      size === 2048 ? { status: "unavailable" } : { status: "ready", path: "C:\\thumbs\\512\\img1.jpg" },
    );
    renderViewer();

    const img = await screen.findByTestId("viewer-img");
    fireEvent.error(img);

    await waitFor(() =>
      expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "thumb"),
    );
    expect(screen.getByTestId("viewer-img")).toHaveAttribute("src", "asset://C:\\thumbs\\512\\img1.jpg");
    expect(thumbMock).toHaveBeenCalledWith(1, 2048);
    expect(thumbMock).toHaveBeenCalledWith(1, 1280);
  });

  it("原图加载慢：在途超过 300ms 出现加载提示，不黑屏误判", async () => {
    convertMock.mockImplementation(() => {
      throw new Error("no asset protocol");
    });
    thumbMock.mockImplementation(() => new Promise(() => undefined)); // 512 档永不在途结算
    renderViewer();

    // 源在途（photo 无原图 → 直达 512 档，仍在途）：300ms 内不出现提示
    await screen.findByTestId("viewer");
    expect(screen.queryByTestId("viewer-loading")).not.toBeInTheDocument();
    // 300ms 后出现 spinner
    await waitFor(() => expect(screen.getByTestId("viewer-loading")).toBeInTheDocument());
  });

  it("RAW 先显示 512 内嵌 JPEG，内嵌全幅直出就绪后替换变清晰（Windows 照片同款）", async () => {
    const rawAssets = [makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")];
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    let resolveEmbed: ((r: { status: "ready"; path: string }) => void) | undefined;
    thumbMock.mockImplementation((id, size) => {
      if (size === 6000) {
        // 内嵌全幅直出档（>2048 为后端语义标记）在途
        return new Promise<{ status: "ready"; path: string }>((resolve) => {
          if (id === 7) resolveEmbed = resolve;
        });
      }
      return Promise.resolve({ status: "ready", path: "C:\\thumbs\\512\\img7.jpg" });
    });
    renderViewer(rawAssets);

    // 首帧不等全幅提取，立即显示 512 档。
    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\512\\img7.jpg");
    expect(img).toHaveAttribute("data-fallback", "thumb");
    expect(convertMock).not.toHaveBeenCalledWith(rawAssets[0].path);
    expect(thumbMock).toHaveBeenCalledWith(7, 1280);
    expect(thumbMock).toHaveBeenCalledWith(7, 6000);
    // 主路径未失败前不请求 2048 显影兜底
    expect(thumbMock).not.toHaveBeenCalledWith(7, 2048);

    // 内嵌全幅就绪：叠加替换变清晰。
    act(() =>
      resolveEmbed?.({ status: "ready", path: "C:\\thumbs\\raw-embed-v1\\img7.jpg" }),
    );
    await waitFor(() => {
      expect(screen.getByTestId("viewer-img")).toHaveAttribute(
        "src",
        "asset://C:\\thumbs\\raw-embed-v1\\img7.jpg",
      );
    });
    expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "raw-embed");
  });

  it("RAW 无内嵌预览（embed 结算 null）→ 才回落 2048 rawler 显影兜底", async () => {
    const rawAssets = [makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")];
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    let resolveFull: ((r: { status: "ready"; path: string }) => void) | undefined;
    thumbMock.mockImplementation((id, size) => {
      // 无内嵌预览：必须是「确定失败」而非排队中，显影兜底才应启用
      if (size === 6000) return Promise.resolve({ status: "unavailable" });
      if (size === 2048) {
        return new Promise<{ status: "ready"; path: string }>((resolve) => {
          if (id === 7) resolveFull = resolve;
        });
      }
      return Promise.resolve({ status: "ready", path: "C:\\thumbs\\512\\img7.jpg" });
    });
    renderViewer(rawAssets);

    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\512\\img7.jpg");
    // embed 失败结算后显影兜底才被请求
    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(7, 2048));

    act(() =>
      resolveFull?.({ status: "ready", path: "C:\\thumbs\\2048-raw-full-v1\\img7.jpg" }),
    );
    await waitFor(() => {
      expect(screen.getByTestId("viewer-img")).toHaveAttribute(
        "src",
        "asset://C:\\thumbs\\2048-raw-full-v1\\img7.jpg",
      );
    });
    expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "raw-full");
  });

  it("RAW 无缩略图（后端恒 None 的兜底）：永久占位不崩溃", async () => {
    thumbMock.mockResolvedValue({ status: "unavailable" });
    renderViewer([makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")]);
    await screen.findByTestId("viewer-placeholder");
    expect(document.querySelector("img[data-testid='viewer-img']")).toBeNull();
  });
});

// --- 切换与关闭 ---------------------------------------------------------------------

describe("查看器：左右切换与关闭", () => {
  it("图片缩放后点击左右箭头仍能切图，不被拖拽捕获拦截", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    function StatefulViewer() {
      const [index, setIndex] = useState(0);
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return (
        <I18nextProvider i18n={i18n}>
          <ViewerOverlay
            asset={GROUP_ASSETS[index]}
            group={group}
            index={index}
            onNavigate={setIndex}
            onClose={() => {}}
          />
        </I18nextProvider>
      );
    }
    render(<StatefulViewer />);

    const stage = await screen.findByTestId("viewer-stage");
    fireEvent.wheel(stage, { deltaY: -100, ctrlKey: true });
    await waitFor(() => expect(stage).toHaveAttribute("data-scale", "1.20"));

    const next = screen.getByTestId("viewer-next");
    fireEvent.pointerDown(next, { pointerId: 1, clientX: 100, clientY: 100 });
    fireEvent.click(next);
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0002.JPG");
  });

  it("箭头按钮/键盘同组切换；首尾禁用（有状态容器驱动 index）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    let setIndex: ((i: number) => void) | undefined;
    function StatefulViewer() {
      const [index, setIdx] = useState(0);
      setIndex = setIdx;
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return (
        <I18nextProvider i18n={i18n}>
          <ViewerOverlay
            asset={GROUP_ASSETS[index]}
            group={group}
            index={index}
            onNavigate={setIdx}
            onClose={() => {}}
          />
        </I18nextProvider>
      );
    }
    render(<StatefulViewer />);

    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0001.JPG");
    expect(screen.getByTestId("viewer-prev")).toBeDisabled();
    expect(screen.getByTestId("viewer-next")).toBeEnabled();

    // 箭头按钮 → 第二张
    fireEvent.click(screen.getByTestId("viewer-next"));
    await waitFor(() => expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0002.JPG"));
    expect(screen.getByTestId("viewer-prev")).toBeEnabled();

    // 键盘 → 第三张；此时 next 禁用
    fireEvent.keyDown(window, { key: "ArrowRight" });
    await waitFor(() => expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0003.JPG"));
    expect(screen.getByTestId("viewer-next")).toBeDisabled();

    // 键盘回退一张
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    await waitFor(() => expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0002.JPG"));
    expect(setIndex).toBeDefined();
  });

  it("旋转 transform 顺序 translate→rotate→scale（平移后旋转绕图片视觉中心）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const img = await screen.findByTestId("viewer-img");

    fireEvent.click(screen.getByTestId("viewer-rotate-cw"));
    await waitFor(() =>
      expect(img.style.transform).toBe("translate(0px, 0px) rotate(90deg) scale(1)"),
    );
  });

  it("顶栏拖拽层不会吞掉旋转、详细信息和关闭按钮点击", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onClose } = renderViewer();

    for (const testId of [
      "viewer-rotate-ccw",
      "viewer-rotate-cw",
      "viewer-exif-toggle",
      "viewer-close",
    ]) {
      expect(screen.getByTestId(testId).className).toContain("pointer-events-auto");
    }

    fireEvent.click(screen.getByTestId("viewer-rotate-cw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
    fireEvent.click(screen.getByTestId("viewer-exif-toggle"));
    expect(screen.getByTestId("viewer-exif")).toHaveAttribute("data-open", "false");
    fireEvent.click(screen.getByTestId("viewer-close"));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("胶片条点击跳转；Esc 关闭返回画廊", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onNavigate, onClose } = renderViewer();

    const thumbs = screen.getAllByTestId("viewer-filmthumb");
    fireEvent.click(thumbs[2]);
    expect(onNavigate).toHaveBeenCalledWith(2);

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("提供明确退出按钮；详情抽屉与预览画布并列且可从抽屉内收起", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onClose } = renderViewer();

    const preview = screen.getByTestId("viewer-preview-pane");
    const leftPane = screen.getByTestId("viewer-left-pane");
    const filmstrip = screen.getByTestId("viewer-filmstrip");
    const drawer = await screen.findByTestId("viewer-exif");
    expect(preview.parentElement).toBe(leftPane);
    expect(filmstrip.parentElement).toBe(leftPane);
    expect(leftPane.parentElement).toBe(drawer.parentElement);
    expect(drawer.className).toContain("shrink-0");
    expect(drawer.className).not.toContain("absolute");
    const close = screen.getByTestId("viewer-close");
    expect(close.parentElement).toBe(screen.getByTestId("viewer-stage"));
    expect(close.className).toContain("rounded-full");

    fireEvent.click(screen.getByTestId("viewer-exif-toggle"));
    expect(screen.getByTestId("viewer-exif")).toHaveAttribute("data-open", "false");
    expect(screen.getByTestId("viewer-exif").className).toContain("w-10");
    expect(screen.queryByTestId("viewer-exif-rows")).not.toBeInTheDocument();
    fireEvent.click(screen.getByTestId("viewer-exif-toggle"));
    expect(screen.getByTestId("viewer-exif")).toHaveAttribute("data-open", "true");
    expect(await screen.findByTestId("viewer-exif-rows")).toBeInTheDocument();

    fireEvent.click(close);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("相邻预取：512 回退档高优先预热（缩略管线）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer(GROUP_ASSETS, 1);

    await waitFor(() => expect(thumbMock).toHaveBeenCalledWith(1, 1280));
    expect(thumbMock).toHaveBeenCalledWith(3, 1280);
  });
});

// --- 缩放/平移 ---------------------------------------------------------------------

describe("查看器：缩放与复位", () => {
  it("主预览滚轮始终只缩放，不触发前后翻页", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onNavigate } = renderViewer(GROUP_ASSETS, 1);
    const stage = await screen.findByTestId("viewer-stage");

    fireEvent.wheel(stage, { deltaY: 100 });
    expect(onNavigate).not.toHaveBeenCalled();
    expect(stage).toHaveAttribute("data-scale", "1.00");

    fireEvent.wheel(stage, { deltaY: -100 });
    await waitFor(() => expect(stage).toHaveAttribute("data-scale", "1.20"));
    expect(onNavigate).not.toHaveBeenCalled();

    fireEvent.wheel(stage, { deltaY: -100 });
    await waitFor(() => expect(stage).toHaveAttribute("data-scale", "1.44"));
  });

  it("底部缩略图条滚轮直接切换当前图片，单次手势最多跨 8 张而不滚动列表", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const manyAssets = Array.from({ length: 15 }, (_, i) =>
      makeAsset(i + 1, "photo", `IMG_${String(i + 1).padStart(4, "0")}.JPG`),
    );
    const { onNavigate } = renderViewer(manyAssets, 1);
    const filmstrip = await screen.findByTestId("viewer-filmstrip");
    filmstrip.scrollLeft = 0;

    fireEvent.wheel(filmstrip, { deltaY: 10_000 });
    expect(onNavigate).toHaveBeenLastCalledWith(9);
    expect(filmstrip.scrollLeft).toBe(0);

    // 同一方向手势已到上限，后续惯性事件不会继续跳；反向视为新手势。
    fireEvent.wheel(filmstrip, { deltaY: 200 });
    expect(onNavigate).toHaveBeenCalledTimes(1);
    fireEvent.wheel(filmstrip, { deltaY: -10_000 });
    expect(onNavigate).toHaveBeenLastCalledWith(0);
    expect(filmstrip.scrollLeft).toBe(0);
  });

  it("wheel 放大至 1.2x（钳制 1x-4x），双击复位", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const stage = await screen.findByTestId("viewer-stage");
    expect(stage).toHaveAttribute("data-scale", "1.00");

    fireEvent.wheel(stage, { deltaY: -100, ctrlKey: true });
    await waitFor(() => expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.20"));

    // 连续放大钳制 4x
    for (let i = 0; i < 12; i += 1) fireEvent.wheel(stage, { deltaY: -100 });
    await waitFor(() => expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "4.00"));

    fireEvent.dblClick(stage);
    await waitFor(() => expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.00"));
  });

  it("1x 时缩小无效（下限钳制）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const stage = await screen.findByTestId("viewer-stage");

    fireEvent.wheel(stage, { deltaY: 100 });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.00");
  });
});

// --- 旋转 -------------------------------------------------------------------------

describe("查看器：旋转（90° 步进）", () => {
  it("按钮顺/逆时针步进；双击复位归零（含旋转）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const stage = await screen.findByTestId("viewer-stage");
    expect(stage).toHaveAttribute("data-rotation", "0");

    fireEvent.click(screen.getByTestId("viewer-rotate-cw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
    fireEvent.click(screen.getByTestId("viewer-rotate-cw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "180");
    // 逆时针回退；角度保持连续，不在一圈边界归一化
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "-90");

    // 双击复位：缩放/平移/旋转全部归零
    fireEvent.dblClick(screen.getByTestId("viewer-stage"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "0");
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.00");
  });

  it("连续向左旋转四次保持同向补间并到达 -360°", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    await screen.findByTestId("viewer-stage");

    const rotateLeft = screen.getByTestId("viewer-rotate-ccw");
    for (let i = 0; i < 4; i += 1) fireEvent.click(rotateLeft);

    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "-360");
    expect(screen.getByTestId("viewer-img").style.transform).toContain("rotate(-360deg)");
  });

  it("键盘 . , R 旋转（./R=顺时针，,=逆时针）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    await screen.findByTestId("viewer-stage");

    fireEvent.keyDown(window, { key: "." });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
    fireEvent.keyDown(window, { key: "," });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "0");
    fireEvent.keyDown(window, { key: "R" });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
  });

  it("键盘 Home/End 跳组首/尾（有状态容器驱动 index）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    function StatefulViewer() {
      const [index, setIdx] = useState(1);
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return (
        <I18nextProvider i18n={i18n}>
          <ViewerOverlay
            asset={GROUP_ASSETS[index]}
            group={group}
            index={index}
            onNavigate={setIdx}
            onClose={() => {}}
          />
        </I18nextProvider>
      );
    }
    render(<StatefulViewer />);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Home" });
    await waitFor(() => expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0001.JPG"));
    fireEvent.keyDown(window, { key: "End" });
    await waitFor(() => expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0003.JPG"));
  });

  it("操作提示：首次 3s 后淡出；? 键重新唤出并再计时（fake timers）", async () => {
    vi.useFakeTimers();
    try {
      convertMock.mockImplementation((p: string) => `asset://${p}`);
      renderViewer();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      expect(screen.getByTestId("viewer-hint")).toBeInTheDocument();

      // 3s 后淡出（CSS opacity 过渡：类切换确定）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3250);
      });
      expect(screen.getByTestId("viewer-hint").className).toContain("opacity-0");

      // ? 唤出，再过 3s 又淡出
      fireEvent.keyDown(window, { key: "?" });
      expect(screen.getByTestId("viewer-hint").className).toContain("opacity-100");
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3250);
      });
      expect(screen.getByTestId("viewer-hint").className).toContain("opacity-0");
    } finally {
      vi.useRealTimers();
    }
  });

  it("胶片条选中项自动滚动居中（切换照片时 scrollToIndex center + smooth）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    class FakeImage {
      src = "";
    }
    vi.stubGlobal("Image", FakeImage);
    const calls: Array<Record<string, unknown>> = [];
    const originalScrollTo = HTMLElement.prototype.scrollTo;
    HTMLElement.prototype.scrollTo = function (arg?: unknown) {
      calls.push((arg ?? {}) as Record<string, unknown>);
    } as typeof HTMLElement.prototype.scrollTo;
    try {
      function StatefulViewer() {
        const [index, setIdx] = useState(0);
        const group = groupAssetsByDate(GROUP_ASSETS)[0];
        return (
          <I18nextProvider i18n={i18n}>
            <ViewerOverlay
              asset={GROUP_ASSETS[index]}
              group={group}
              index={index}
              onNavigate={setIdx}
              onClose={() => {}}
            />
          </I18nextProvider>
        );
      }
      render(<StatefulViewer />);
      expect(await screen.findByTestId("viewer")).toBeInTheDocument();
      const before = calls.length;

      fireEvent.click(screen.getByTestId("viewer-next"));
      // 切换后胶片条平滑滚动到新选中项（居中对齐）
      await waitFor(() => {
        expect(calls.length).toBeGreaterThan(before);
        const last = calls[calls.length - 1];
        expect(last).toMatchObject({ behavior: "smooth" });
        expect(last).toHaveProperty("left");
      });
    } finally {
      HTMLElement.prototype.scrollTo = originalScrollTo;
      vi.unstubAllGlobals();
    }
  });
});

// --- 切换原子提交与胶片条 ------------------------------------------------------------

describe("查看器：原子切图与胶片条", () => {
  it("新图加载完成前不可见，完成后单层原子替换，避免两张图片叠显", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    let setIndex: ((i: number) => void) | undefined;
    function StatefulViewer() {
      const [index, setIdx] = useState(0);
      setIndex = setIdx;
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return (
        <I18nextProvider i18n={i18n}>
          <ViewerOverlay
            asset={GROUP_ASSETS[index]}
            group={group}
            index={index}
            onNavigate={setIdx}
            onClose={() => {}}
          />
        </I18nextProvider>
      );
    }
    render(<StatefulViewer />);

    const url1 = `asset://${GROUP_ASSETS[0].path}`;
    const url2 = `asset://${GROUP_ASSETS[1].path}`;
    const img1 = await screen.findByTestId("viewer-img");
    expect(img1).toHaveAttribute("src", url1);

    // 首图 onLoad → 提交为底层
    fireEvent.load(img1);
    await waitFor(() =>
      expect(screen.getByTestId("viewer-img-prev")).toHaveAttribute("src", url1),
    );

    // 切到第二张：新图仅在不可见解码层加载，旧图仍是唯一可见图层。
    act(() => setIndex?.(1));
    const img2 = await screen.findByTestId("viewer-img");
    expect(img2).toHaveAttribute("src", url2);
    expect(img2.className).toContain("invisible");
    expect(screen.getByTestId("viewer-img-prev")).toHaveAttribute("src", url1);
    expect(screen.getByTestId("viewer-img-prev").className).not.toContain("invisible");
    expect(
      screen.getByTestId("viewer-stage").querySelectorAll("img:not(.invisible)"),
    ).toHaveLength(1);

    // 新图 onLoad 后一次提交：DOM 中只保留第二张可见图，不存在交叉淡入重叠期。
    fireEvent.load(img2);
    await waitFor(() =>
      expect(screen.queryByTestId("viewer-img-prev")).toHaveAttribute("src", url2),
    );
    expect(screen.queryByTestId("viewer-img")).not.toBeInTheDocument();
  });

  it("胶片条横向虚拟化：48 张组只渲染可视区格子（DOM 数远小于组总数）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const bigGroup = Array.from({ length: 48 }, (_, i) =>
      makeAsset(i + 1, "photo", `IMG_${String(i + 1).padStart(4, "0")}.JPG`),
    );
    renderViewer(bigGroup);

    await screen.findByTestId("viewer");
    const thumbs = screen.getAllByTestId("viewer-filmthumb");
    expect(thumbs.length).toBeLessThan(48);
    // 静态迷你占位（序号），无骨架动画
    expect(thumbs[0].querySelector('[data-testid="thumb-mini"]')).not.toBeNull();
    const cell = thumbs[0].querySelector('[data-testid="viewer-filmthumb-cell"]');
    expect(cell?.className).not.toContain("sp-skeleton");
  });

  it("相邻预取：预热相邻 photo 原图与 512 档 URL 解码（new Image）", async () => {
    const created: Array<{ src: string }> = [];
    class FakeImage {
      src = "";
      constructor() {
        created.push(this);
      }
    }
    vi.stubGlobal("Image", FakeImage);
    try {
      convertMock.mockImplementation((p: string) => `asset://${p}`);
      thumbMock.mockResolvedValue({ status: "pending" });
      renderViewer(GROUP_ASSETS, 1);

      await waitFor(() => expect(created.length).toBeGreaterThanOrEqual(2));
      const srcs = created.map((c) => c.src);
      expect(srcs).toContain(`asset://${GROUP_ASSETS[0].path}`);
      expect(srcs).toContain(`asset://${GROUP_ASSETS[2].path}`);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});

// --- EXIF 面板（M4 二轮：LR 式分组 文件/图像/拍摄/位置） ------------------------------

describe("查看器：EXIF 面板", () => {
  it("展示核心元数据与库内重复；EXIF 扩展无值行整行隐藏；可收起/展开", async () => {
    const user = userEvent.setup();
    renderViewer();

    const rows = await screen.findByTestId("viewer-exif-rows");
    expect(rows).toHaveTextContent("Canon EOS R5");
    // 镜头后端暂未返回 →「—」（不渲染 undefined）
    expect(rows).toHaveTextContent("—");
    expect(rows).toHaveTextContent("2026-09-18 10:20:30");
    expect(rows).toHaveTextContent("5.0 MB");
    expect(rows).toHaveTextContent(GROUP_ASSETS[0].path);
    expect(rows).toHaveTextContent("2026-09-19 08:00:00");
    const dup = within(rows).getByText("库内重复");
    expect(dup.nextSibling).toHaveTextContent("2 张");

    // 无 EXIF 扩展数据：图像/位置组整组不渲染（无值行不显示「—」）
    expect(screen.queryByTestId("viewer-exif-group-image")).not.toBeInTheDocument();
    expect(screen.queryByTestId("viewer-exif-group-location")).not.toBeInTheDocument();
    expect(screen.queryByText("分辨率")).not.toBeInTheDocument();
    expect(screen.queryByText("ISO")).not.toBeInTheDocument();
    expect(screen.queryByText("光圈")).not.toBeInTheDocument();
    expect(screen.queryByText("快门")).not.toBeInTheDocument();
    expect(screen.queryByText("焦距")).not.toBeInTheDocument();
    expect(rows.textContent).not.toContain("undefined");

    // 收起后保留仅容纳折叠图标的窄轨
    await user.click(screen.getByTestId("viewer-exif-toggle"));
    expect(screen.getByTestId("viewer-exif")).toHaveAttribute("data-open", "false");
    expect(screen.getByTestId("viewer-exif").className).toContain("w-10");
    expect(screen.queryByTestId("viewer-exif-rows")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("viewer-exif-toggle"));
    expect(await screen.findByTestId("viewer-exif-rows")).toBeInTheDocument();
  });

  it("LR 式分组渲染：文件/图像/拍摄/位置组标题 + 组内字段（新契约字段）", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      format: "NEF",
      width: 8192,
      height: 5464,
      megapixels: 44.7,
      aspect: "3:2",
      orientation: 1,
      iso: 400,
      aperture: "2.8",
      shutter: "1/250",
      focalLength: "35",
      flash: "未闪光",
      meteringMode: "评价测光",
      whiteBalance: "自动",
      exposureProgram: "光圈优先",
      software: "Adobe Lightroom",
      gpsLat: 31.2304,
      gpsLon: 121.4737,
    });
    renderViewer();

    const rows = await screen.findByTestId("viewer-exif-rows");
    // 四组标题（小字大写分组）
    expect(within(rows).getByText("文件")).toBeInTheDocument();
    expect(within(rows).getByText("图像")).toBeInTheDocument();
    expect(within(rows).getByText("拍摄")).toBeInTheDocument();
    expect(within(rows).getByText("位置")).toBeInTheDocument();

    const fileGroup = screen.getByTestId("viewer-exif-group-file");
    expect(within(fileGroup).getByText("文件名").nextSibling).toHaveTextContent("IMG_0001.JPG");
    expect(within(fileGroup).getByText("格式").nextSibling).toHaveTextContent("NEF");

    const imageGroup = screen.getByTestId("viewer-exif-group-image");
    expect(within(imageGroup).getByText("分辨率").nextSibling).toHaveTextContent("8192 × 5464");
    expect(within(imageGroup).getByText("总像素").nextSibling).toHaveTextContent("44.7 MP");
    expect(within(imageGroup).getByText("长宽比").nextSibling).toHaveTextContent("3:2");
    expect(within(imageGroup).getByText("方向").nextSibling).toHaveTextContent("横拍");

    const shotGroup = screen.getByTestId("viewer-exif-group-camera");
    expect(within(shotGroup).getByText("闪光灯").nextSibling).toHaveTextContent("未闪光");
    expect(within(shotGroup).getByText("测光").nextSibling).toHaveTextContent("评价测光");
    expect(within(shotGroup).getByText("白平衡").nextSibling).toHaveTextContent("自动");
    expect(within(shotGroup).getByText("曝光程序").nextSibling).toHaveTextContent("光圈优先");
    expect(within(shotGroup).getByText("软件").nextSibling).toHaveTextContent("Adobe Lightroom");

    const locGroup = screen.getByTestId("viewer-exif-group-location");
    expect(within(locGroup).getByText("纬度").nextSibling).toHaveTextContent("31.2304");
    expect(within(locGroup).getByText("经度").nextSibling).toHaveTextContent("121.4737");
  });

  it("orientation 翻译：6 = 竖拍（EXIF 5-8 竖拍 / 1-4 横拍）", async () => {
    detailMock.mockResolvedValue({ ...DETAIL, orientation: 6 });
    renderViewer();

    const imageGroup = await screen.findByTestId("viewer-exif-group-image");
    expect(within(imageGroup).getByText("方向").nextSibling).toHaveTextContent("竖拍");
  });

  it("快门/光圈/焦距格式化：1/250 → 1/250s、f/2.8、35mm", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      iso: 400,
      aperture: "2.8",
      shutter: "1/250",
      focalLength: "35",
    });
    renderViewer();

    const shotGroup = await screen.findByTestId("viewer-exif-group-camera");
    expect(within(shotGroup).getByText("快门").nextSibling).toHaveTextContent("1/250s");
    expect(within(shotGroup).getByText("光圈").nextSibling).toHaveTextContent("f/2.8");
    expect(within(shotGroup).getByText("焦距").nextSibling).toHaveTextContent("35mm");
    expect(within(shotGroup).getByText("ISO").nextSibling).toHaveTextContent("400");
  });

  it("GPS 仅一侧坐标：位置组显示，缺侧为「—」", async () => {
    detailMock.mockResolvedValue({ ...DETAIL, gpsLat: 31.2304 });
    renderViewer();

    const locGroup = await screen.findByTestId("viewer-exif-group-location");
    expect(within(locGroup).getByText("纬度").nextSibling).toHaveTextContent("31.2304");
    expect(within(locGroup).getByText("经度").nextSibling).toHaveTextContent("—");
  });

  it("字段全缺失：核心行显示「—」、重复计数 0 张，绝不渲染 undefined", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      camera: null,
      capturedAt: null,
      createdAt: null,
      dupCount: 0,
    });
    renderViewer();

    const rows = await screen.findByTestId("viewer-exif-rows");
    expect(rows).toHaveTextContent("0 张");
    expect(rows.textContent).not.toContain("undefined");
    // 相机行仍渲染（拍摄组核心行），值为「—」
    expect(within(rows).getByText("相机").nextSibling).toHaveTextContent("—");
  });

  it("assetDetail 失败时保留资产基础信息，不切换成加载动画或空面板", async () => {
    detailMock.mockResolvedValue(null);
    renderViewer();

    const rows = await screen.findByTestId("viewer-exif-rows");
    expect(rows).toHaveTextContent(GROUP_ASSETS[0].path);
    expect(screen.queryByTestId("viewer-exif-loading")).not.toBeInTheDocument();
  });
});

// --- 星标条与切图闪缩（M4.5 wave-3） -----------------------------------------------------

describe("查看器：星标条", () => {
  it("LR 风格快捷键：0-5 评分、P/U 旗标、[ ] 旋转、Z 缩放、I 开关详情", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    await screen.findByTestId("viewer-exif");

    fireEvent.keyDown(window, { key: "4" });
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 4));
    fireEvent.keyDown(window, { key: "0" });
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 0));

    fireEvent.keyDown(window, { key: "P" });
    await waitFor(() => expect(flagMock).toHaveBeenCalledWith(1, true));
    fireEvent.keyDown(window, { key: "U" });
    await waitFor(() => expect(flagMock).toHaveBeenCalledWith(1, false));

    fireEvent.keyDown(window, { key: "[" });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "-90");
    fireEvent.keyDown(window, { key: "]" });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "0");

    fireEvent.keyDown(window, { key: "Z" });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "2.00");
    fireEvent.keyDown(window, { key: "Z" });
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.00");

    fireEvent.keyDown(window, { key: "I" });
    expect(screen.getByTestId("viewer-exif")).toHaveAttribute("data-open", "false");
  });

  it("详情 rating 驱动星级；点星调 assetRatingSet(id, n)，再点同星=清除（0）", async () => {
    detailMock.mockResolvedValue({ ...DETAIL, rating: 3 });
    const user = userEvent.setup();
    renderViewer();

    const bar = await screen.findByTestId("viewer-rating");
    const stars = within(bar).getAllByTestId("viewer-rating-star");
    expect(stars).toHaveLength(5);
    expect(stars[2]).toHaveAttribute("data-filled", "true");
    expect(stars[3]).toHaveAttribute("data-filled", "false");

    await user.click(stars[4]);
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 5));
    // 乐观 UI：第 5 星点亮
    await waitFor(() => expect(within(bar).getAllByTestId("viewer-rating-star")[4]).toHaveAttribute("data-filled", "true"));

    // 再点当前星（5）= 清除
    await user.click(within(bar).getAllByTestId("viewer-rating-star")[4]);
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 0));
  });

  it("详情无 rating：全灰；点第 1 星 → rating 1", async () => {
    detailMock.mockResolvedValue({ ...DETAIL });
    const user = userEvent.setup();
    renderViewer();

    const bar = await screen.findByTestId("viewer-rating");
    for (const star of within(bar).getAllByTestId("viewer-rating-star")) {
      expect(star).toHaveAttribute("data-filled", "false");
    }
    await user.click(within(bar).getAllByTestId("viewer-rating-star")[0]);
    await waitFor(() => expect(ratingMock).toHaveBeenCalledWith(1, 1));
  });
});

describe("查看器：EXIF 面板切图闪缩修复", () => {
  it("切图时保留上一份详情行结构（不回退基础行集），新详情到达后仅值变", async () => {
    // 用 StatefulViewer 驱动切换（与既有用例同模式）
    let setIndex: ((i: number) => void) | undefined;
    let resolveSecond: ((d: AssetDetailDto) => void) | undefined;
    detailMock.mockImplementation(async (id: number) => {
      if (id === 1) return { ...DETAIL, width: 8192, height: 5464, iso: 400 };
      return new Promise<AssetDetailDto>((resolve) => {
        resolveSecond = resolve;
      });
    });
    convertMock.mockImplementation((p: string) => `asset://${p}`);

    function StatefulViewer() {
      const [index, setIdx] = useState(0);
      setIndex = setIdx;
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return (
        <I18nextProvider i18n={i18n}>
          <ViewerOverlay
            asset={GROUP_ASSETS[index]}
            group={group}
            index={index}
            onNavigate={setIdx}
            onClose={() => {}}
          />
        </I18nextProvider>
      );
    }
    render(<StatefulViewer />);
    await screen.findByTestId("viewer");
    // 第一张完整详情到达（图像组存在）
    await waitFor(() =>
      expect(screen.getByTestId("viewer-exif-group-image")).toBeInTheDocument(),
    );

    // 切到第二张：新详情挂起——行结构保持（图像组不消失，显示的是上一份的值）
    act(() => setIndex?.(1));
    await waitFor(() => expect(screen.getByTestId("viewer-exif-rows")).toHaveAttribute("data-asset-id", "1"));
    expect(screen.getByTestId("viewer-exif-group-image")).toBeInTheDocument();

    // 新详情到达：仅值/结构一次更新（data-asset-id 翻到 2）
    act(() => resolveSecond?.({ ...DETAIL, id: 2 }));
    await waitFor(() =>
      expect(screen.getByTestId("viewer-exif-rows")).toHaveAttribute("data-asset-id", "2"),
    );
  });
});
