import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import { photoLibraryList } from "@/ipc/api";
import {
  FilterPanel,
  buildChips,
  buildFilters,
  EMPTY_INPUTS,
  type SearchInputs,
} from "./FilterPanel";

/**
 * 筛选面板照片库维度（M5，2026-10-09 单库多照片库定案）：画廊默认全局跨库
 * 混排，按需收窄——photo_library_list 数据源多选下拉 → inputs.libraries →
 * buildFilters 映射 filters.libraryIds（OR）；chips 展示库名并可单独移除。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    photoLibraryList: vi.fn(),
  };
});

const listMock = vi.mocked(photoLibraryList);

function renderPanel(inputs: SearchInputs = EMPTY_INPUTS) {
  const onPatch = vi.fn();
  return {
    onPatch,
    ...render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <FilterPanel inputs={inputs} onPatch={onPatch} />
        </MemoryRouter>
      </I18nextProvider>,
    ),
  };
}

beforeEach(() => {
  vi.clearAllMocks();
  listMock.mockResolvedValue([
    {
      id: "lib-a",
      name: "主照片库",
      rootPath: "D:\\照片",
      createdAt: "2026-10-09T00:00:00Z",
      status: "online",
      assetCount: 120,
      sizeBytes: 1024,
    },
    {
      id: "lib-b",
      name: "移动硬盘库",
      rootPath: "E:\\备份",
      createdAt: "2026-10-09T00:00:00Z",
      status: "offline",
      assetCount: 8,
      sizeBytes: 2048,
    },
  ]);
});

describe("buildFilters：照片库多选 → filters.libraryIds", () => {
  it("未选不携带（默认全局跨库混排）", () => {
    expect(buildFilters(EMPTY_INPUTS).libraryIds).toBeUndefined();
  });

  it("多选 OR：按 id 收窄", () => {
    const inputs: SearchInputs = {
      ...EMPTY_INPUTS,
      libraries: [
        { id: "lib-a", name: "主照片库" },
        { id: "lib-b", name: "移动硬盘库" },
      ],
    };
    expect(buildFilters(inputs).libraryIds).toEqual(["lib-a", "lib-b"]);
  });
});

describe("筛选面板：照片库维度下拉", () => {
  it("下拉渲染 photo_library_list 选项（名称 + 照片数）；点选 → onPatch 带名称", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel();
    await waitFor(() => expect(listMock).toHaveBeenCalled());

    await user.click(screen.getByTestId("search-library-button"));
    const menu = await screen.findByTestId("search-library-menu");
    const options = within(menu).getAllByTestId("search-library-option");
    expect(options.map((o) => o.getAttribute("data-value"))).toEqual(["lib-a", "lib-b"]);
    expect(options[0]).toHaveTextContent("主照片库");
    expect(options[0]).toHaveTextContent("120");

    await user.click(options[0]);
    expect(onPatch).toHaveBeenCalledWith({
      libraries: [{ id: "lib-a", name: "主照片库" }],
    });
  });

  it("已选显示名称（id 不露脸）；再点同一项 = 取消勾选；× 清空", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel({
      ...EMPTY_INPUTS,
      libraries: [{ id: "lib-a", name: "主照片库" }],
    });

    const button = screen.getByTestId("search-library-button");
    expect(button).toHaveTextContent("主照片库");
    expect(button).not.toHaveTextContent("lib-a");

    // 多选下拉点选不收起（与相机/格式一致）：再点已选项 = 取消勾选
    await user.click(button);
    const menu = await screen.findByTestId("search-library-menu");
    const first = within(menu).getAllByTestId("search-library-option")[0];
    expect(first).toHaveAttribute("data-checked", "true");
    await user.click(first);
    expect(onPatch).toHaveBeenCalledWith({ libraries: [] });

    // × 清空按钮 → onPatch({libraries: []})
    await user.click(screen.getByTestId("search-library-clear"));
    expect(onPatch).toHaveBeenLastCalledWith({ libraries: [] });
  });

  it("chips：所选照片库各一枚（库名展示），patch 移除单项", () => {
    const inputs: SearchInputs = {
      ...EMPTY_INPUTS,
      libraries: [
        { id: "lib-a", name: "主照片库" },
        { id: "lib-b", name: "移动硬盘库" },
      ],
    };
    const chips = buildChips(inputs, (key) => key);
    const libraryChips = chips.filter((c) => c.key.startsWith("library:"));
    expect(libraryChips.map((c) => c.label)).toEqual(["主照片库", "移动硬盘库"]);
    expect(libraryChips[0].patch.libraries).toEqual([{ id: "lib-b", name: "移动硬盘库" }]);
  });

  it("空登记表：下拉显示空态文案（不报错）", async () => {
    listMock.mockResolvedValue([]);
    const user = userEvent.setup();
    renderPanel();
    await waitFor(() => expect(listMock).toHaveBeenCalled());

    await user.click(screen.getByTestId("search-library-button"));
    const menu = await screen.findByTestId("search-library-menu");
    expect(menu).toHaveTextContent("还没有照片库");
  });
});
