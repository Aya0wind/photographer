import { afterEach, describe, expect, it, vi } from "vitest";

import { fireEvent } from "@testing-library/react";

import type { MapCluster } from "@/ipc/api/map";
import { createBubbleElement } from "./PhotoBubble";

/**
 * 气泡「N 张 →」角标与样图点击语义（拍摄地图联动照片子页入口）：
 * - 角标：可聚焦（role/tabindex）、aria-label，点击/Enter 触发 onOpenPhotos
 *   且 stopPropagation 不冒泡到气泡本体的下钻
 * - 样图缩略图（主图/副图）点击同样触发 onOpenPhotos
 * - 气泡本体点击语义不变（onDrill 下钻/县层放大）
 */

function clusterFixture(overrides: Partial<MapCluster> = {}): MapCluster {
  return {
    regionId: 10,
    name: "中国",
    lat: 30,
    lon: 104,
    count: 12,
    samples: [
      { id: 1, path: "X:/a.jpg", kind: "photo" },
      { id: 2, path: "X:/b.jpg", kind: "photo" },
      { id: 3, path: "X:/c.jpg", kind: "photo" },
    ],
    ...overrides,
  };
}

function mountBubble(onDrill: () => void, onOpenPhotos: (regionId: number) => void) {
  const element = createBubbleElement(clusterFixture(), 0, onDrill, {
    onOpenPhotos,
    photosLabel: "12 张 →",
  });
  document.body.appendChild(element);
  return element;
}

afterEach(() => {
  document.querySelectorAll(".map-bubble").forEach((el) => el.remove());
});

describe("PhotoBubble：打开照片入口", () => {
  it("角标按钮可聚焦、带 aria-label；点击触发 onOpenPhotos(regionId) 且不下钻", () => {
    const onDrill = vi.fn();
    const onOpenPhotos = vi.fn();
    const bubble = mountBubble(onDrill, onOpenPhotos);

    const badge = bubble.querySelector<HTMLElement>(".map-bubble-photos");
    expect(badge).not.toBeNull();
    expect(badge!.getAttribute("role")).toBe("button");
    expect(badge!.getAttribute("tabindex")).toBe("0");
    expect(badge!.getAttribute("aria-label")).toBe("12 张 →");
    expect(badge!.textContent).toBe("12 张 →");

    fireEvent.click(badge!);
    expect(onOpenPhotos).toHaveBeenCalledTimes(1);
    expect(onOpenPhotos).toHaveBeenCalledWith(10);
    expect(onDrill).not.toHaveBeenCalled();
  });

  it("角标键盘操作（Enter/Space）同样触发 onOpenPhotos", () => {
    const onDrill = vi.fn();
    const onOpenPhotos = vi.fn();
    const bubble = mountBubble(onDrill, onOpenPhotos);
    const badge = bubble.querySelector<HTMLElement>(".map-bubble-photos")!;

    fireEvent.keyDown(badge, { key: "Enter" });
    fireEvent.keyDown(badge, { key: " " });
    expect(onOpenPhotos).toHaveBeenCalledTimes(2);
    expect(onOpenPhotos).toHaveBeenNthCalledWith(1, 10);
    expect(onDrill).not.toHaveBeenCalled();
  });

  it("样图缩略图（主图/副图）点击 → onOpenPhotos，不冒泡到下钻", () => {
    const onDrill = vi.fn();
    const onOpenPhotos = vi.fn();
    const bubble = mountBubble(onDrill, onOpenPhotos);

    fireEvent.click(bubble.querySelector<HTMLElement>(".map-bubble-main")!);
    expect(onOpenPhotos).toHaveBeenCalledTimes(1);
    expect(onOpenPhotos).toHaveBeenCalledWith(10);

    fireEvent.click(bubble.querySelector<HTMLElement>(".map-bubble-extra-1")!);
    expect(onOpenPhotos).toHaveBeenCalledTimes(2);
    expect(onDrill).not.toHaveBeenCalled();
  });

  it("气泡本体点击语义不变：onDrill（下钻/县层放大）", () => {
    const onDrill = vi.fn();
    const onOpenPhotos = vi.fn();
    const bubble = mountBubble(onDrill, onOpenPhotos);

    fireEvent.click(bubble);
    expect(onDrill).toHaveBeenCalledTimes(1);
    expect(onOpenPhotos).not.toHaveBeenCalled();
  });
});
