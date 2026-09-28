import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import EditorCanvas, { type EditorTool } from "./EditorCanvas";
import { defaultRecipe } from "../lib/recipe";

vi.mock("react-konva", () => {
  const Container = ({ children }: { children?: ReactNode }) => <div>{children}</div>;
  return Object.fromEntries(["Circle", "Group", "Image", "Layer", "Line", "Rect", "Stage", "Text", "Transformer"].map((name) => [name, Container]));
});

beforeEach(() => {
  vi.stubGlobal("Image", class {
    naturalWidth = 1200;
    naturalHeight = 800;
    onload: (() => void) | null = null;
    set src(_value: string) { queueMicrotask(() => this.onload?.()); }
  });
  vi.stubGlobal("PointerEvent", class extends MouseEvent {
    pointerId: number;
    constructor(type: string, options: PointerEventInit = {}) {
      super(type, options);
      this.pointerId = options.pointerId ?? 1;
    }
  });
});
afterEach(() => vi.unstubAllGlobals());

async function canvas(tool: EditorTool = "view") {
  const onZoom = vi.fn();
  const noop = vi.fn();
  render(<EditorCanvas src="test.jpg" zoom={2} onZoom={onZoom} fallbackSize={null} recipe={defaultRecipe()} tool={tool}
    brushOptions={{ color: "#FF0000", widthRel: 0.01 }} cropRatio={null} cropDraft={null}
    onCropDraftChange={noop} selectedTextId={null} onSelectText={noop} onPlaceText={noop} onTextChange={noop}
    onStrokeCommit={noop} onGestureStart={noop} onGestureEnd={noop} onImageReady={noop} onImageError={noop} />);
  const viewport = await screen.findByTestId("editor-canvas-viewport");
  Object.defineProperties(viewport, {
    scrollWidth: { value: 1800, configurable: true }, clientWidth: { value: 600, configurable: true },
    scrollHeight: { value: 1200, configurable: true }, clientHeight: { value: 400, configurable: true },
    setPointerCapture: { value: vi.fn() }, hasPointerCapture: { value: vi.fn(() => true) }, releasePointerCapture: { value: vi.fn() },
  });
  viewport.scrollLeft = 200;
  viewport.scrollTop = 150;
  return { viewport, onZoom };
}

describe("编辑器画布缩放与拖动", () => {
  it("滚轮放大/缩小并阻止滚动页面", async () => {
    const { viewport, onZoom } = await canvas();
    expect(fireEvent.wheel(viewport, { deltaY: -100, cancelable: true })).toBe(false);
    expect(onZoom).toHaveBeenLastCalledWith(1.1);
    fireEvent.wheel(viewport, { deltaY: 100 });
    expect(onZoom).toHaveBeenLastCalledWith(1 / 1.1);
  });

  it("查看模式拖动平移，松开后停止", async () => {
    const { viewport } = await canvas();
    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 400, clientY: 300 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 250, clientY: 180 });
    expect(viewport.scrollLeft).toBe(350);
    expect(viewport.scrollTop).toBe(270);
    fireEvent.pointerUp(viewport, { pointerId: 1 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 100, clientY: 100 });
    expect(viewport.scrollLeft).toBe(350);
    expect(viewport.releasePointerCapture).toHaveBeenCalledWith(1);
  });

  it("画笔工具的拖动不平移照片", async () => {
    const { viewport } = await canvas("brush");
    fireEvent.pointerDown(viewport, { pointerId: 1, button: 0, clientX: 400, clientY: 300 });
    fireEvent.pointerMove(viewport, { pointerId: 1, clientX: 250, clientY: 180 });
    expect(viewport.scrollLeft).toBe(200);
    expect(viewport.scrollTop).toBe(150);
  });
});
