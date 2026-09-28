import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import AssetThumb from "./AssetThumb";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { assetThumbGet, type AssetKind } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, assetThumbGet: vi.fn() };
});
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const thumbMock = vi.mocked(assetThumbGet);
const convertMock = vi.mocked(convertFileSrc);

function renderThumb(kind: AssetKind, name: string, id = 1) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AssetThumb asset={{ id, kind, name }} size={240} testId="thumb" />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  resetThumbPipelineForTests();
});

describe("AssetThumb 骨架屏（加载中 vs 永久无图）", () => {
  it("节点复用到另一张照片时先显示序号，解码完成后才显示新图", async () => {
    thumbMock.mockImplementation(async (id: number) => ({ status: "ready", path: `C:\\thumbs\\${id}.jpg` }));
    convertMock.mockImplementation((path: string) => `asset://${path}`);
    const draw = (id: number) => <I18nextProvider i18n={i18n}><AssetThumb asset={{ id, kind: "photo", name: `${id}.jpg` }} size={240} miniLabel={String(id)} skeleton={false} testId="thumb" /></I18nextProvider>;
    const { rerender } = render(draw(1));
    await waitFor(() => expect(screen.getByTestId("thumb-img")).toHaveAttribute("src", "asset://C:\\thumbs\\1.jpg"));
    fireEvent.load(screen.getByTestId("thumb-img"));
    expect(screen.getByTestId("thumb-img").className).toContain("opacity-100");
    rerender(draw(2));
    await waitFor(() => expect(screen.getByTestId("thumb-img")).toHaveAttribute("src", "asset://C:\\thumbs\\2.jpg"));
    expect(screen.getByTestId("thumb-img").className).toContain("opacity-0");
    expect(screen.getByTestId("thumb-mini")).toHaveTextContent("2");
    fireEvent.load(screen.getByTestId("thumb-img"));
    expect(screen.getByTestId("thumb-img").className).toContain("opacity-100");
    expect(screen.queryByTestId("thumb-mini")).not.toBeInTheDocument();
  });
  it("加载中：容器挂 sp-skeleton + kind 图形叠加", async () => {
    thumbMock.mockImplementation(() => new Promise(() => undefined)); // 永不在途结算
    renderThumb("photo", "IMG_0001.JPG");

    const container = await screen.findByTestId("thumb");
    expect(container.className).toContain("sp-skeleton");
    expect(container).toHaveAttribute("data-loading", "true");
    // kind 图形保持（辨识度不丢）
    expect(container.querySelector('[data-testid="thumb-photo"]')).not.toBeNull();
  });

  it("结算为无图：骨架退静态（无动画类），kind 图形保留", async () => {
    thumbMock.mockResolvedValue({ status: "unavailable" });
    renderThumb("photo", "IMG_0001.JPG");

    const container = await screen.findByTestId("thumb");
    await waitFor(() => expect(container).toHaveAttribute("data-loading", "false"));
    expect(container.className).not.toContain("sp-skeleton");
    expect(container.querySelector('[data-testid="thumb-photo"]')).not.toBeNull();
  });

  it("raw：请求缩略图 + 水印角标恒在", async () => {
    thumbMock.mockResolvedValue({ status: "ready", path: "C:\\thumbs\\256\\1.jpg" });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderThumb("raw", "IMG_0002.CR3");

    const container = await screen.findByTestId("thumb");
    await waitFor(() => expect(container.querySelector("img")).not.toBeNull());
    expect(container.querySelector("img")).toHaveAttribute("loading", "eager");
    expect(thumbMock).toHaveBeenCalledWith(1, 240);
    expect(container.querySelector('[data-testid="thumb-raw-badge"]')).not.toBeNull();
  });
});

// --- 源缺失角标（missing 终态：源文件被移动/删除） --------------------------------------

describe("AssetThumb 源缺失角标（missing）", () => {
  it("missing+历史缓存：照常显示缓存图 + 右下角「源缺失」角标", async () => {
    thumbMock.mockResolvedValue({ status: "missing", cachedPath: "C:\\thumbs\\cache\\1.jpg" });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    renderThumb("photo", "IMG_0001.JPG");

    const img = await screen.findByTestId("thumb-img");
    expect(img).toHaveAttribute("src", "asset://C:\\thumbs\\cache\\1.jpg");
    fireEvent.load(img);
    expect(screen.getByTestId("thumb-missing-badge")).toHaveTextContent("源缺失");
    expect(screen.queryByTestId("thumb-photo")).not.toBeInTheDocument();
  });

  it("missing 无缓存：kind 占位图形 + 同款小角标（failed 占位语义不变）", async () => {
    thumbMock.mockResolvedValue({ status: "missing", cachedPath: null });
    renderThumb("photo", "IMG_0001.JPG");

    const container = await screen.findByTestId("thumb");
    await waitFor(() => expect(screen.getByTestId("thumb-missing-badge")).toBeInTheDocument());
    expect(screen.getByTestId("thumb-missing-badge")).toHaveTextContent("源缺失");
    // 无缓存图：kind 占位图形保留，不渲染 img
    expect(container.querySelector('[data-testid="thumb-photo"]')).not.toBeNull();
    expect(container.querySelector("img")).toBeNull();
  });
});
