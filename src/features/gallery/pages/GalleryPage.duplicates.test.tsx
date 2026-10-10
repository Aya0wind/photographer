import { assetFixture } from "@/test/fixtures";
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
  duplicatesList,
  photoLibraryList,
  type AssetDto,
  type DuplicateGroupDto,
} from "@/ipc/api";

/**
 * 画廊「隐藏跨库重复」（M5 §四，纯显示过滤）：
 * - 开关在浮动工具条（aria-pressed），开启才拉 duplicates_list(exact)
 * - 跨库同哈希组只显示一张代表（在线 > RAW > 评分 > 导入早），瓦片 ×N 徽标
 * - 徽标点击 → 重复项列表（组内全部副本，含所属库名/缺失标记）
 * - 操作只作用于可见资产（隐藏副本不渲染瓦片）；开关关闭恢复全部
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    assetsPage: vi.fn(),
    assetsSeek: vi.fn(async () => []),
    assetVersions: vi.fn(async () => ({ groupId: null, members: [] })),
    assetGroupDates: vi.fn(),
    assetThumbGet: vi.fn(),
    assetsCount: vi.fn(async () => 3),
    isIpcAvailable: vi.fn(() => true),
    duplicatesList: vi.fn(),
    photoLibraryList: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
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
const duplicatesMock = vi.mocked(duplicatesList);
const listLibrariesMock = vi.mocked(photoLibraryList);

function asset(id: number, libraryId: string, overrides: Partial<AssetDto> = {}): AssetDto {
  return assetFixture(id, {
    capturedAt: "2026-09-18T10:00:00",
    name: `IMG_${id}.JPG`,
    libraryId,
    ...overrides,
  });
}

function renderGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/gallery" element={<GalleryPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeAll(() => {
  // jsdom 无布局：虚拟列表需要非零视口
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
  vi.mocked(convertFileSrc).mockReturnValue("");
  class IntersectionObserverStub {
    observe(): void {}
    unobserve(): void {}
    disconnect(): void {}
  }
  (window as unknown as Record<string, unknown>).IntersectionObserver = IntersectionObserverStub;
});

beforeEach(() => {
  localStorage.removeItem("photographer.gallery.hideCrossLibraryDuplicates");
  duplicatesMock.mockReset().mockResolvedValue([]);
  listLibrariesMock.mockReset().mockResolvedValue([]);
  assetsPageMock.mockReset();
  groupDatesMock.mockReset().mockResolvedValue([]);
  thumbMock.mockReset().mockResolvedValue({ status: "pending" });
  resetThumbPipelineForTests();
  clearGallerySnapshotForTests();
});

describe("画廊：隐藏跨库重复（M5 显示层折叠）", () => {
  it("默认关：两库同内容副本都显示；开启后只显示代表 + ×2 徽标；关闭恢复", async () => {
    // id 1（lib-a 普通 JPG）与 id 2（lib-b 普通 JPG）同内容；id 3 无重复
    const page = [asset(1, "lib-a"), asset(2, "lib-b"), asset(3, "lib-a")];
    assetsPageMock.mockResolvedValue(page);
    listLibrariesMock.mockResolvedValue([
      {
        id: "lib-a",
        name: "库 A",
        rootPath: "D:\\a",
        createdAt: "2026-10-09T00:00:00Z",
        status: "online",
        assetCount: 2,
        sizeBytes: 0,
      },
      {
        id: "lib-b",
        name: "库 B",
        rootPath: "E:\\b",
        createdAt: "2026-10-09T00:00:00Z",
        status: "online",
        assetCount: 1,
        sizeBytes: 0,
      },
    ]);
    duplicatesMock.mockResolvedValue([
      { kind: "exact", assets: [asset(1, "lib-a"), asset(2, "lib-b")] } satisfies DuplicateGroupDto,
    ]);
    const user = userEvent.setup();
    renderGallery();

    // 默认全显示
    expect(await screen.findAllByTestId("gallery-tile")).toHaveLength(3);

    // 开启开关：拉 exact 重复组 → 折叠为 2 张瓦片（1、3），代表 1 带徽标 ×2
    await user.click(screen.getByTestId("gallery-hide-duplicates"));
    await waitFor(() => expect(duplicatesMock).toHaveBeenCalledWith("exact", 0, 100));
    await waitFor(() =>
      expect(screen.getAllByTestId("gallery-tile").map((t) => t.getAttribute("data-asset-id"))).toEqual(["1", "3"]),
    );
    const badge = screen.getByTestId("gallery-duplicate-badge");
    expect(badge).toHaveAttribute("data-count", "2");

    // 重复项列表：组内全部副本（含所属库名）；关闭列表
    await user.click(badge);
    const dialog = await screen.findByTestId("duplicates-dialog");
    const members = within(dialog).getAllByTestId("duplicates-dialog-member");
    expect(members.map((m) => m.getAttribute("data-asset-id"))).toEqual(["1", "2"]);
    expect(within(members[0]).getByTestId("duplicates-member-library")).toHaveTextContent("库 A");
    await user.click(within(dialog).getByTestId("duplicates-dialog-close"));
    expect(screen.queryByTestId("duplicates-dialog")).not.toBeInTheDocument();

    // 关闭开关：副本回来、徽标消失、不再持有组数据
    await user.click(screen.getByTestId("gallery-hide-duplicates"));
    await waitFor(() =>
      expect(screen.getAllByTestId("gallery-tile").map((t) => t.getAttribute("data-asset-id"))).toEqual(["1", "2", "3"]),
    );
    expect(screen.queryByTestId("gallery-duplicate-badge")).not.toBeInTheDocument();
  });

  it("无重复组：开关可点但无徽标（弱化态）", async () => {
    assetsPageMock.mockResolvedValue([asset(1, "lib-a")]);
    duplicatesMock.mockResolvedValue([]);
    const user = userEvent.setup();
    renderGallery();

    await screen.findAllByTestId("gallery-tile");
    const toggle = screen.getByTestId("gallery-hide-duplicates");
    expect(toggle).toHaveAttribute("data-pressed", "false");
    expect(toggle).toHaveAttribute("data-has-duplicates", "false");

    await user.click(toggle);
    await waitFor(() => expect(duplicatesMock).toHaveBeenCalled());
    expect(screen.queryByTestId("gallery-duplicate-badge")).not.toBeInTheDocument();
  });
});
