import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, within } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import GearPage from "./GearPage";
import { gearStats, type GearStats } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    gearStats: vi.fn(),
  };
});

const gearStatsMock = vi.mocked(gearStats);

// --- 工具 ---------------------------------------------------------------------------

function fullStats(overrides: Partial<GearStats> = {}): GearStats {
  return {
    cameras: [
      { name: "Canon EOS R5", count: 100 },
      { name: "Sony A7 IV", count: 40 },
    ],
    lenses: [
      { name: "RF 24-70mm F2.8", count: 80 },
      { name: "RF 50mm F1.8", count: 20 },
    ],
    focalBuckets: [
      { label: "24mm", min: 20, max: 35, count: 30 },
      { label: "50mm", min: 35, max: 70, count: 60 },
      { label: "85mm", min: 70, max: 105, count: 10 },
      { label: "135mm", min: 105, max: 180, count: 0 },
      { label: "200+mm", min: 200, max: null, count: 5 },
    ],
    isoBuckets: [
      { label: "ISO 100", count: 90 },
      { label: "ISO 400", count: 30 },
      { label: "ISO 3200+", count: 3 },
    ],
    apertureBuckets: [
      { label: "f/1.x", count: 12 },
      { label: "f/2.8", count: 44 },
    ],
    shutterBuckets: [
      { label: "1/250s", count: 55 },
      { label: "1/1000+", count: 8 },
    ],
    ...overrides,
  };
}

function renderGear() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gear"]}>
        <Routes>
          <Route path="/gear" element={<GearPage />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  gearStatsMock.mockReset().mockResolvedValue(null);
});

// --- 渲染 ---------------------------------------------------------------------------

describe("器材统计：渲染", () => {
  it("机身/镜头 TOP 榜 + 焦段/ISO/光圈/快门四分布柱图", async () => {
    gearStatsMock.mockResolvedValue(fullStats());
    renderGear();

    // TOP 榜：型号 + 计数行
    const cameras = await screen.findByTestId("gear-top-cameras");
    const rows = within(cameras).getAllByTestId("gear-top-row");
    expect(rows.map((r) => r.getAttribute("data-name"))).toEqual(["Canon EOS R5", "Sony A7 IV"]);
    expect(rows[0]).toHaveTextContent("100");
    expect(within(screen.getByTestId("gear-top-lenses")).getAllByTestId("gear-top-row")).toHaveLength(2);

    // 四分布图：柱带 label/count，最高柱 data-max
    for (const chart of ["focal", "iso", "aperture", "shutter"] as const) {
      const card = screen.getByTestId(`gear-chart-${chart}`);
      expect(within(card).getAllByTestId("gear-chart-bar").length).toBeGreaterThan(0);
    }
    const isoBars = within(screen.getByTestId("gear-chart-iso")).getAllByTestId("gear-chart-bar");
    expect(isoBars.map((b) => b.getAttribute("data-label"))).toEqual(["ISO 100", "ISO 400", "ISO 3200+"]);
    expect(isoBars.map((b) => b.getAttribute("data-count"))).toEqual(["90", "30", "3"]);
    expect(isoBars[0]).toHaveAttribute("data-max", "true");
    expect(isoBars[1]).not.toHaveAttribute("data-max");
    // 柱列 title 悬停提示数值
    expect(isoBars[0]).toHaveAttribute("title", "ISO 100 · 90");

    expect(screen.queryByTestId("gear-empty")).not.toBeInTheDocument();
    expect(gearStatsMock).toHaveBeenCalledTimes(1);
  });

  it("null（无数据/后端未就绪）→ 空态文案", async () => {
    gearStatsMock.mockResolvedValue(null);
    renderGear();

    const empty = await screen.findByTestId("gear-empty");
    expect(empty).toHaveTextContent("暂无器材数据");
    expect(screen.queryByTestId("gear-top-cameras")).not.toBeInTheDocument();
    expect(screen.queryByTestId("gear-chart-focal")).not.toBeInTheDocument();
  });

  it("数据全 0 也给空态", async () => {
    gearStatsMock.mockResolvedValue(
      fullStats({
        cameras: [{ name: "A", count: 0 }],
        lenses: [],
        focalBuckets: [
          { label: "24mm", min: 20, max: 35, count: 0 },
          { label: "50mm", min: 35, max: 70, count: 0 },
        ],
        isoBuckets: [{ label: "ISO 100", count: 0 }],
        apertureBuckets: [{ label: "f/1.x", count: 0 }],
        shutterBuckets: [{ label: "1s", count: 0 }],
      }),
    );
    renderGear();

    expect(await screen.findByTestId("gear-empty")).toBeInTheDocument();
    expect(screen.queryByTestId("gear-chart-iso")).not.toBeInTheDocument();
  });
});

// --- 比例计算 -----------------------------------------------------------------------

describe("器材统计：横条/柱高比例", () => {
  it("TOP 榜横条宽度 = count/最大值百分比；分布柱高同口径且最高柱 accent", async () => {
    gearStatsMock.mockResolvedValue(fullStats());
    renderGear();

    const rows = within(await screen.findByTestId("gear-top-cameras")).getAllByTestId("gear-top-row");
    const bars = rows.map((r) => within(r).getByTestId("gear-top-bar"));
    expect(bars[0].style.width).toBe("100%"); // 100/100
    expect(bars[1].style.width).toBe("40%"); // 40/100

    // 焦段分布：最高柱 50mm（60）→ 100%；24mm（30）→ 50%
    const focal = within(screen.getByTestId("gear-chart-focal")).getAllByTestId("gear-chart-bar");
    expect(focal).toHaveLength(5);
    expect(focal[0]).toHaveAttribute("data-count", "30");
    expect(focal[1]).toHaveAttribute("data-max", "true");
    const barEls = focal.map((b) => b.querySelector("div.rounded-t") as HTMLElement);
    expect(barEls[1].style.height).toBe("100%");
    expect(barEls[0].style.height).toBe("50%");
    // 0 计柱保底可见高度（3%），不带 max 标记
    expect(barEls[3].style.height).toBe("3%");
    expect(focal[3]).not.toHaveAttribute("data-max");
  });
});
