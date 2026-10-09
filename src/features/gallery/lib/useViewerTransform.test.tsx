import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useViewerTransform } from "./useViewerTransform";

vi.mock("@/lib/motion", () => ({ useMotionOn: () => true }));
let current: ReturnType<typeof useViewerTransform>;
let callbacks: Map<number, FrameRequestCallback>;
let nextFrame = 0;
function Probe() {
  current = useViewerTransform(1);
  return <div ref={current.stageRef} data-testid="stage" />;
}
function frame(time: number) {
  const pending = [...callbacks.values()];
  callbacks.clear();
  act(() => pending.forEach((callback) => callback(time)));
}
beforeEach(() => {
  callbacks = new Map();
  nextFrame = 0;
  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => { callbacks.set(++nextFrame, callback); return nextFrame; });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => callbacks.delete(id));
});
afterEach(() => vi.unstubAllGlobals());

describe("preview zoom stability", () => {
  it("缩放允许25%到800%，复位仍回到适应窗口100%",()=>{
    render(<Probe/>);
    act(()=>current.setZoom(0.01));
    expect(current.view.scale).toBe(0.25);
    expect(current.view.x).toBe(0);
    expect(current.view.y).toBe(0);
    act(()=>current.toggleZoom());
    expect(current.view.scale).toBe(1);
    act(()=>current.setZoom(12));
    expect(current.view.scale).toBe(8);
    act(()=>current.resetView());
    expect(current.view.scale).toBe(1);
  });
  it("keeps the cursor's image point fixed during animation and successive wheel input", () => {
    render(<Probe />);
    const stage = screen.getByTestId("stage");
    vi.spyOn(stage, "getBoundingClientRect").mockReturnValue({ x: 0, y: 0, top: 0, left: 0, bottom: 300, right: 400, width: 400, height: 300, toJSON: () => ({}) });
    fireEvent.wheel(stage, { deltaY: -100, clientX: 300, clientY: 200 });
    frame(0);
    frame(60);
    expect(current.renderedView.scale).toBeGreaterThan(1);
    expect(current.renderedView.scale).toBeLessThan(1.2);
    expect((100 - current.renderedView.x) / current.renderedView.scale).toBeCloseTo(100);
    expect((50 - current.renderedView.y) / current.renderedView.scale).toBeCloseTo(50);
    fireEvent.wheel(stage, { deltaY: -100, clientX: 300, clientY: 200 });
    frame(80);
    frame(140);
    expect((100 - current.renderedView.x) / current.renderedView.scale).toBeCloseTo(100);
    expect((50 - current.renderedView.y) / current.renderedView.scale).toBeCloseTo(50);
    frame(200);
    expect(current.renderedView.scale).toBeCloseTo(1.44);
    expect(current.animating).toBe(false);
  });
  it("uses precision-wheel magnitude rather than jumping 20 percent for every tiny event", () => {
    render(<Probe />);
    const stage = screen.getByTestId("stage");
    fireEvent.wheel(stage, { deltaY: -1 });
    expect(current.view.scale).toBeGreaterThan(1);
    expect(current.view.scale).toBeLessThan(1.01);
    const scale = current.view.scale;
    fireEvent.wheel(stage, { deltaY: 0 });
    expect(current.view.scale).toBe(scale);
  });
});
