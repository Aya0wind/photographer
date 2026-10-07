import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import ViewerImageLayer, { viewerImageSize } from "./ViewerImageLayer";

describe("large preview rendering", () => {
  it("fits without enlarging small images and aligns output to physical pixels", () => {
    expect(viewerImageSize({ width: 6000, height: 4000 }, { width: 1200, height: 800 }, 1)).toEqual({ width: 1200, height: 800 });
    expect(viewerImageSize({ width: 320, height: 200 }, { width: 1200, height: 800 }, 1)).toEqual({ width: 320, height: 200 });
    const size = viewerImageSize({ width: 6000, height: 4000 }, { width: 1177, height: 793 }, 1.2, 1.25)!;
    expect(size.width * 1.25).toBeCloseTo(Math.round(size.width * 1.25));
    expect(size.height * 1.25).toBeCloseTo(Math.round(size.height * 1.25));
  });
  it("zooms by resizing the same decoded image and keeps pan, rotation and viewport resizing", () => {
    const view = (scale: number, width = 1200, height = 800, dragging = false) => <ViewerImageLayer src="asset://large.jpg" alt="large preview" viewport={{ width, height }} view={{ scale, x: 40, y: 30, rotation: 90 }} dragging={dragging} />;
    const { rerender } = render(view(1));
    const image = screen.getByRole("img");
    Object.defineProperties(image, { naturalWidth: { value: 6000 }, naturalHeight: { value: 4000 } });
    fireEvent.load(image);
    expect(image.style.width).toBe("1200px");
    expect(image.style.height).toBe("800px");
    rerender(view(4));
    expect(screen.getByRole("img")).toBe(image);
    expect(image.style.width).toBe("4800px");
    expect(image.style.height).toBe("3200px");
    expect(image.style.transform).toBe("translate(40px, 30px) rotate(90deg)");
    expect(image.style.transform).not.toContain("scale");
    expect(image.className).not.toContain("will-change-transform");
    rerender(view(4, 900, 600, true));
    expect(image.style.width).toBe("3600px");
    expect(image.style.height).toBe("2400px");
    expect(image.style.transition).toBe("none");
  });
});
