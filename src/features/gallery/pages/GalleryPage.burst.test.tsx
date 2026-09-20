import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import GalleryPage from "./GalleryPage";
import { resetThumbPipelineForTests } from "../lib/thumbPipeline";
import { clearGallerySnapshotForTests } from "../lib/galleryCache";
import {
  assetGroupDates,
  assetThumbGet,
  assetsPage,
  isIpcAvailable,
  type AssetDto,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    isIpcAvailable: vi.fn(() => true),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

import { convertFileSrc } from "@tauri-apps/api/core";

const assetsPageMock = vi.mocked(assetsPage);
const groupDatesMock = vi.mocked(assetGroupDates);
const thumbMock = vi.mocked(assetThumbGet);
const ipcAvailableMock = vi.mocked(isIpcAvailable);
const convertMock = vi.mocked(convertFileSrc);

// --- 工具 -------------------------------------------------------------------------

function makeAsset(
  id: number,
  burst?: { burstId: number; burstCount?: number },
): AssetDto {
  const file = `IMG_${String(id).padStart(4, "0")}.JPG`;
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\2026\\${file}`,
    name: file,
    kind: "photo",
    capturedAt: "2026-09-18T10:00:00",
    camera: "Canon EOS R5",
    sizeBytes: 1024 * 1024,
    ...(burst ?? {}),
  };
}

function ImportProbe() {
  return <div data-testid="import-probe">IMPORT_PAGE</div>;
}

function renderGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/gallery" element={<GalleryPage />} />
          <Route path="/import" element={<ImportProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

// jsdom 无布局：TanStack Virtual 视口按元素宽高估算行（同 GalleryPage.test.tsx）
beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get: () => 1200,
  });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get: () => 800,
  });
  Object.defineProperty(HTMLElement.prototype, "clientHeight", {
    configurable: true,
    get: () => 800,
  });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", {
    configurable: true,
    get: () => 10000,
  });
});

class IntersectionObserverStub {
  static instances: IntersectionObserverStub[] = [];
  observed: Element[] = [];
  callback: IntersectionObserverCallback;
  constructor(callback: IntersectionObserverCallback) {
    this.callback = callback;
    IntersectionObserverStub.instances.push(this);
  }
  observe(el: Element): void {
    this.observed.push(el);
  }
  unobserve(): void {}
  disconnect(): void {
    this.observed = [];
  }
  triggerIntersecting(): void {
    for (const el of this.observed) {
      this.callback(
        [{ isIntersecting: true, target: el } as IntersectionObserverEntry],
        this as unknown as IntersectionObserver,
      );
    }
  }
}

beforeAll(() => {
  (window as unknown as Record<string, unknown>).IntersectionObserver = IntersectionObserverStub;
});

beforeEach(() => {
  assetsPageMock.mockReset().mockResolvedValue([]);
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  convertMock.mockReset().mockReturnValue("");
  ipcAvailableMock.mockReset().mockReturnValue(true);
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
  IntersectionObserverStub.instances = [];
});

// --- 连拍堆叠卡（M6） -----------------------------------------------------------------

describe("画廊：连拍堆叠卡", () => {
  it("连续同 burstId 折叠为一张堆叠卡：封面 + 底片层 + 「连拍 N」角标；组员瓦片不渲染，组头计数保持真实张数", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(11, { burstId: 9, burstCount: 3 }),
      makeAsset(10, { burstId: 9, burstCount: 3 }),
      makeAsset(9, { burstId: 9, burstCount: 3 }),
      makeAsset(8),
    ]);

    renderGallery();

    // 3 连拍折叠为封面 1 张 + 单张 1 张 = 2 块瓦片；组员（9/10）不在 DOM
    const tiles = await screen.findAllByTestId("gallery-tile");
    expect(tiles).toHaveLength(2);
    expect(screen.queryByRole("img", { name: "IMG_0009.JPG" })).not.toBeInTheDocument();
    expect(screen.queryByRole("img", { name: "IMG_0010.JPG" })).not.toBeInTheDocument();

    // 堆叠视觉：封面瓦片带底片层偏移 + 角标「连拍 3」（N=burstCount，含封面）
    const badge = screen.getByTestId("gallery-burst-badge");
    expect(badge).toHaveTextContent("连拍 3");
    const coverTile = badge.closest("[data-testid='gallery-tile']") as HTMLElement;
    expect(within(coverTile).getAllByTestId("gallery-burst-layer").length).toBeGreaterThanOrEqual(1);

    // 组头计数 = 真实 4 张（折叠不改口径）
    expect(screen.getByTestId("gallery-group")).toHaveTextContent("4 张");

    // 非连拍单张无角标
    const singleTile = tiles.find((t) => t !== coverTile) as HTMLElement;
    expect(within(singleTile).queryByTestId("gallery-burst-badge")).not.toBeInTheDocument();
    expect(within(singleTile).queryByTestId("gallery-burst-layer")).not.toBeInTheDocument();
  });

  it("点击堆叠卡打开查看器定位到封面；胶片条仍是完整组（天然顺序翻）", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(11, { burstId: 9, burstCount: 3 }),
      makeAsset(10, { burstId: 9, burstCount: 3 }),
      makeAsset(9, { burstId: 9, burstCount: 3 }),
      makeAsset(8),
    ]);
    const user = userEvent.setup();
    renderGallery();

    const badge = await screen.findByTestId("gallery-burst-badge");
    await user.click(badge.closest("[data-testid='gallery-tile']") as HTMLElement);

    // 查看器打开在封面（组内第一张 IMG_0011）
    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0011.JPG");

    // 胶片条 = 未折叠全量（4 张，含连拍组员）
    await waitFor(() => {
      expect(screen.getAllByTestId("viewer-filmthumb")).toHaveLength(4);
    });
  });

  it("单张不成组（即使携带 burstId）不显示角标与堆叠层", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(5, { burstId: 7, burstCount: 2 }),
      makeAsset(4),
    ]);

    renderGallery();

    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(2);
    expect(screen.queryByTestId("gallery-burst-badge")).not.toBeInTheDocument();
    expect(screen.queryByTestId("gallery-burst-layer")).not.toBeInTheDocument();
  });

  it("burstCount 缺失时角标回退连续段长度；跨组互不影响（组内折叠）", async () => {
    assetsPageMock.mockResolvedValue([
      makeAsset(21, { burstId: 5 }),
      makeAsset(20, { burstId: 5 }),
      makeAsset(19, { burstId: 5 }),
    ]);

    renderGallery();

    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(1);
    expect(screen.getByTestId("gallery-burst-badge")).toHaveTextContent("连拍 3");
    // 组头计数仍为 3
    expect(screen.getByTestId("gallery-group")).toHaveTextContent("3 张");
  });
});
