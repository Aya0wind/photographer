import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import ViewerOverlay from "./ViewerOverlay";
import { groupAssetsByDate, type AssetGroup } from "../lib/assetGroups";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { assetDetail, assetThumbGet, type AssetDetailDto, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetDetail: vi.fn(),
    assetThumbGet: vi.fn(),
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
  thumbMock.mockReset().mockResolvedValue(null);
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
      size === 2048 ? "C:\\thumbs\\2048\\img1.jpg" : null,
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
      size === 2048 ? null : "C:\\thumbs\\512\\img1.jpg",
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

  it("RAW 无原图：直接用大档缩略图（512 档），不尝试原图 asset 协议与 2048 中间档", async () => {
    const rawAssets = [makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")];
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    thumbMock.mockResolvedValue("C:\\thumbs\\512\\img7.jpg");
    renderViewer(rawAssets);

    // 不尝试原图（CR3 无 inline 提取），直接大档缩略图
    const img = await screen.findByTestId("viewer-img");
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\512\\img7.jpg");
    expect(img).toHaveAttribute("data-fallback", "thumb");
    expect(convertMock).not.toHaveBeenCalledWith(rawAssets[0].path);
    expect(thumbMock).toHaveBeenCalledWith(7, 1280);
    expect(thumbMock).not.toHaveBeenCalledWith(7, 2048);
  });

  it("RAW 无缩略图（后端恒 None 的兜底）：永久占位不崩溃", async () => {
    renderViewer([makeAsset(7, "raw", "IMG_0007.CR3"), makeAsset(8, "raw", "IMG_0008.CR3")]);
    await screen.findByTestId("viewer-placeholder");
    expect(document.querySelector("img[data-testid='viewer-img']")).toBeNull();
  });
});

// --- 切换与关闭 ---------------------------------------------------------------------

describe("查看器：左右切换与关闭", () => {
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
    expect(await screen.findByText("IMG_0002.JPG")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-prev")).toBeEnabled();

    // 键盘 → 第三张；此时 next 禁用
    fireEvent.keyDown(window, { key: "ArrowRight" });
    expect(await screen.findByText("IMG_0003.JPG")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-next")).toBeDisabled();

    // 键盘回退一张
    fireEvent.keyDown(window, { key: "ArrowLeft" });
    expect(await screen.findByText("IMG_0002.JPG")).toBeInTheDocument();
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

  it("胶片条点击跳转；Esc 关闭返回画廊", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { onNavigate, onClose } = renderViewer();

    const thumbs = screen.getAllByTestId("viewer-filmthumb");
    fireEvent.click(thumbs[2]);
    expect(onNavigate).toHaveBeenCalledWith(2);

    fireEvent.keyDown(window, { key: "Escape" });
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
  it("wheel 放大至 1.2x（钳制 1x-4x），双击复位", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderViewer();
    const stage = await screen.findByTestId("viewer-stage");
    expect(stage).toHaveAttribute("data-scale", "1.00");

    fireEvent.wheel(stage, { deltaY: -100 });
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
    // 逆时针回退；继续逆时针从 0 回绕到 270
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "90");
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    fireEvent.click(screen.getByTestId("viewer-rotate-ccw"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "270");

    // 双击复位：缩放/平移/旋转全部归零
    fireEvent.dblClick(screen.getByTestId("viewer-stage"));
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-rotation", "0");
    expect(screen.getByTestId("viewer-stage")).toHaveAttribute("data-scale", "1.00");
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
    expect(await screen.findByText("IMG_0001.JPG")).toBeInTheDocument();
    fireEvent.keyDown(window, { key: "End" });
    expect(await screen.findByText("IMG_0003.JPG")).toBeInTheDocument();
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

  it("胶片条当前格自动滚入可视区（scrollIntoView inline nearest）", async () => {
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const calls: Array<Record<string, unknown>> = [];
    class FakeImage {
      src = "";
    }
    vi.stubGlobal("Image", FakeImage);
    const original = HTMLElement.prototype.scrollIntoView;
    HTMLElement.prototype.scrollIntoView = function (arg?: unknown) {
      calls.push((arg ?? {}) as Record<string, unknown>);
    } as typeof HTMLElement.prototype.scrollIntoView;
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

      fireEvent.click(screen.getByTestId("viewer-next"));
      await waitFor(() => {
        const last = calls[calls.length - 1];
        expect(last).toMatchObject({ block: "nearest", inline: "nearest" });
      });
    } finally {
      HTMLElement.prototype.scrollIntoView = original;
      vi.unstubAllGlobals();
    }
  });
});

// --- 切换交叉淡入与胶片条 ------------------------------------------------------------

describe("查看器：交叉淡入与胶片条", () => {
  it("双图层交叉淡入：旧图层保留至新图 onLoad 提交（切换无空窗）", async () => {
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

    // 切到第二张：新图层挂载（未 onLoad 前 opacity-0），旧图层仍在
    act(() => setIndex?.(1));
    const img2 = await screen.findByTestId("viewer-img");
    expect(img2).toHaveAttribute("src", url2);
    expect(img2.className).toContain("opacity-0");
    expect(screen.getByTestId("viewer-img-prev")).toHaveAttribute("src", url1);

    // 新图 onLoad → 150ms 淡入提交：旧图层移除，新图成为底层
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
      thumbMock.mockResolvedValue(null);
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

// --- EXIF 面板 ---------------------------------------------------------------------

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

    // 无 EXIF 扩展数据：尺寸/ISO/光圈/快门/焦距行不渲染（无值行不显示「—」）
    expect(screen.queryByText("尺寸")).not.toBeInTheDocument();
    expect(screen.queryByText("ISO")).not.toBeInTheDocument();
    expect(screen.queryByText("光圈")).not.toBeInTheDocument();
    expect(screen.queryByText("快门")).not.toBeInTheDocument();
    expect(screen.queryByText("焦距")).not.toBeInTheDocument();
    expect(rows.textContent).not.toContain("undefined");

    // 收起（AnimatePresence 退场 → waitFor）
    await user.click(screen.getByTestId("viewer-exif-toggle"));
    await waitFor(() => expect(screen.queryByTestId("viewer-exif")).not.toBeInTheDocument());

    await user.click(screen.getByTestId("viewer-exif-toggle"));
    expect(await screen.findByTestId("viewer-exif-rows")).toBeInTheDocument();
  });

  it("EXIF 扩展字段存在时展示尺寸/ISO/光圈/快门/焦距（契约扩展后自动出现）", async () => {
    detailMock.mockResolvedValue({
      ...DETAIL,
      width: 8192,
      height: 5464,
      iso: 400,
      aperture: 2.8,
      shutter: "1/250",
      focalLength: 35,
    });
    renderViewer();

    const rows = await screen.findByTestId("viewer-exif-rows");
    expect(within(rows).getByText("尺寸").nextSibling).toHaveTextContent("8192 × 5464");
    expect(within(rows).getByText("ISO").nextSibling).toHaveTextContent("400");
    expect(within(rows).getByText("光圈").nextSibling).toHaveTextContent("f/2.8");
    expect(within(rows).getByText("快门").nextSibling).toHaveTextContent("1/250");
    expect(within(rows).getByText("焦距").nextSibling).toHaveTextContent("35 mm");
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
    // 相机/拍摄时间/入库时间行仍渲染（核心行），值为「—」/空时间
    expect(within(rows).getByText("相机").nextSibling).toHaveTextContent("—");
  });

  it("assetDetail 失败（后端不可用）：面板显示降级文案", async () => {
    detailMock.mockResolvedValue(null);
    renderViewer();

    expect(await screen.findByTestId("viewer-exif-unavailable")).toHaveTextContent("不可用");
  });
});
