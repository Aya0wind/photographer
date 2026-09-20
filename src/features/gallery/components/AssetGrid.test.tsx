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
      selection: { active: true, selected: [2], onToggle },
    });

    const tiles = screen.getAllByTestId("gallery-tile");
    // 初始选中 id2：data-selected + 角标 1
    const selectedTile = tiles.find((t) => t.getAttribute("data-asset-id") === "2");
    expect(selectedTile).toHaveAttribute("data-selected", "true");
    expect(selectedTile?.querySelector('[data-testid="gallery-tile-select-badge"]')).toHaveTextContent("1");
    expect(tiles.find((t) => t.getAttribute("data-asset-id") === "1")).toHaveAttribute("data-selected", "false");

    await user.click(tiles.find((t) => t.getAttribute("data-asset-id") === "1") as HTMLElement);
    expect(onToggle).toHaveBeenCalledTimes(1);
    expect(onToggle).toHaveBeenCalledWith(expect.objectContaining({ id: 1 }));
    expect(onOpenAsset).not.toHaveBeenCalled();
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
