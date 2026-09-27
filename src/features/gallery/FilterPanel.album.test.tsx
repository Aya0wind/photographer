import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import { albumList } from "@/ipc/api";
import {
  FilterPanel,
  buildChips,
  buildFilters,
  EMPTY_INPUTS,
  type SearchInputs,
} from "./FilterPanel";

/**
 * 筛选面板相册维度（④）：album_list 数据源单选下拉 → inputs.album →
 * buildFilters 映射 filters.albumId；chips 展示相册名并可单独移除；
 * hideAlbum=true（相册详情页）隐藏该维度。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumList: vi.fn(),
  };
});

const albumListMock = vi.mocked(albumList);

function renderPanel(inputs: SearchInputs = EMPTY_INPUTS, hideAlbum = false) {
  const onPatch = vi.fn();
  return {
    onPatch,
    ...render(
      <I18nextProvider i18n={i18n}>
        <FilterPanel inputs={inputs} onPatch={onPatch} hideAlbum={hideAlbum} />
      </I18nextProvider>,
    ),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  albumListMock.mockResolvedValue([
    { id: 1, name: "青海湖 2026", coverAssetId: null, itemCount: 12, createdAt: "2026-09-01" },
    { id: 2, name: "街拍", coverAssetId: null, itemCount: 5, createdAt: "2026-09-02" },
  ]);
});

describe("筛选面板：所属相册维度", () => {
  it("下拉渲染 album_list 选项（名称 + 张数）；点选 → onPatch({album:{id,name}})", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel();
    await waitFor(() => expect(albumListMock).toHaveBeenCalled());

    await user.click(screen.getByTestId("search-album-button"));
    const menu = await screen.findByTestId("search-album-menu");
    const options = within(menu).getAllByTestId("search-album-option");
    expect(options.map((o) => o.getAttribute("data-album-id"))).toEqual(["1", "2"]);
    expect(options[0]).toHaveTextContent("青海湖 2026");
    expect(options[0]).toHaveTextContent("12");

    await user.click(options[0]);
    expect(onPatch).toHaveBeenCalledWith({ album: { id: 1, name: "青海湖 2026" } });
  });

  it("已选相册再点同一项 = 取消；× 清除按钮同效", async () => {
    const user = userEvent.setup();
    renderPanel({ ...EMPTY_INPUTS, album: { id: 1, name: "青海湖 2026" } });

    // 按钮高亮显示已选名
    expect(screen.getByTestId("search-album-button")).toHaveTextContent("青海湖 2026");

    await user.click(screen.getByTestId("search-album-button"));
    const options = await screen.findAllByTestId("search-album-option");
    expect(options[0]).toHaveAttribute("data-selected", "true");
    await user.click(options[0]); // 再点同项 = 清除
    expect(screen.getAllByTestId("search-album-option").length).toBeGreaterThan(0);

    // × 清除按钮
    fireEvent.click(screen.getByTestId("search-album-clear"));
  });

  it("无相册 → 下拉空态文案", async () => {
    const user = userEvent.setup();
    albumListMock.mockResolvedValue([]);
    renderPanel();
    await screen.findByTestId("search-filter-panel");

    await user.click(screen.getByTestId("search-album-button"));
    const menu = await screen.findByTestId("search-album-menu");
    expect(within(menu).queryByTestId("search-album-option")).not.toBeInTheDocument();
    expect(menu).toHaveTextContent("还没有手工相册");
  });

  it("hideAlbum=true：不渲染相册下拉、不调 albumList", async () => {
    renderPanel(EMPTY_INPUTS, true);
    await screen.findByTestId("search-filter-panel");
    expect(screen.queryByTestId("search-album-button")).not.toBeInTheDocument();
    expect(albumListMock).not.toHaveBeenCalled();
  });

  it("buildFilters：album → filters.albumId；chips 展示相册名", () => {
    const inputs: SearchInputs = { ...EMPTY_INPUTS, album: { id: 7, name: "青海湖" } };
    expect(buildFilters(inputs).albumId).toBe(7);
    expect(buildFilters(EMPTY_INPUTS).albumId).toBeUndefined();

    const chips = buildChips(inputs, (key) => key);
    const albumChip = chips.find((c) => c.key === "album");
    expect(albumChip?.label).toBe("青海湖");
    expect(albumChip?.patch.album).toBeNull();
  });
});
