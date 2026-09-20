import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor } from "@testing-library/react";
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

  it("video（M8 海报）：进缩略图管线——pending 骨架、ready 出图+播放角标", async () => {
    thumbMock.mockResolvedValueOnce({ status: "pending" });
    renderThumb("video", "VID_0001.MP4");
    await screen.findByTestId("thumb");
    expect(thumbMock).toHaveBeenCalledWith(1, 240);
    // pending：与 photo 同走骨架等待（海报在后台生成）
    const container = screen.getByTestId("thumb");
    expect(container.className).toContain("sp-skeleton");

    // ready：出图 + 播放角标
    thumbMock.mockResolvedValue({ status: "ready", path: "C:\\thumbs\\video-256-v1\\1.jpg" });
    convertMock.mockImplementation((p: string) => `asset://${p}`);
    const { unmount } = render(
      <I18nextProvider i18n={i18n}>
        <AssetThumb asset={{ id: 2, kind: "video", name: "VID_0002.MP4" }} size={240} testId="thumb2" />
      </I18nextProvider>,
    );
    const c2 = await screen.findByTestId("thumb2");
    await waitFor(() => expect(c2.querySelector("img")).not.toBeNull());
    expect(c2.querySelector('[data-testid="thumb-video-play"]')).not.toBeNull();
    unmount();
  });

  it("video：海报提取失败（unavailable）→ 回退 kind 占位、不吃骨架", async () => {
    thumbMock.mockResolvedValue({ status: "unavailable" });
    renderThumb("video", "VID_0001.MP4");

    const container = await screen.findByTestId("thumb");
    await waitFor(() => expect(container).toHaveAttribute("data-loading", "false"));
    expect(container.className).not.toContain("sp-skeleton");
    expect(container.querySelector('[data-testid="thumb-video"]')).not.toBeNull();
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
