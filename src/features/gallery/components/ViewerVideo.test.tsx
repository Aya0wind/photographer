import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import ViewerVideo from "./ViewerVideo";
import { supportsHevc } from "../lib/hevc";
import { openWithSystem } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, openWithSystem: vi.fn() };
});
vi.mock("@tauri-apps/api/core", () => ({
  convertFileSrc: vi.fn((p: string) => `asset://${p}`),
}));
vi.mock("../lib/hevc", () => ({ supportsHevc: vi.fn() }));

const openMock = vi.mocked(openWithSystem);
const hevcMock = vi.mocked(supportsHevc);

function renderVideo(path = "Y:\\照片\\C0121.MP4", posterUrl: string | null = null) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ViewerVideo path={path} posterUrl={posterUrl} />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  openMock.mockReset().mockResolvedValue(undefined);
  hevcMock.mockReset().mockReturnValue(true);
});

describe("查看器视频舞台（M8）", () => {
  it("渲染 <video>：asset 协议 src + 海报 poster + controls", () => {
    renderVideo("Y:\\照片\\C0121.MP4", "asset://thumb.jpg");
    const video = screen.getByTestId("viewer-video");
    expect(video.tagName).toBe("VIDEO");
    expect(video).toHaveAttribute("src", "asset://Y:\\照片\\C0121.MP4");
    expect(video).toHaveAttribute("poster", "asset://thumb.jpg");
    expect(video).toHaveAttribute("controls");
  });

  it("海报未就绪：不挂 poster 属性（preload metadata 自渲首帧）", () => {
    renderVideo("Y:\\照片\\C0121.MP4", null);
    expect(screen.getByTestId("viewer-video")).not.toHaveAttribute("poster");
  });

  it("onError → 回退卡：HEVC 缺失文案 + 系统播放按钮调用 open_with_system", async () => {
    hevcMock.mockReturnValue(false);
    renderVideo("Y:\\照片\\HEVC.MP4");
    fireEvent.error(screen.getByTestId("viewer-video"));

    const fallback = await screen.findByTestId("viewer-video-fallback");
    expect(fallback.textContent).toContain("HEVC");
    fireEvent.click(screen.getByTestId("viewer-video-system"));
    await waitFor(() => expect(openMock).toHaveBeenCalledWith("Y:\\照片\\HEVC.MP4"));
  });

  it("onError + HEVC 可用 → 通用失败文案（仍可系统播放）", async () => {
    hevcMock.mockReturnValue(true);
    renderVideo();
    fireEvent.error(screen.getByTestId("viewer-video"));
    const fallback = await screen.findByTestId("viewer-video-fallback");
    expect(fallback.textContent).not.toContain("HEVC (H.265)");
    expect(screen.getByTestId("viewer-video-system")).toBeInTheDocument();
  });

  it("路径切换：失败态复位回 <video>", async () => {
    const { rerender } = renderVideo();
    fireEvent.error(screen.getByTestId("viewer-video"));
    await screen.findByTestId("viewer-video-fallback");

    rerender(
      <I18nextProvider i18n={i18n}>
        <ViewerVideo path="Y:\\照片\\OTHER.MP4" posterUrl={null} />
      </I18nextProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("viewer-video")).toBeInTheDocument());
  });
});
