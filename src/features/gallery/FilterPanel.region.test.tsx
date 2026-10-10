import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import { mapRegionTree, type RegionCacheRow } from "@/ipc/api/map";
import {
  FilterPanel,
  buildChips,
  buildFilters,
  hasActiveFilters,
  parseInputs,
  serializeInputs,
  EMPTY_INPUTS,
  type SearchInputs,
} from "./FilterPanel";

/**
 * 筛选面板拍摄位置维度（三级级联，2026-10-09 拍摄地图联动）：
 * inputs.region（id+path 随行）→ 序列化键/防抖链 → buildFilters 映射
 * filters.regionId（单选，含子树）；chips 完整路径展示可单独移除；
 * 级联选项来自 mapRegionTree（国家 → 省 → 市县，上一级选定才出下一级）。
 */

vi.mock("@/ipc/api/map", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api/map")>();
  return {
    ...actual,
    mapRegionTree: vi.fn(),
  };
});

const treeMock = vi.mocked(mapRegionTree);

/** 地区树夹具：中国/日本 → 浙江省/广东省 → 杭州市/宁波市 → 西湖区 */
function treeFixture(): RegionCacheRow[] {
  return [
    { id: 1, parent: null, level: 0, name: "中国", code: "CN", lat: 35, lon: 104 },
    { id: 2, parent: null, level: 0, name: "日本", code: "JP", lat: 36, lon: 138 },
    { id: 11, parent: 1, level: 1, name: "浙江省", code: "33", lat: 30, lon: 120 },
    { id: 12, parent: 1, level: 1, name: "广东省", code: "44", lat: 23, lon: 113 },
    { id: 111, parent: 11, level: 2, name: "杭州市", code: "3301", lat: 30.2, lon: 120.1 },
    { id: 112, parent: 11, level: 2, name: "宁波市", code: "3302", lat: 29.8, lon: 121.5 },
    { id: 1111, parent: 111, level: 3, name: "西湖区", code: "330106", lat: 30.25, lon: 120.13 },
  ];
}

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
  treeMock.mockResolvedValue(treeFixture());
});

// --- 序列化与 filters 映射 ------------------------------------------------------------

describe("region 序列化与 buildFilters", () => {
  it("未选不携带（regionId 缺省 = 不过滤）", () => {
    expect(buildFilters(EMPTY_INPUTS).regionId).toBeUndefined();
    expect(hasActiveFilters(EMPTY_INPUTS)).toBe(false);
  });

  it("选中 → filters.regionId（单选）", () => {
    const inputs: SearchInputs = {
      ...EMPTY_INPUTS,
      region: { id: 111, path: "中国 / 浙江省 / 杭州市" },
    };
    expect(buildFilters(inputs).regionId).toBe(111);
    expect(hasActiveFilters(inputs)).toBe(true);
  });

  it("serialize → parse 往返保留 region（防抖键重建条件）", () => {
    const inputs: SearchInputs = {
      ...EMPTY_INPUTS,
      region: { id: 1111, path: "中国 / 浙江省 / 杭州市 / 西湖区" },
    };
    const roundtrip = parseInputs(serializeInputs(inputs));
    expect(roundtrip.region).toEqual({ id: 1111, path: "中国 / 浙江省 / 杭州市 / 西湖区" });
    expect(buildFilters(roundtrip).regionId).toBe(1111);
  });

  it("chips：完整路径展示（模板插值），patch 移除该条件", () => {
    const inputs: SearchInputs = {
      ...EMPTY_INPUTS,
      region: { id: 111, path: "中国 / 浙江省 / 杭州市" },
    };
    const chips = buildChips(inputs, (key) =>
      key === "search.regionChip" ? "{{path}}" : key,
    );
    const chip = chips.find((c) => c.key === "region");
    expect(chip?.label).toBe("中国 / 浙江省 / 杭州市");
    expect(chip?.patch.region).toBeNull();
    expect(hasActiveFilters(chip!.patch)).toBe(false);
  });
});

// --- 级联选择 UI ----------------------------------------------------------------------

/** 下拉禁用态挂在最外层 wrapper（relative + pointer-events-none）；从按钮上溯两层取 */
function dropdownWrapperOf(testId: string): HTMLElement {
  const wrapper = screen.getByTestId(testId).parentElement?.parentElement;
  expect(wrapper).toBeDefined();
  return wrapper!;
}

describe("筛选面板：位置三级级联", () => {
  it("国家下拉渲染 mapRegionTree 选项；未选国家时省/市县禁用", async () => {
    const user = userEvent.setup();
    renderPanel();
    await waitFor(() => expect(treeMock).toHaveBeenCalled());

    await user.click(screen.getByTestId("search-region-country-button"));
    const menu = await screen.findByTestId("search-region-country-menu");
    const options = within(menu).getAllByTestId("search-region-country-option");
    expect(options.map((o) => o.textContent)).toEqual(["中国", "日本"]);

    // 省级/市县在选定上一级前禁用（wrapper pointer-events-none + opacity）
    expect(dropdownWrapperOf("search-region-province-button")).toHaveClass("pointer-events-none");
    expect(dropdownWrapperOf("search-region-city-button")).toHaveClass("pointer-events-none");
  });

  it("逐级下钻：国家 → 省 → 市/县，选定即生效（path 全路径随行）", async () => {
    const user = userEvent.setup();
    // 受控宿主：面板是受控组件，级联下钻需把 onPatch 落回 state 才能逐级验证
    const applied: Array<Partial<SearchInputs>> = [];
    function Host() {
      const [inputs, setInputs] = useState<SearchInputs>(EMPTY_INPUTS);
      return (
        <FilterPanel
          inputs={inputs}
          onPatch={(patch) => {
            applied.push(patch);
            setInputs((prev) => ({ ...prev, ...patch }));
          }}
        />
      );
    }
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <Host />
        </MemoryRouter>
      </I18nextProvider>,
    );
    await waitFor(() => expect(treeMock).toHaveBeenCalled());

    // 国家：中国（选后省级解锁）
    await user.click(screen.getByTestId("search-region-country-button"));
    await user.click(
      within(await screen.findByTestId("search-region-country-menu")).getAllByTestId(
        "search-region-country-option",
      )[0],
    );
    expect(applied[applied.length - 1]).toEqual({ region: { id: 1, path: "中国" } });
    expect(screen.getByTestId("search-region-country-button")).toHaveTextContent("中国");
    expect(dropdownWrapperOf("search-region-province-button")).not.toHaveClass("pointer-events-none");

    // 省：浙江省（受国家 parent 限定；日本省份不出现）
    await user.click(screen.getByTestId("search-region-province-button"));
    const provinceMenu = await screen.findByTestId("search-region-province-menu");
    const provinces = within(provinceMenu).getAllByTestId("search-region-province-option");
    expect(provinces.map((o) => o.textContent)).toEqual(["浙江省", "广东省"]);
    await user.click(provinces[0]);
    expect(applied[applied.length - 1]).toEqual({ region: { id: 11, path: "中国 / 浙江省" } });
    expect(dropdownWrapperOf("search-region-city-button")).not.toHaveClass("pointer-events-none");

    // 市县：省直属市 + 各市直属县（缩进项）；选县级带全路径、按钮回显名称
    await user.click(screen.getByTestId("search-region-city-button"));
    const cityMenu = await screen.findByTestId("search-region-city-menu");
    const cities = within(cityMenu).getAllByTestId("search-region-city-option");
    expect(cities.map((o) => o.textContent)).toEqual(["杭州市", "西湖区", "宁波市"]);
    await user.click(cities[1]);
    expect(applied[applied.length - 1]).toEqual({
      region: { id: 1111, path: "中国 / 浙江省 / 杭州市 / 西湖区" },
    });
    expect(screen.getByTestId("search-region-city-button")).toHaveTextContent("西湖区");
    expect(screen.getByTestId("search-region-province-button")).toHaveTextContent("浙江省");
  });

  it("「不限」回退到上一级；× 一键清空（清空 = 去筛选）", async () => {
    const user = userEvent.setup();
    const { onPatch } = renderPanel({
      ...EMPTY_INPUTS,
      region: { id: 1111, path: "中国 / 浙江省 / 杭州市 / 西湖区" },
    });
    await waitFor(() => expect(treeMock).toHaveBeenCalled());

    // 市县「不限」→ 回退到省级（region = 省节点）
    await user.click(screen.getByTestId("search-region-city-button"));
    await user.click(screen.getByTestId("search-region-city-all"));
    expect(onPatch).toHaveBeenLastCalledWith({
      region: { id: 11, path: "中国 / 浙江省" },
    });

    // × 清空按钮 → region: null（去筛选）
    await user.click(screen.getByTestId("search-region-clear"));
    expect(onPatch).toHaveBeenLastCalledWith({ region: null });
  });

  it("已选时各级按钮回显名称；树缓存未就绪 → 下拉禁用仅回显路径", async () => {
    treeMock.mockResolvedValue(null);
    renderPanel({
      ...EMPTY_INPUTS,
      region: { id: 111, path: "中国 / 浙江省 / 杭州市" },
    });
    await waitFor(() => expect(treeMock).toHaveBeenCalled());

    // 国家按钮禁用 + 路径回显 span（title = 完整路径）
    expect(dropdownWrapperOf("search-region-country-button")).toHaveClass("pointer-events-none");
    const fallback = screen.getByTitle("中国 / 浙江省 / 杭州市");
    expect(fallback).toHaveTextContent("中国 / 浙江省 / 杭州市");
  });
});
