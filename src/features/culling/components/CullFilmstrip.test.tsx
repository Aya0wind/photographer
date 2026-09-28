import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import CullFilmstrip, { filmstripWindow } from "./CullFilmstrip";
import { resetThumbPipelineForTests } from "@/features/gallery/lib/thumbPipeline";

/** 决定表辅助 */
function decisionsOf(entries: Array<[number, "accepted" | "rejected"]>): Map<number, "accepted" | "rejected" | null> {
  return new Map(entries);
}

function renderStrip(
  count: number,
  decisions: Map<number, "accepted" | "rejected" | null> = new Map(),
  index = 0,
) {
  const ids = Array.from({ length: count }, (_, i) => i + 1);
  const onJump = vi.fn();
  const utils = render(
    <I18nextProvider i18n={i18n}>
      <CullFilmstrip
        ids={ids}
        decisions={decisions}
        origins={new Map()}
        assetMeta={new Map()}
        index={index}
        onJump={onJump}
      />
    </I18nextProvider>,
  );
  return { onJump, ...utils };
}

beforeEach(() => {
  resetThumbPipelineForTests();
});

describe("胶片条窗口推导（filmstripWindow）", () => {
  it("scrollLeft 推导可见窗口 ±overscan；边界裁剪", () => {
    // STEP=74（68+6）：窗口从 floor(scrollLeft/74)-10 起
    expect(filmstripWindow(5000, 0, 0)).toEqual({ start: 0, end: 11 });
    expect(filmstripWindow(5000, 74 * 100, 74 * 20)).toEqual({
      start: 90,
      end: 130,
    });
    // 尾部裁剪到 count
    expect(filmstripWindow(5, 0, 0)).toEqual({ start: 0, end: 5 });
    expect(filmstripWindow(0, 500, 300)).toEqual({ start: 0, end: 0 });
  });
});

describe("胶片条虚拟化冒烟（CullFilmstrip）", () => {
  it("万格只渲染窗口内少量 DOM（虚拟滚动），点击跳转回传索引", async () => {
    const { onJump } = renderStrip(5000);

    const strip = screen.getByTestId("cull-filmstrip");
    expect(strip).toHaveAttribute("data-count", "5000");
    const cells = screen.getAllByTestId("cull-film-cell");
    expect(cells.length).toBeLessThan(30); // 视口(0)+overscan(10×2) 量级
    expect(cells.length).toBeGreaterThan(0);

    await userEvent.click(cells[3]);
    expect(onJump).toHaveBeenCalledWith(3);
  });

  it("滚动更新窗口（scrollLeft 驱动重渲），决定角标随决定表渲染", () => {
    renderStrip(
      600,
      decisionsOf([
        [91, "accepted"],
        [92, "rejected"],
      ]),
      90,
    );
    // 初始窗口含 91/92（index=90 自动居中会尝试 scrollTo，jsdom 走 scrollLeft 分支）
    expect(screen.getAllByTestId("cull-film-cell").length).toBeGreaterThan(0);

    // 手动滚到 300 格处，窗口应移动（新格子出现、旧的卸载）
    const scroller = screen.getByTestId("cull-filmstrip").querySelector(".sp-scroll") as HTMLElement;
    expect(scroller).not.toBeNull();
    scroller.scrollLeft = 74 * 300;
    fireEvent.scroll(scroller);
    const window1 = screen.getByTestId("cull-film-window").getAttribute("data-window");
    expect(window1).not.toBe("0:11");
    const cellsAfter = screen.getAllByTestId("cull-film-cell");
    expect(cellsAfter.length).toBeLessThan(30);
    // 窗口起点应在 300 附近（±overscan）
    const start = Number(window1?.split(":")[0]);
    expect(Math.abs(start - 290)).toBeLessThanOrEqual(2);
  });

  it("当前格高亮（data-current）与绿✓/红✗ 角标", () => {
    renderStrip(
      20,
      decisionsOf([
        [5, "accepted"],
        [6, "rejected"],
      ]),
      4,
    );
    const current = screen.getAllByTestId("cull-film-cell").find((c) => c.getAttribute("data-current") === "true");
    expect(current).toBeDefined();
    expect(current?.getAttribute("data-asset-id")).toBe("5");
    expect(current?.getAttribute("data-decision")).toBe("accepted");
    expect(screen.getAllByTestId("cull-film-accepted").length).toBe(1);
    expect(screen.getAllByTestId("cull-film-rejected").length).toBe(1);
  });
});
