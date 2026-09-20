import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";

import i18n from "@/i18n";
import MediaPage, { countsByKind } from "./MediaPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { assetThumbGet, assetsPage, formatList, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    formatList: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const assetsPageMock = vi.mocked(assetsPage);
const formatListMock = vi.mocked(formatList);
const thumbMock = vi.mocked(assetThumbGet);

/** 落点 URL 断言探针 */
function LocationProbe() {
  const location = useLocation();
  return (
    <div>
      <div data-testid="search-probe" />
      <span data-testid="loc">{`${location.pathname}${location.search}`}</span>
    </div>
  );
}

function makeAsset(id: number, kind: AssetDto["kind"]): AssetDto {
  const ext = kind === "raw" ? "NEF" : kind === "video" ? "MP4" : "JPG";
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.${ext}`,
    name: `IMG_${id}.${ext}`,
    kind,
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    sizeBytes: 1,
  };
}

function renderMedia() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/media"]}>
        <Routes>
          <Route path="/media" element={<MediaPage />} />
          <Route path="/search" element={<LocationProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  formatListMock.mockReset().mockResolvedValue([]);
  assetsPageMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
});

// --- 计数归并（纯函数） ---------------------------------------------------------------

describe("countsByKind：formatList 计数按大类归并", () => {
  it("NEF/ARW→RAW、JPG/HEIC→照片、MP4/MOV→视频、未知格式忽略", () => {
    const counts = countsByKind([
      { format: "JPG", count: 100 },
      { format: "HEIC", count: 20 },
      { format: "NEF", count: 30 },
      { format: "ARW", count: 5 },
      { format: "MP4", count: 4 },
      { format: "MOV", count: 1 },
      { format: "XYZ", count: 999 }, // 未知 → 不归并
    ]);
    expect(counts).toEqual({ photo: 120, raw: 35, video: 5 });
  });
});

// --- 页面渲染与交互 -------------------------------------------------------------------

describe("媒体类型页", () => {
  it("三大卡渲染：计数来自 formatList 归并；封面取该类首张（assetsPage limit=1）", async () => {
    formatListMock.mockResolvedValue([
      { format: "JPG", count: 100 },
      { format: "NEF", count: 30 },
      { format: "MP4", count: 4 },
    ]);
    assetsPageMock.mockImplementation(async (_afterId: number, _limit: number, filters?: { kinds?: string[] }) => {
      const kinds = filters?.kinds ?? [];
      if (kinds.includes("raw")) return [makeAsset(7, "raw")];
      if (kinds.includes("video")) return [makeAsset(8, "video")];
      return [makeAsset(6, "photo")];
    });
    renderMedia();

    const cards = await screen.findAllByTestId("media-card");
    expect(cards).toHaveLength(3);
    expect(cards.map((c) => c.getAttribute("data-kind"))).toEqual(["photo", "raw", "video"]);
    expect(cards[0]).toHaveAttribute("data-count", "100");
    expect(cards[1]).toHaveAttribute("data-count", "30");
    expect(cards[2]).toHaveAttribute("data-count", "4");
    expect(cards[0]).toHaveTextContent("照片");
    expect(cards[1]).toHaveTextContent("RAW");
    expect(cards[2]).toHaveTextContent("视频");

    // 封面：每类 assetsPage(0, 1, {kinds:[kind]})
    await waitFor(() => expect(assetsPageMock).toHaveBeenCalledWith(0, 1, { kinds: ["photo"] }));
    expect(assetsPageMock).toHaveBeenCalledWith(0, 1, { kinds: ["raw"] });
    expect(assetsPageMock).toHaveBeenCalledWith(0, 1, { kinds: ["video"] });
    expect(thumbMock).toHaveBeenCalledWith(7, 240);
  });

  it("点击卡 → /search?kind=<kind>（搜索页 URL 协议预置筛选）", async () => {
    formatListMock.mockResolvedValue([{ format: "NEF", count: 3 }]);
    const user = userEvent.setup();
    renderMedia();

    const cards = await screen.findAllByTestId("media-card");
    await user.click(cards[1]);

    expect(await screen.findByTestId("loc")).toHaveTextContent("/search?kind=raw");
  });

  it("该类无资产 → 渐变封面占位（不请求缩略图）", async () => {
    formatListMock.mockResolvedValue([{ format: "JPG", count: 2 }]);
    // 只有照片有资产；RAW/视频无 → 渐变占位
    assetsPageMock.mockImplementation(async (_afterId: number, _limit: number, filters?: { kinds?: string[] }) =>
      filters?.kinds?.includes("photo") ? [makeAsset(6, "photo")] : [],
    );
    renderMedia();

    await screen.findAllByTestId("media-card");
    await waitFor(() => expect(screen.getByTestId("media-cover-fallback-raw")).toBeInTheDocument());
    expect(screen.getByTestId("media-cover-fallback-video")).toBeInTheDocument();
    expect(screen.queryByTestId("media-cover-fallback-photo")).not.toBeInTheDocument();
    // 只有照片封面请求缩略图
    expect(thumbMock).toHaveBeenCalledWith(6, 240);
    expect(thumbMock.mock.calls.every(([id]) => id === 6)).toBe(true);
  });

  it("formatList 失败/空（后端未就绪）：计数 0 + 空态提示，三卡仍渲染可跳转", async () => {
    formatListMock.mockRejectedValue("backend down");
    const user = userEvent.setup();
    renderMedia();

    const cards = await screen.findAllByTestId("media-card");
    expect(cards).toHaveLength(3);
    for (const card of cards) expect(card).toHaveAttribute("data-count", "0");
    expect(await screen.findByTestId("media-empty")).toBeInTheDocument();

    await user.click(cards[2]);
    expect(await screen.findByTestId("loc")).toHaveTextContent("/search?kind=video");
  });
});
