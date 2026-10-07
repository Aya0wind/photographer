import { assetFixture } from "@/test/fixtures";
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
  assetLabelSet,
  assetRatingSet,
  assetRejectSet,
  assetThumbGet,
  assetVersions,
  type AssetDetailDto,
  type AssetDto,
  type AssetVersions,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetDetail: vi.fn(),
    assetThumbGet: vi.fn(),
    assetFlagSet: vi.fn(),
    assetRatingSet: vi.fn(),
    assetLabelSet: vi.fn(),
    assetRejectSet: vi.fn(),
    assetVersions: vi.fn(),
    clipboardCopyFiles: vi.fn(),
    revealInExplorer: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
  isTauri: vi.fn(() => false),
}));

import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { clipboardCopyFiles, revealInExplorer } from "@/ipc/api";

const detailMock = vi.mocked(assetDetail);
const revealItemMock = vi.mocked(revealItemInDir);
const revealBatchMock = vi.mocked(revealInExplorer);
const copyFilesMock = vi.mocked(clipboardCopyFiles);
const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);
const ratingMock = vi.mocked(assetRatingSet);
const flagMock = vi.mocked(assetFlagSet);
const labelMock = vi.mocked(assetLabelSet);
const rejectMock = vi.mocked(assetRejectSet);
const versionsMock = vi.mocked(assetVersions);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(id: number, kind: AssetDto["kind"], name: string): AssetDto {
  return assetFixture(id, {
    path: `Y:\\照片\\SmartPhoto\\2026\\${name}`,
    name,
    kind,
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 5 * 1024 * 1024,
  });
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
  aiAnalysis: null,
};

function renderViewer(
  assets: AssetDto[] = GROUP_ASSETS,
  index = 0,
  overrides?: {
    onNavigate?: (i: number) => void;
    onClose?: () => void;
    onAssetPatched?: (id: number, patch: Partial<AssetDto>) => void;
    onVersionSelect?: (assetId: number) => void;
  },
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
        onAssetPatched={overrides?.onAssetPatched}
        onVersionSelect={overrides?.onVersionSelect}
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
  vi.mocked(isTauri).mockReturnValue(false);
  revealItemMock.mockReset().mockResolvedValue(undefined);
  revealBatchMock.mockReset().mockResolvedValue(1);
  copyFilesMock.mockReset().mockResolvedValue(undefined);
  detailMock.mockReset().mockResolvedValue(DETAIL);
  ratingMock.mockReset().mockResolvedValue(undefined);
  flagMock.mockReset().mockResolvedValue(undefined);
  labelMock.mockReset().mockResolvedValue(undefined);
  rejectMock.mockReset().mockResolvedValue(undefined);
  versionsMock.mockReset().mockResolvedValue(null);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  resetThumbPipelineForTests();
});

// --- 打开与图片来源 -----------------------------------------------------------------

describe("查看器：打开与图片来源", () => {
  it("photo 优先原图 asset 协议；预览显示当前顺序和总数", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();

    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", `asset://${GROUP_ASSETS[0].path}`);
    expect(img).toHaveAttribute("data-fallback", "original");
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0001.JPG");
    expect(screen.getByTestId("viewer-index")).toHaveTextContent("1 / 3");

    const strip = screen.getByTestId("viewer-filmstrip");
    expect(strip.className).toContain("overflow-hidden");
    expect(strip.className).not.toContain("overflow-x-auto");
    expect(within(strip).getAllByTestId("viewer-filmthumb")).toHaveLength(3);
  });

  it("首次打开就固定 17 个缩略图槽位，翻页时当前照片始终居中", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    function StatefulViewer() {
      const [index, setIndex] = useState(0);
      const group = groupAssetsByDate(GROUP_ASSETS)[0];
      return <I18nextProvider i18n={i18n}><ViewerOverlay asset={GROUP_ASSETS[index]} group={group} index={index} onNavigate={setIndex} onClose={() => {}} /></I18nextProvider>;
    }
    render(<StatefulViewer />);
    const slots = screen.getAllByTestId("viewer-filmstrip-slot");
    expect(slots).toHaveLength(17);
    expect(within(slots[8]).getByTestId("viewer-filmthumb")).toHaveAttribute("data-current", "true");
    expect(slots[7]).toBeEmptyDOMElement();
    fireEvent.click(screen.getByTestId("viewer-next"));
    const nextSlots = screen.getAllByTestId("viewer-filmstrip-slot");
    expect(nextSlots).toHaveLength(17);
    expect(within(nextSlots[8]).getByTestId("viewer-filmthumb")).toHaveAttribute("data-current", "true");
    expect(within(nextSlots[7]).getByTestId("viewer-filmthumb")).toHaveAttribute("data-asset-id", "1");
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

  it("RAW 远图事件回执丢失（后端队满/事件先于订阅）：pending 周期兜底重拉仍升级大图", async () => {
    // 真机复现：点击未预加载的远处照片 → 512 秒出，rawEmbed(6000) pending 后
    // thumbnailReady 丢失 → 旧实现永停 512，切图再切回才出。修复后 hook 周期
    // 重拉，rawEmbed 链最终到达 ready。
    vi.useFakeTimers();
    try {
      const rawAssets = [makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")];
      convertMock.mockImplementation((p: string) => `asset://${p}`);
      let embedCalls = 0;
      thumbMock.mockImplementation(async (id: number, size: number) => {
        if (size === 6000 && id === 7) {
          embedCalls += 1;
          // 第一次 pending（生成排队/任务被丢，无回执事件）；兜底重拉时已就绪
          return embedCalls === 1
            ? { status: "pending" }
            : { status: "ready", path: "C:\\thumbs\\raw-embed-v1\\img7.jpg" };
        }
        return Promise.resolve({ status: "ready", path: `C:\\thumbs\\512\\img${id}.jpg` });
      });
      renderViewer(rawAssets);

      // 首帧：512 档立即显示，rawEmbed 在途
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });
      const img = screen.getByTestId("viewer-img");
      expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\512\\img7.jpg");
      expect(img).toHaveAttribute("data-fallback", "thumb");
      expect(embedCalls).toBe(1);

      // 不发任何 thumbnailReady 事件（回执丢失）→ 2.5s 兜底重拉 → 大图升级
      //（fake timers 下不用 waitFor：其内部 interval 也被冻结，须直接断言）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(2600);
      });
      expect(embedCalls).toBe(2);
      expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "raw-embed");
      expect(screen.getByTestId("viewer-img")).toHaveAttribute(
        "src",
        "asset://C:\\thumbs\\raw-embed-v1\\img7.jpg",
      );
    } finally {
      vi.useRealTimers();
    }
  });
});

// --- 源缺失（missing 终态：源文件被第三方移动/删除） -------------------------------------

describe("查看器：源缺失（missing）", () => {
  it("photo 缺源有历史缓存：降档后显示缓存图 + 琥珀横幅；横幅不阻塞翻图", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (_id: number, size: number) => {
      // 2048 档缺源无缓存（不会再有图）→ 降 512 档；512 档有历史缓存尽力展示
      if (size === 2048) return { status: "missing", cachedPath: null };
      return { status: "missing", cachedPath: "C:\\thumbs\\cache\\2.jpg" };
    });
    const { onNavigate } = renderViewer(GROUP_ASSETS, 1);

    // 原图（磁盘上已不存在）渲染失败 → 逐级降档
    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("data-fallback", "original");
    fireEvent.error(img);

    await waitFor(() =>
      expect(screen.getByTestId("viewer-img")).toHaveAttribute("data-fallback", "thumb"),
    );
    expect(screen.getByTestId("viewer-img")).toHaveAttribute("src", "asset://C:\\thumbs\\cache\\2.jpg");
    // 顶部琥珀警示横幅
    const banner = await screen.findByTestId("viewer-missing-banner");
    expect(banner).toHaveTextContent("源文件已被移动或删除，部分操作不可用");
    expect(screen.queryByTestId("viewer-missing-placeholder")).not.toBeInTheDocument();

    // 横幅不阻塞翻图
    fireEvent.click(screen.getByTestId("viewer-next"));
    expect(onNavigate).toHaveBeenCalledWith(2);
  });

  it("photo 缺源无任何缓存：居中缺源占位（不空白、不无限转圈）", async () => {
    // convertFileSrc 返回空 → 无原图可用，直达 512 档；全链 missing 无缓存
    thumbMock.mockResolvedValue({ status: "missing", cachedPath: null });
    renderViewer();

    // 2026-09-29：居中缺源占位已移除（与顶部横幅重复且切换时闪现）——仅顶部横幅
    expect(await screen.findByTestId("viewer-missing-banner")).toBeInTheDocument();
    expect(screen.queryByTestId("viewer-missing-placeholder")).not.toBeInTheDocument();
    expect(screen.getByTestId("viewer-missing-banner")).toBeInTheDocument();
    // 舞台不留空 img / 不出现加载 spinner（此前真机「stage 空白 + 无限 loading」根因）
    expect(screen.queryByTestId("viewer-img")).not.toBeInTheDocument();
    await waitFor(() => expect(screen.queryByTestId("viewer-loading")).not.toBeInTheDocument());
  });

  it("RAW 缺源（thumb 结算 missing）：内嵌/显影皆缺 → 缓存缩略图尽力展示 + 横幅", async () => {
    const rawAssets = [makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")];
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockImplementation(async (_id: number, size: number) => {
      if (size === 6000 || size === 2048) return { status: "missing", cachedPath: null };
      return { status: "missing", cachedPath: "C:\\thumbs\\cache\\7.jpg" };
    });
    renderViewer(rawAssets);

    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\cache\\7.jpg");
    expect(img).toHaveAttribute("data-fallback", "thumb");
    await screen.findByTestId("viewer-missing-banner");
  });

  it("健康资产（ready）不出现缺源横幅", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockResolvedValue({ status: "ready", path: "C:\\thumbs\\512\\img1.jpg" });
    renderViewer();
    await screen.findByTestId("viewer-img");
    expect(screen.queryByTestId("viewer-missing-banner")).not.toBeInTheDocument();
  });
});

// --- 切换与关闭 ---------------------------------------------------------------------

describe("查看器：左右切换与关闭", () => {
  it("全屏预览挂在窗口顶层，画布不受页面动画或页面高度限制", async () => {
    convertMock.mockImplementation((path: string) => `asset://${path}`);
    const group = groupAssetsByDate(GROUP_ASSETS)[0];
    render(<div data-testid="transformed-page" style={{ transform: "translateY(8px)", height: 300, overflow: "hidden" }}>
      <I18nextProvider i18n={i18n}><ViewerOverlay asset={GROUP_ASSETS[0]} group={group} index={0} onNavigate={() => {}} onClose={() => {}} /></I18nextProvider>
    </div>);
    const viewer = await screen.findByTestId("viewer");
    expect(viewer.parentElement).toBe(document.body);
    expect(viewer.closest('[data-testid="transformed-page"]')).toBeNull();
    expect(screen.getByTestId("viewer-layout")).toHaveClass("absolute", "inset-0");
    expect(screen.getByTestId("viewer-titlebar")).toHaveClass("absolute");
    expect(screen.getByTestId("viewer-filmstrip")).toHaveClass("absolute");
  });
  it("顶部标题和功能按钮默认收回，靠近顶部展开，移开后收回", async () => {
    convertMock.mockImplementation((path: string) => `asset://${path}`);
    renderViewer();
    const viewer = await screen.findByTestId("viewer");
    vi.spyOn(viewer, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, right: 1280, bottom: 800, x: 0, y: 0, width: 1280, height: 800, toJSON: () => ({}) });
    const title = screen.getByTestId("viewer-titlebar");
    const controls = screen.getByTestId("viewer-top-controls");
    expect(title).toHaveAttribute("data-visible", "false");
    expect(controls).toHaveAttribute("inert");
    expect(title).toHaveClass("absolute");
    fireEvent.mouseEnter(screen.getByTestId("viewer-top-edge"));
    expect(title).toHaveAttribute("data-visible", "true");
    fireEvent.mouseMove(viewer, { clientY: 20 });
    expect(title).toHaveAttribute("data-visible", "true");
    expect(controls).not.toHaveAttribute("inert");
    fireEvent(viewer, new MouseEvent("pointerout", { bubbles: true, clientX: 300, clientY: 0, relatedTarget: null }));
    expect(title).toHaveAttribute("data-visible", "true");
    fireEvent.mouseMove(viewer, { clientY: 200 });
    expect(title).toHaveAttribute("data-visible", "false");
    expect(controls).toHaveAttribute("inert");
  });
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

  it("旋转保持 translate→rotate 顺序，缩放由实际尺寸负责", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const img = await screen.findByTestId("viewer-img");

    fireEvent.click(screen.getByTestId("viewer-rotate-cw"));
    await waitFor(() =>
      expect(img.style.transform).toBe("translate(0px, 0px) rotate(90deg)"),
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
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());
    fireEvent.click(screen.getByTestId("viewer-close"));
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("左右方向键逐张翻页；Esc 关闭返回画廊", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onNavigate, onClose } = renderViewer();

    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(onNavigate).toHaveBeenCalledWith(1);
    fireEvent.keyDown(window, { key: "Home" });
    fireEvent.keyDown(window, { key: "End" });
    expect(onNavigate).toHaveBeenCalledTimes(1);

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("提供明确退出按钮；详情抽屉与预览画布并列且可从抽屉内收起", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onClose } = renderViewer();

    const preview = screen.getByTestId("viewer-preview-pane");
    const leftPane = screen.getByTestId("viewer-left-pane");
    const drawer = await screen.findByTestId("viewer-exif");
    expect(preview.parentElement).toBe(leftPane);
    expect(screen.getByTestId("viewer-filmstrip").parentElement).toBe(leftPane);
    expect(leftPane.parentElement).toBe(drawer.parentElement);
    expect(drawer.className).toContain("shrink-0");
    expect(drawer.className).not.toContain("absolute");
    const close = screen.getByTestId("viewer-close");
    expect(close.parentElement?.parentElement).toBe(screen.getByTestId("viewer-stage"));
    expect(close.className).toContain("rounded-full");

    fireEvent.click(screen.getByTestId("viewer-exif-toggle"));
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());
    fireEvent.click(screen.getByTestId("viewer-exif-toggle"));
    expect(await screen.findByTestId("viewer-exif")).toHaveAttribute("data-open", "true");
    expect(await screen.findByTestId("viewer-exif-rows")).toBeInTheDocument();

    fireEvent.click(close);
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1));
  });

  it("F11 切换窗口全屏，进入时自动收起详细信息", async () => {
    const setFullscreen = vi.fn().mockResolvedValue(undefined);
    const isFullscreen = vi.fn().mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    vi.mocked(isTauri).mockReturnValue(true);
    vi.mocked(getCurrentWindow).mockReturnValue({ setFullscreen, isFullscreen } as unknown as ReturnType<typeof getCurrentWindow>);
    renderViewer();
    expect(screen.getByTestId("viewer-exif")).toBeInTheDocument();
    const controls = screen.getByTestId("viewer-fullscreen").parentElement!;
    expect(screen.getByTestId("viewer-exif-toggle").nextElementSibling).toBe(screen.getByTestId("viewer-fullscreen"));
    expect(screen.getByTestId("viewer-edit").parentElement).toBe(controls);
    expect(screen.getByTestId("viewer-rotate-ccw").parentElement).toBe(controls);
    expect(screen.getByTestId("viewer-rotate-cw").parentElement).toBe(controls);
    fireEvent.keyDown(window, { key: "F11" });
    await waitFor(() => expect(setFullscreen).toHaveBeenCalledWith(true));
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());
    fireEvent.keyDown(window, { key: "F11" });
    await waitFor(() => expect(setFullscreen).toHaveBeenCalledWith(false));
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
    await waitFor(() => expect(screen.getByTestId("viewer-img").style.transform).toContain("rotate(-360deg)"));
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

  it("Home/End 不再快速跳到首尾", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onNavigate } = renderViewer(GROUP_ASSETS, 1);
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "Home" });
    fireEvent.keyDown(window, { key: "End" });
    expect(onNavigate).not.toHaveBeenCalled();
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
      const strip = screen.getByTestId("viewer-filmstrip");
      expect(strip).toHaveClass("absolute");
      expect(strip).toHaveAttribute("data-visible", "true");

      // 3s 后淡出（CSS opacity 过渡：类切换确定）
      await act(async () => {
        await vi.advanceTimersByTimeAsync(3250);
      });
      expect(screen.getByTestId("viewer-hint").className).toContain("opacity-0");
      expect(strip).toHaveAttribute("data-visible", "false");
      expect(strip).toHaveAttribute("inert");
      expect(screen.getByTestId("viewer-prev")).toHaveAttribute("inert");
      expect(screen.getByTestId("viewer-next")).toHaveAttribute("inert");
      fireEvent.mouseMove(screen.getByTestId("viewer"));
      expect(strip).toHaveAttribute("data-visible", "true");
      expect(screen.getByTestId("viewer-next")).not.toHaveAttribute("inert");
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      fireEvent.wheel(screen.getByTestId("viewer-stage"), { deltaY: -1 });
      await act(async () => { await vi.advanceTimersByTimeAsync(2000); });
      expect(strip).toHaveAttribute("data-visible", "true");
      await act(async () => { await vi.advanceTimersByTimeAsync(1100); });
      expect(strip).toHaveAttribute("data-visible", "false");

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

  it("底部缩略图只显示当前照片前后各 8 张，点击最右边最多跳 8 张", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const manyAssets = Array.from({ length: 40 }, (_, i) => makeAsset(i + 1, "photo", `IMG_${i + 1}.JPG`));
    const { onNavigate } = renderViewer(manyAssets, 20);
    const thumbs = within(screen.getByTestId("viewer-filmstrip")).getAllByTestId("viewer-filmthumb");
    expect(thumbs).toHaveLength(17);
    expect(thumbs[0]).toHaveAttribute("data-asset-id", "13");
    expect(thumbs[16]).toHaveAttribute("data-asset-id", "29");
    fireEvent.click(thumbs[16]);
    expect(onNavigate).toHaveBeenCalledWith(28);
  });

  it("补页改变缩略图位置时保留已解码的图片，新图加载前显示序号", async () => {
    const initial = Array.from({ length: 20 }, (_, i) => makeAsset(i + 1, "photo", `IMG_${i + 1}.JPG`));
    const inserted = Array.from({ length: 4 }, (_, i) => makeAsset(i + 100, "photo", `NEW_${i}.JPG`));
    thumbMock.mockImplementation(async (id: number) => ({ status: "ready", path: `C:\\thumbs\\${id}.jpg` }));
    convertMock.mockImplementation((path: string) => `asset://${path}`);
    function StatefulViewer() {
      const [items, setItems] = useState(initial);
      const index = items.findIndex((item) => item.id === 10);
      return <I18nextProvider i18n={i18n}>
        <button onClick={() => setItems([...items.slice(0, 4), ...inserted, ...items.slice(4)])} data-testid="append-page">append</button>
        <ViewerOverlay asset={items[index]} group={{ key: "__viewer__", date: null, assets: items }} index={index} onNavigate={() => {}} onClose={() => {}} />
      </I18nextProvider>;
    }
    render(<StatefulViewer />);
    const tile = (id: number) => screen.getAllByTestId("viewer-filmthumb").find((element) => element.getAttribute("data-asset-id") === String(id));
    await waitFor(() => expect(tile(5)?.querySelector("img")).not.toBeNull());
    for (const image of screen.getAllByTestId("viewer-filmthumb-cell-img")) fireEvent.load(image);
    expect(tile(5)?.querySelector("img")?.className).toContain("opacity-100");
    fireEvent.click(screen.getByTestId("append-page"));
    expect(tile(5)?.querySelector("img")?.className).toContain("opacity-100");
    await waitFor(() => expect(tile(101)?.querySelector("img")).not.toBeNull());
    expect(within(tile(101)!).getByTestId("thumb-mini")).toBeInTheDocument();
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

    // 即使 onLoad 已触发，也必须等浏览器 decode 完成，期间旧图继续显示。
    let finishDecode: (() => void) | undefined;
    Object.defineProperty(img2, "decode", {
      configurable: true,
      value: vi.fn(
        () =>
          new Promise<void>((resolve) => {
            finishDecode = resolve;
          }),
      ),
    });
    fireEvent.load(img2);
    await act(async () => Promise.resolve());
    expect(screen.getByTestId("viewer-img-prev")).toHaveAttribute("src", url1);
    expect(img2.className).toContain("invisible");

    // decode 后新图先接管并保留旧图至少一帧，随后才清理旧层。
    act(() => finishDecode?.());
    await waitFor(() =>
      expect(screen.queryByTestId("viewer-img-prev")).toHaveAttribute("src", url2),
    );
    // 提交后沿用已经完成解码的同一个 DOM 节点，不把 src 写到另一节点上重新绘制。
    expect(screen.getByTestId("viewer-img-prev")).toBe(img2);
    expect(screen.queryByTestId("viewer-img")).not.toBeInTheDocument();
    await waitFor(() =>
      expect(screen.queryByTestId("viewer-img-retiring")).not.toBeInTheDocument(),
    );
  });

  it("胶片条只渲染当前照片附近，避免加载整组缩略图", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const bigGroup = Array.from({ length: 48 }, (_, i) =>
      makeAsset(i + 1, "photo", `IMG_${String(i + 1).padStart(4, "0")}.JPG`),
    );
    renderViewer(bigGroup);

    await screen.findByTestId("viewer");
    const thumbs = screen.getAllByTestId("viewer-filmthumb");
    expect(thumbs.length).toBe(9);
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
    expect(rows).not.toHaveTextContent(GROUP_ASSETS[0].path);
    expect(rows).toHaveTextContent("2026-09-19 08:00:00");
    const dup = within(rows).getByText("库内重复");
    expect(dup.nextSibling).toHaveTextContent("2 张");

    // 无 EXIF 扩展数据：位置组不渲染；图像组保留 AI 选片行（未分析态，C 阶段）
    expect(screen.getByTestId("viewer-exif-group-image")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-ai-eyes")).toHaveAttribute("data-state", "not_analyzed");
    expect(screen.getByTestId("viewer-ai-blur")).toHaveAttribute("data-soft", "false");
    expect(screen.getAllByTestId("viewer-ai-badge").length).toBe(2);
    expect(screen.queryByTestId("viewer-exif-group-location")).not.toBeInTheDocument();
    expect(screen.queryByText("分辨率")).not.toBeInTheDocument();
    expect(screen.queryByText("ISO")).not.toBeInTheDocument();
    expect(screen.queryByText("光圈")).not.toBeInTheDocument();
    expect(screen.queryByText("快门")).not.toBeInTheDocument();
    expect(screen.queryByText("焦距")).not.toBeInTheDocument();
    expect(rows.textContent).not.toContain("undefined");

    // 收起后保留仅容纳折叠图标的窄轨
    await user.click(screen.getByTestId("viewer-exif-toggle"));
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());

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

  it("小于一秒的十进制快门显示分数，文件路径不进入详细信息", async () => {
    detailMock.mockResolvedValue({ ...DETAIL, shutter: "0.02" });
    renderViewer();
    const shotGroup = await screen.findByTestId("viewer-exif-group-camera");
    expect(within(shotGroup).getByText("快门").nextSibling).toHaveTextContent("1/50s");
    expect(screen.queryByText("文件路径")).not.toBeInTheDocument();
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
    expect(rows).not.toHaveTextContent(GROUP_ASSETS[0].path);
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
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());
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

// --- 大图右键菜单（②：与瓦片同款，作用于当前资产） ---------------------------------------

describe("查看器：大图右键菜单", () => {
  it("右键舞台出菜单；旗标调 assetFlagSet(当前资产)；reveal/copy 调对应通道", async () => {
    const user = userEvent.setup();
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onClose } = renderViewer();

    fireEvent.contextMenu(screen.getByTestId("viewer-stage"), { clientX: 300, clientY: 200 });
    const menu = await screen.findByTestId("asset-context-menu");
    expect(within(menu).getByTestId("asset-context-menu-item-reveal")).toBeInTheDocument();

    await user.click(within(menu).getByTestId("asset-context-menu-item-flag"));
    await waitFor(() => expect(flagMock).toHaveBeenCalledWith(1, true));

    fireEvent.contextMenu(screen.getByTestId("viewer-stage"), { clientX: 300, clientY: 200 });
    await user.click(
      within(await screen.findByTestId("asset-context-menu")).getByTestId(
        "asset-context-menu-item-reveal",
      ),
    );
    await waitFor(() => expect(revealBatchMock).toHaveBeenCalledWith([GROUP_ASSETS[0].path]));
    expect(revealItemMock).not.toHaveBeenCalled();

    fireEvent.contextMenu(screen.getByTestId("viewer-stage"), { clientX: 300, clientY: 200 });
    await user.click(
      within(await screen.findByTestId("asset-context-menu")).getByTestId(
        "asset-context-menu-item-copy",
      ),
    );
    await waitFor(() => expect(copyFilesMock).toHaveBeenCalledWith([GROUP_ASSETS[0].path]));
    void onClose;
  });

  it("菜单打开时 Esc 关菜单不关查看器", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onClose } = renderViewer();

    fireEvent.contextMenu(screen.getByTestId("viewer-stage"), { clientX: 300, clientY: 200 });
    expect(await screen.findByTestId("asset-context-menu")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() =>
      expect(screen.queryByTestId("asset-context-menu")).not.toBeInTheDocument(),
    );
    expect(screen.getByTestId("viewer")).toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });
});

// --- 选片补全（B1）：颜色标签 / 拒绝旗标 -----------------------------------------------

describe("查看器：颜色标签（星级行旁色点）", () => {
  it("默认无色标：色点空心；点开菜单选红色 → assetLabelSet([id],'red') + onAssetPatched 回传", async () => {
    const user = userEvent.setup();
    const onAssetPatched = vi.fn();
    renderViewer(GROUP_ASSETS, 0, { onAssetPatched });
    await screen.findByTestId("viewer-rating");

    expect(screen.getByTestId("viewer-color")).toHaveAttribute("data-label", "none");

    await user.click(screen.getByTestId("viewer-color"));
    const menu = screen.getByTestId("viewer-color-menu");
    const red = within(menu)
      .getAllByTestId("viewer-color-option")
      .find((el) => el.getAttribute("data-label") === "red");
    await user.click(red as HTMLElement);

    await waitFor(() => expect(labelMock).toHaveBeenCalledWith([1], "red"));
    expect(onAssetPatched).toHaveBeenCalledWith(1, { colorLabel: "red" });
  });

  it("已有色标（asset.colorLabel=green）：色点回显；点清除 → assetLabelSet([id],null)", async () => {
    const user = userEvent.setup();
    const onAssetPatched = vi.fn();
    const labeled = [{ ...GROUP_ASSETS[0], colorLabel: "green" }];
    renderViewer(labeled, 0, { onAssetPatched });
    await screen.findByTestId("viewer-rating");

    expect(screen.getByTestId("viewer-color")).toHaveAttribute("data-label", "green");

    await user.click(screen.getByTestId("viewer-color"));
    await user.click(screen.getByTestId("viewer-color-clear"));
    await waitFor(() => expect(labelMock).toHaveBeenCalledWith([1], null));
    expect(onAssetPatched).toHaveBeenCalledWith(1, { colorLabel: null });
  });
});

describe("查看器：拒绝旗标（与星级分层）", () => {
  it("未拒绝：点按钮 → assetRejectSet([id],true)；X 键切换 → false", async () => {
    const user = userEvent.setup();
    const onAssetPatched = vi.fn();
    renderViewer(GROUP_ASSETS, 0, { onAssetPatched });
    await screen.findByTestId("viewer-rating");

    expect(screen.getByTestId("viewer-reject")).toHaveAttribute("aria-pressed", "false");
    await user.click(screen.getByTestId("viewer-reject"));
    await waitFor(() => expect(rejectMock).toHaveBeenCalledWith([1], true));
    expect(onAssetPatched).toHaveBeenCalledWith(1, { rejected: true });

    // X 键：已拒绝（draft=true）→ 切回 false
    fireEvent.keyDown(window, { key: "x" });
    await waitFor(() => expect(rejectMock).toHaveBeenLastCalledWith([1], false));
    expect(onAssetPatched).toHaveBeenLastCalledWith(1, { rejected: false });
  });

  it("已拒绝资产：按钮点亮（aria-pressed=true），可再点取消", async () => {
    const user = userEvent.setup();
    const rejected = [{ ...GROUP_ASSETS[0], rejected: true }];
    renderViewer(rejected, 0);
    await screen.findByTestId("viewer-rating");

    const btn = screen.getByTestId("viewer-reject");
    expect(btn).toHaveAttribute("aria-pressed", "true");
    await user.click(btn);
    await waitFor(() => expect(rejectMock).toHaveBeenCalledWith([1], false));
  });
});

// --- 版本关系（B2）：版本区 chips --------------------------------------------------------

const VERSIONS_GROUP: AssetVersions = {
  groupId: 5,
  members: [
    { assetId: 1, role: "raw", name: "IMG_0001.NEF", thumbReady: true },
    { assetId: 9, role: "derived", name: "IMG_0001_edit_v1.jpg", thumbReady: true },
  ],
};

describe("查看器：版本区（B2）", () => {
  it("RAW+JPG 只在详情版本区切换，JPG 使用简洁名称", async () => {
    versionsMock.mockResolvedValue({ groupId: 5, members: [
      { assetId: 1, role: "raw", name: "IMG_0001.NEF", thumbReady: true },
      { assetId: 9, role: "sooc", name: "IMG_0001.JPG", thumbReady: true },
    ] });
    const user = userEvent.setup();
    const onVersionSelect = vi.fn();
    renderViewer(GROUP_ASSETS, 0, { onVersionSelect });
    await screen.findByTestId("viewer-versions");
    expect(screen.queryByTestId("viewer-format-switch")).not.toBeInTheDocument();
    const chip = screen.getAllByTestId("viewer-version-chip").find((node) => node.dataset.role === "sooc")!;
    expect(chip).toHaveTextContent("JPG");
    expect(chip).not.toHaveTextContent("机内");
    await user.click(chip);
    expect(onVersionSelect).toHaveBeenCalledWith(9);
  });

  it("多成员：渲染 RAW/成片 chips（成片带标），当前项高亮；点击 → onVersionSelect(成员 id)", async () => {
    const user = userEvent.setup();
    versionsMock.mockResolvedValue(VERSIONS_GROUP);
    const onVersionSelect = vi.fn();
    renderViewer(GROUP_ASSETS, 0, { onVersionSelect });
    await screen.findByTestId("viewer-versions");

    const chips = screen.getAllByTestId("viewer-version-chip");
    expect(chips).toHaveLength(2);
    expect(chips[0]).toHaveAttribute("data-role", "raw");
    expect(chips[0]).toHaveAttribute("data-current", "true");
    expect(chips[1]).toHaveAttribute("data-role", "derived");
    expect(within(chips[1]).getByTestId("viewer-version-derived-badge")).toHaveTextContent("成片");
    expect(within(chips[1]).getByText("IMG_0001_edit_v1.jpg")).toBeInTheDocument();

    await user.click(chips[1]);
    expect(onVersionSelect).toHaveBeenCalledWith(9);
  });

  it("孤片（members 只有自己）：不渲染版本区；asset_versions 失败同样不渲染", async () => {
    versionsMock.mockResolvedValue({ groupId: null, members: [{ assetId: 1, role: null, name: "IMG_0001.JPG", thumbReady: false }] });
    renderViewer(GROUP_ASSETS, 0);
    await screen.findByTestId("viewer-rating");
    await waitFor(() => expect(versionsMock).toHaveBeenCalledWith(1));
    expect(screen.queryByTestId("viewer-versions")).not.toBeInTheDocument();
  });
});

// --- AI 选片行（C 阶段） ----------------------------------------------------------------

describe("查看器：AI 选片行（C）", () => {
  it("已分析：闭眼三态文案 + 清晰度分与软片标黄 + 「AI 建议」徽标", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      aiAnalysis: {
        eyes: { value: "closed", score: 92 },
        blur: { value: "soft", score: 35 },
      },
    });
    renderViewer();
    await screen.findByTestId("viewer-ai-eyes");

    const eyes = screen.getByTestId("viewer-ai-eyes");
    expect(eyes).toHaveAttribute("data-state", "closed");
    expect(eyes).toHaveTextContent("有人闭眼");
    const blur = screen.getByTestId("viewer-ai-blur");
    expect(blur).toHaveAttribute("data-soft", "true");
    expect(blur).toHaveTextContent("35");
    // AI 建议徽标（两行各一枚，弱样式层级）
    expect(screen.getAllByTestId("viewer-ai-badge")).toHaveLength(2);
  });

  it("已分析无人脸：文案「未检出人脸（无法判定闭眼）」与未分析区分", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      aiAnalysis: { blur: { value: "sharp", score: 88 } },
    });
    renderViewer();
    const eyes = await screen.findByTestId("viewer-ai-eyes");
    expect(eyes).toHaveAttribute("data-state", "no_face");
    expect(eyes).toHaveTextContent("未检出人脸（无法判定闭眼）");
    expect(screen.getByTestId("viewer-ai-blur")).toHaveAttribute("data-soft", "false");
  });

  it("可能闭眼：maybe 文案", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      aiAnalysis: { eyes: { value: "maybe", score: 60 } },
    });
    renderViewer();
    const eyes = await screen.findByTestId("viewer-ai-eyes");
    expect(eyes).toHaveAttribute("data-state", "maybe");
    expect(eyes).toHaveTextContent("可能闭眼");
  });
});
