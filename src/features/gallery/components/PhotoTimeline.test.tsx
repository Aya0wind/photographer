import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import PhotoTimeline, { timelineMonths } from "./PhotoTimeline";

vi.mock("@/lib/motion", () => ({ useMotionOn: () => false }));

const dates = [
  { date: "2026-09-18", count: 4, coverAssetId: 40 },
  { date: "2026-09-01", count: 3, coverAssetId: 30 },
  { date: "2025-02-08", count: 5, coverAssetId: 20 },
  { date: "2024-01-01", count: 2, coverAssetId: 10 },
  { date: "unknown", count: 9, coverAssetId: 1 },
];

describe("photo timeline", () => {
  it("keeps all year positions fixed while hovering the marks", () => {
    render(<I18nextProvider i18n={i18n}><PhotoTimeline dates={dates} currentDate="2025-02-08" busy={false} onJump={vi.fn()} /></I18nextProvider>);
    const track = screen.getByTestId("timeline-scroll-thumb").parentElement!;
    vi.spyOn(track, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, right: 20, bottom: 600, width: 20, height: 600, x: 0, y: 0, toJSON: () => ({}) });
    const before = screen.getAllByRole("button").map((button) => button.style.top);
    fireEvent(track, new MouseEvent("pointermove", { bubbles: true, clientY: 300 }));
    expect(screen.getAllByRole("button").map((button) => button.style.top)).toEqual(before);
    expect(screen.getByText("2025")).toBeVisible();
  });
  it("merges month counts and anchors the first date, excluding unknown dates", () => {
    expect(timelineMonths(dates)).toEqual([
      { ...dates[0], count: 7, month: "2026-09" },
      { ...dates[2], month: "2025-02" },
      { ...dates[3], month: "2024-01" },
    ]);
  });
  it("jumps to older dates and marks the visible month", () => {
    const onJump = vi.fn();
    render(<I18nextProvider i18n={i18n}><PhotoTimeline dates={dates} currentDate="2025-02-03" busy={false} onJump={onJump} /></I18nextProvider>);
    expect(screen.getByRole("navigation", { name: "照片时间轴" })).toBeInTheDocument();
    expect(screen.getByText("2025")).toBeVisible();
    expect(screen.getByTestId("timeline-scroll-thumb")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /2025/ })).toHaveAttribute("aria-current", "date");
    fireEvent.click(screen.getByRole("button", { name: /2024/ }));
    expect(onJump).toHaveBeenCalledWith({ ...dates[3], month: "2024-01" });
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
  });
  it("uses daily marks when all photos are within one month", () => {
    render(<I18nextProvider i18n={i18n}><PhotoTimeline dates={dates.slice(0, 2)} currentDate="2026-09-01" busy={false} onJump={vi.fn()} /></I18nextProvider>);
    expect(screen.getAllByRole("button")).toHaveLength(2);
    expect(screen.getByRole("button", { name: /18日/ })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /1日/ })).toHaveAttribute("aria-current", "date");
  });
  it("keeps moving the shared scrollbar inside unknown dates, including an unknown-only library", async () => {
    const unknown = [{ date: null, count: 500, coverAssetId: 1 }];
    const view = (progress: number) => <I18nextProvider i18n={i18n}><PhotoTimeline dates={unknown} currentDate={null} dateProgress={progress} busy={false} onJump={vi.fn()} /></I18nextProvider>;
    const { rerender } = render(view(0.1));
    expect(screen.getByRole("button", { name: "跳转到 未知日期" })).toHaveAttribute("aria-current", "date");
    const thumb = screen.getByTestId("timeline-scroll-thumb");
    await waitFor(() => expect(parseFloat(thumb.style.top)).toBeGreaterThan(0));
    const before = parseFloat(thumb.style.top);
    rerender(view(0.8));
    await waitFor(() => expect(parseFloat(thumb.style.top)).toBeGreaterThan(before));
  });
  it.each(["mouse", "pointer"])("supports %s dragging outside the thin rail until release", (kind) => {
    const onDrag = vi.fn();
    const onJump = vi.fn();
    render(<I18nextProvider i18n={i18n}><PhotoTimeline dates={[{ date: null, count: 500, coverAssetId: 1 }]} currentDate={null} busy={false} onJump={onJump} onDrag={onDrag} /></I18nextProvider>);
    const thumb = screen.getByTestId("timeline-scroll-thumb");
    const track = thumb.parentElement!;
    vi.spyOn(track, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, right: 20, bottom: 600, width: 20, height: 600, x: 0, y: 0, toJSON: () => ({}) });
    if (kind === "mouse") {
      fireEvent.mouseDown(thumb, { button: 0, clientY: 20 });
      fireEvent.mouseMove(window, { clientY: 420 });
      expect(onDrag).toHaveBeenCalled();
      fireEvent.mouseUp(window, { clientY: 420 });
    } else {
      fireEvent(thumb, new MouseEvent("pointerdown", { bubbles: true, button: 0, clientY: 20 }));
      fireEvent(window, new MouseEvent("pointermove", { bubbles: true, clientY: 420 }));
      expect(onDrag).toHaveBeenCalled();
      fireEvent(window, new MouseEvent("pointerup", { bubbles: true, clientY: 420 }));
    }
    expect(onJump).toHaveBeenCalledTimes(1);
    expect(onJump.mock.calls[0][0]).toMatchObject({ date: null, coverAssetId: 1 });
    expect(onJump.mock.calls[0][1]).toBeCloseTo(0.7);
  });
});
