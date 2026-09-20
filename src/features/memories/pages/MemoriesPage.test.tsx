import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import MemoriesPage from "./MemoriesPage";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";
import { resetViewMarkForTests } from "@/features/gallery/lib/viewMark";
import { assetThumbGet, assetViewMark, onThisDay, type AssetDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    onThisDay: vi.fn(),
    assetViewMark: vi.fn(),
    assetThumbGet: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn().mockResolvedValue(undefined),
  convertFileSrc: vi.fn(),
}));

const onThisDayMock = vi.mocked(onThisDay);
const markMock = vi.mocked(assetViewMark);
const thumbMock = vi.mocked(assetThumbGet);

// --- 工具 ---------------------------------------------------------------------------

function makeAsset(id: number, year: number): AssetDto {
  const file = `IMG_${String(id).padStart(4, "0")}.JPG`;
  return {
    id,
    path: `Y:\\照片\\SmartPhoto\\${year}\\${file}`,
    name: file,
    kind: "photo",
    capturedAt: `${year}-09-19T10:00:00`,
    camera: "Canon EOS R5",
    sizeBytes: 1024,
  };
}

function renderMemories() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/memories"]}>
        <Routes>
          <Route path="/memories" element={<MemoriesPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

beforeEach(() => {
  onThisDayMock.mockReset().mockResolvedValue([]);
  markMock.mockReset().mockResolvedValue(undefined);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
  resetViewMarkForTests();
});

// --- 年份分块 -----------------------------------------------------------------------

describe("那年今天：年份分块渲染", () => {
  it("按年份分块降序；今年块置顶标「今天」，往年块头「N 年前」；块内横滚卡", async () => {
    const thisYear = new Date().getFullYear();
    onThisDayMock.mockResolvedValue([
      makeAsset(1, thisYear - 6),
      makeAsset(2, thisYear - 3),
      makeAsset(3, thisYear - 3),
      makeAsset(4, thisYear),
    ]);

    renderMemories();

    const blocks = await screen.findAllByTestId("memories-block");
    expect(blocks.map((b) => b.getAttribute("data-year"))).toEqual([
      String(thisYear),
      String(thisYear - 3),
      String(thisYear - 6),
    ]);

    // 今年块：标题「今天」+ 徽标；块头计数 1 张
    expect(blocks[0]).toHaveTextContent("今天");
    expect(within(blocks[0]).getByTestId("memories-today-badge")).toBeInTheDocument();

    // 往年块：N 年前 + 该年张数；无「今天」徽标
    expect(blocks[1]).toHaveTextContent("3 年前");
    expect(blocks[1]).toHaveTextContent("2 张");
    expect(blocks[2]).toHaveTextContent("6 年前");
    expect(within(blocks[1]).queryByTestId("memories-today-badge")).not.toBeInTheDocument();

    // 卡片共 4 张，块内分布 1/2/1
    expect(blocks.map((b) => within(b).getAllByTestId("memories-card").length)).toEqual([1, 2, 1]);
    expect(onThisDayMock).toHaveBeenCalledTimes(1);
  });

  it("今天无历史（[] / 命令失败回退）→ 空态文案", async () => {
    onThisDayMock.mockResolvedValue([]);
    renderMemories();

    const empty = await screen.findByTestId("memories-empty");
    expect(empty).toHaveTextContent("今天还没有历史记录");
    expect(empty).toHaveTextContent("往年今天拍摄的照片会出现在这里");
    expect(screen.queryByTestId("memories-block")).not.toBeInTheDocument();
  });

  it("点击照片进查看器定位该张；组内导航/胶片条不跨年份块", async () => {
    const thisYear = new Date().getFullYear();
    onThisDayMock.mockResolvedValue([
      makeAsset(10, thisYear - 3),
      makeAsset(11, thisYear - 3),
      makeAsset(12, thisYear - 8),
    ]);
    const user = userEvent.setup();
    renderMemories();

    const blocks = await screen.findAllByTestId("memories-block");
    const card = within(blocks[0]).getAllByTestId("memories-card")[0]; // 3 年前块第一张
    await user.click(card);

    expect(await screen.findByTestId("viewer")).toBeInTheDocument();
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0010.JPG");
    await waitFor(() => expect(markMock).toHaveBeenCalledWith(10));

    // 胶片条只含同年块（2 张），不跨到 2026 年块
    expect(screen.getAllByTestId("viewer-filmthumb")).toHaveLength(2);

    // 组内下一张 → 同年第二张
    await user.click(screen.getByTestId("viewer-next"));
    expect(screen.getByTestId("viewer-name")).toHaveTextContent("IMG_0011.JPG");

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByTestId("viewer")).not.toBeInTheDocument());
    expect(screen.getByTestId("memories-page")).toBeInTheDocument();
  });
});
