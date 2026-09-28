import type React from "react";
import { beforeAll, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import AssetGrid, { ASPECT_FALLBACK, justifyItems } from "./AssetGrid";
import { groupAssetsByDate, type AssetGroup } from "../lib/assetGroups";
import type { AssetDto } from "@/ipc/api";

/**
 * AssetGrid 布局双模式（M4.5 A4）：justify（统一行高按宽高比分配宽，4:3 兜底，
 * 按行虚拟化）与 square（等宽方格，默认兼容视图）。
 * jsdom 无布局：offsetWidth mock 1200 → justify 可用宽 1176（扣 px-3 两侧 24）。
 */

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", { configurable: true, get: () => 1200 });
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", { configurable: true, get: () => 800 });
});

function asset(id: number, aspect?: { w: number; h: number }): AssetDto {
  return {
    id,
    path: `Y:\\照片\\IMG_${id}.JPG`,
    name: `IMG_${id}.JPG`,
    kind: "photo",
    capturedAt: "2026-09-18T10:00:00",
    camera: null,
    sizeBytes: 1,
    width: aspect?.w ?? null,
    height: aspect?.h ?? null,
  };
}

function groupsOf(assets: AssetDto[]): AssetGroup[] {
  return groupAssetsByDate(assets);
}

function renderGrid(props: Partial<React.ComponentProps<typeof AssetGrid>> & { groups: AssetGroup[] }) {
  return render(
    <I18nextProvider i18n={i18n}>
      <AssetGrid {...props} />
    </I18nextProvider>,
  );
}

describe("虚拟行布局刷新", () => {
  it("语义结果替换和图大小变化后立即重算下一组位置，不需要折叠展开", () => {
    const first: AssetGroup[] = [
      { key: "a", date: "2026-09-18", assets: [asset(1)] },
      { key: "b", date: null, assets: [asset(2)] },
    ];
    const { rerender } = renderGrid({ groups: first, layout: "justify", tile: 120 });
    const next: AssetGroup[] = [
      { key: "a", date: "2026-09-18", assets: [asset(3)] },
      { key: "b", date: null, assets: [asset(4)] },
    ];
    rerender(<I18nextProvider i18n={i18n}><AssetGrid groups={next} layout="justify" tile={320} /></I18nextProvider>);
    const secondHeader = screen.getAllByTestId("gallery-group")[1].parentElement!;
    expect(secondHeader.style.transform).toBe("translateY(364px)");
    const tile = screen.getAllByTestId("gallery-tile")[0];
    expect(tile.style.height).toBe("320px");
    expect(tile.closest("[data-index]")!.getAttribute("style")).toContain("translateY(40px)");
  });
});

// --- justify 算法（纯函数） -----------------------------------------------------------

describe("justifyItems 贪心切行", () => {
  it("行高跌破目标即封行：封行高 = 可用宽/宽高比和", () => {
    // 4 张 4:3：3 张时行高 292 > 220 继续收；第 4 张 218.25 ≤ 220 封行
    const rows = justifyItems([asset(1), asset(2), asset(3), asset(4)], 1176, 220);
    expect(rows).toHaveLength(1);
    expect(rows[0].assets).toHaveLength(4);
    expect(rows[0].height).toBe(218); // round(1164 / (4×4/3))
    // 各项宽度 = aspect × 行高（取整）
    expect(rows[0].widths).toEqual([291, 291, 291, 291]);
  });

  it("组尾余量按目标行高左对齐（不足整行不拉伸）", () => {
    const rows = justifyItems([asset(1), asset(2)], 1176, 220);
    expect(rows).toHaveLength(1);
    expect(rows[0].height).toBe(220);
    expect(rows[0].widths).toEqual([293, 293]); // round(4/3 × 220)
  });

  it("横竖混排：宽度按各自宽高比分配（竖图窄、横图宽）", () => {
    const portrait = { w: 2000, h: 3000 }; // 2:3
    const landscape = { w: 6000, h: 4000 }; // 3:2
    const rows = justifyItems(
      [asset(1, portrait), asset(2, landscape)],
      1176,
      220,
    );
    expect(rows).toHaveLength(1);
    expect(rows[0].height).toBe(220);
    expect(rows[0].widths[0]).toBe(Math.round((2000 / 3000) * 220)); // ≈147
    expect(rows[0].widths[1]).toBe(Math.round((6000 / 4000) * 220)); // =330
    expect(rows[0].widths[0]).toBeLessThan(rows[0].widths[1]);
  });

  it("无尺寸资产按 4:3 兜底", () => {
    const rows = justifyItems([asset(1)], 1176, 220);
    expect(rows[0].widths[0]).toBe(Math.round(ASPECT_FALLBACK * 220));
  });

  it("多行切分：超出整行的资产落到下一行", () => {
    const items = Array.from({ length: 8 }, (_, i) => asset(i + 1));
    const rows = justifyItems(items, 1176, 220);
    // 4 张一行（见上）：8 张 = 两行
    expect(rows).toHaveLength(2);
    for (const row of rows) expect(row.assets).toHaveLength(4);
  });
});

// --- 渲染（jsdom 宽度 mock 1200） ------------------------------------------------------

describe("AssetGrid justify 渲染", () => {
  it("tile 宽 = 宽高比×行高、高 = 行高；data-layout=justify", () => {
    renderGrid({ groups: groupsOf([asset(1, { w: 6000, h: 4000 })]), layout: "justify", tile: 220 });

    const scroll = screen.getByTestId("gallery-grid-scroll");
    expect(scroll).toHaveAttribute("data-layout", "justify");
    const tile = screen.getByTestId("gallery-tile");
    expect(tile.style.width).toBe("330px"); // 3/2 × 220
    expect(tile.style.height).toBe("220px");
  });

  it("无尺寸资产 4:3 兜底渲染", () => {
    renderGrid({ groups: groupsOf([asset(1)]), layout: "justify", tile: 220 });

    const tile = screen.getByTestId("gallery-tile");
    expect(tile.style.width).toBe("293px"); // 4/3 × 220
    expect(tile.style.height).toBe("220px");
  });

  it("square（默认）保持等宽方格不受影响", () => {
    renderGrid({ groups: groupsOf([asset(1)]), tile: 200 });

    const scroll = screen.getByTestId("gallery-grid-scroll");
    expect(scroll).toHaveAttribute("data-layout", "square");
    const tile = screen.getByTestId("gallery-tile");
    expect(tile.style.width).toBe("200px");
    expect(tile.style.height).toBe("200px");
  });
});

// --- 组头折叠（M4.5 wave-3） -----------------------------------------------------------

describe("AssetGrid：组头折叠", () => {
  it("点击组头折叠：组内瓦片不渲染、头行变矮；再点展开恢复", async () => {
    const userEvent = (await import("@testing-library/user-event")).default;
    const user = userEvent.setup();
    renderGrid({ groups: groupsOf([asset(1), asset(2)]) });

    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(2);
    const header = screen.getByTestId("gallery-group");
    expect(header).toHaveAttribute("data-collapsed", "false");

    await user.click(header);

    expect(screen.getByTestId("gallery-group")).toHaveAttribute("data-collapsed", "true");
    expect(screen.queryByTestId("gallery-tile")).not.toBeInTheDocument();
    expect(screen.getByTestId("gallery-group")).toHaveTextContent("2 张");

    await user.click(screen.getByTestId("gallery-group"));
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(2);
  });

  it("折叠互不影响：A 折 B 展", async () => {
    const userEvent = (await import("@testing-library/user-event")).default;
    const user = userEvent.setup();
    const a = asset(1);
    const b = { ...asset(2), capturedAt: "2026-09-01T10:00:00" };
    renderGrid({ groups: groupsOf([a, b]) });

    await user.click(screen.getAllByTestId("gallery-group")[0]);

    const after = screen.getAllByTestId("gallery-group");
    expect(after[0]).toHaveAttribute("data-collapsed", "true");
    expect(after[1]).toHaveAttribute("data-collapsed", "false");
    expect(screen.getAllByTestId("gallery-tile")).toHaveLength(1);
  });
});

// --- 多选（M4.5 wave-3） -----------------------------------------------------------------

describe("AssetGrid：多选交互", () => {
  it("selection.active 时点击瓦片=切换选中（不触发 onOpenAsset）；序号角标随选中序", async () => {
    const onOpenAsset = vi.fn();
    const onToggle = vi.fn((a: AssetDto) => void a);
    const userEvent = (await import("@testing-library/user-event")).default;
    const user = userEvent.setup();
    renderGrid({
      groups: groupsOf([asset(1), asset(2)]),
      onOpenAsset,
      onCheckClick: onToggle,
      selection: { active: true, selected: [2], onToggle },
    });

    const tiles = screen.getAllByTestId("gallery-tile");
    // 初始选中 id2：data-selected + 实心勾圆钮（③：check 圆钮替代序号角标）
    const selectedTile = tiles.find((t) => t.getAttribute("data-asset-id") === "2");
    expect(selectedTile).toHaveAttribute("data-selected", "true");
    const checkOf = (id: string) =>
      tiles.find((t) => t.getAttribute("data-asset-id") === id)?.querySelector(
        '[data-testid="tile-check"]',
      ) as HTMLElement;
    expect(checkOf("2")).toHaveAttribute("data-selected", "true");
    expect(checkOf("1")).toHaveAttribute("data-selected", "false");
    expect(tiles.find((t) => t.getAttribute("data-asset-id") === "1")).toHaveAttribute("data-selected", "false");

    await user.click(tiles.find((t) => t.getAttribute("data-asset-id") === "1") as HTMLElement);
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onToggle).toHaveBeenCalledWith(expect.objectContaining({ id: 1 }));
    expect(onOpenAsset).not.toHaveBeenCalled();
  });

  it("按住鼠标拖过多张照片会连续选择，随后 click 不会重复切换", () => {
    const onToggle = vi.fn();
    renderGrid({
      groups: groupsOf([asset(1), asset(2)]),
      selection: { active: true, selected: [], onToggle },
    });

    const tiles = screen.getAllByTestId("gallery-tile");
    fireEvent.pointerDown(tiles[0], { button: 0, pointerType: "mouse" });
    fireEvent.pointerEnter(tiles[1], { pointerType: "mouse" });
    fireEvent.pointerUp(tiles[1], { pointerType: "mouse" });
    fireEvent.click(tiles[1]);

    expect(onToggle.mock.calls.map(([a]) => a.id)).toEqual([1, 2]);
  });

  it("从已选照片开始拖动时统一取消经过的照片", () => {
    const onToggle = vi.fn();
    renderGrid({
      groups: groupsOf([asset(1), asset(2)]),
      selection: { active: true, selected: [1, 2], onToggle },
    });

    const tiles = screen.getAllByTestId("gallery-tile");
    fireEvent.pointerDown(tiles[0], { button: 0, pointerType: "mouse" });
    fireEvent.pointerEnter(tiles[1], { pointerType: "mouse" });
    expect(onToggle.mock.calls.map(([a]) => a.id)).toEqual([1, 2]);
  });

  it("check 圆钮点击 stopPropagation：只进多选不开查看器；默认 hover 显示、多选态常显", async () => {
    const onOpenAsset = vi.fn();
    const onCheckClick = vi.fn();
    const userEvent = (await import("@testing-library/user-event")).default;
    const user = userEvent.setup();
    const first = renderGrid({
      groups: groupsOf([asset(1), asset(2)]),
      onOpenAsset,
      onCheckClick,
    });

    // 非多选态：圆钮渲染（CSS hover 控制显隐，DOM 恒在且 opacity-0）
    const checks = screen.getAllByTestId("tile-check");
    expect(checks).toHaveLength(2);
    expect(checks[0].className).toContain("opacity-0");
    expect(checks[0].className).toContain("group-hover:opacity-100");

    await user.click(checks[0]);
    expect(onCheckClick).toHaveBeenCalledTimes(1);
    expect(onCheckClick).toHaveBeenCalledWith(expect.objectContaining({ id: 1 }));
    expect(onOpenAsset).not.toHaveBeenCalled(); // 不触发瓦片本身（不开查看器）
    first.unmount();

    // 多选态：圆钮常显（opacity-100），选中=实心勾
    renderGrid({
      groups: groupsOf([asset(1), asset(2)]),
      onOpenAsset,
      onCheckClick,
      selection: { active: true, selected: [1], onToggle: vi.fn() },
    });
    const checkAfter = screen
      .getAllByTestId("gallery-tile")
      .find((t) => t.getAttribute("data-asset-id") === "1")
      ?.querySelector('[data-testid="tile-check"]') as HTMLElement;
    expect(checkAfter.className).toContain("opacity-100");
    expect(checkAfter).toHaveAttribute("data-selected", "true");
  });

  it("Ctrl+点击（未开多选）→ onCtrlClick；普通点击仍开查看器", async () => {
    const onOpenAsset = vi.fn();
    const onCtrlClick = vi.fn();
    const userEvent = (await import("@testing-library/user-event")).default;
    const user = userEvent.setup();
    renderGrid({ groups: groupsOf([asset(1), asset(2)]), onOpenAsset, onCtrlClick });

    fireEvent.click(screen.getAllByTestId("gallery-tile")[0], { ctrlKey: true });
    expect(onCtrlClick).toHaveBeenCalledTimes(1);

    await user.click(screen.getAllByTestId("gallery-tile")[1]);
    expect(onOpenAsset).toHaveBeenCalledTimes(1);
  });
});

// --- 选片补全（B1）：色点角标 / 拒绝弱化 / 只读态 -----------------------------------------

describe("AssetGrid：颜色标签与拒绝旗标瓦片呈现", () => {
  it("有 colorLabel：瓦片左下角色点（data-label）；无则不渲染", () => {
    const labeled = { ...asset(1), colorLabel: "blue" };
    renderGrid({ groups: groupsOf([labeled, asset(2)]) });

    const dot = screen.getByTestId("tile-color-dot");
    expect(dot).toHaveAttribute("data-label", "blue");
    // 只给有标记的资产渲染
    const tile2 = screen.getAllByTestId("gallery-tile").find((t) => t.getAttribute("data-asset-id") === "2");
    expect(tile2?.querySelector('[data-testid="tile-color-dot"]')).toBeNull();
  });

  it("rejected 资产：瓦片弱化（opacity-50）+ 红旗角标 + data-rejected", () => {
    const rejected = { ...asset(1), rejected: true };
    renderGrid({ groups: groupsOf([rejected]) });

    const tile = screen.getByTestId("gallery-tile");
    expect(tile).toHaveAttribute("data-rejected", "true");
    expect(tile.className).toContain("opacity-50");
    expect(screen.getByTestId("tile-reject-badge")).toBeInTheDocument();
  });

  it("正常资产：无弱化与角标（data-rejected 不出现）", () => {
    renderGrid({ groups: groupsOf([asset(1)]) });
    expect(screen.getByTestId("gallery-tile").hasAttribute("data-rejected")).toBe(false);
    expect(screen.queryByTestId("tile-reject-badge")).not.toBeInTheDocument();
  });

  it("readOnly（回收站只读态）：不渲染收藏星钮，多选仍可用", () => {
    const onCheckClick = vi.fn();
    renderGrid({
      groups: groupsOf([asset(1)]),
      readOnly: true,
      selection: { active: true, selected: [], onToggle: () => {} },
      onCheckClick,
    });

    expect(screen.queryByTestId("tile-favorite")).not.toBeInTheDocument();
    expect(screen.getByTestId("tile-check")).toBeInTheDocument();
  });
});
