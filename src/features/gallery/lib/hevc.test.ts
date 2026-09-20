import { afterEach, describe, expect, it, vi } from "vitest";

import { supportsHevc } from "./hevc";

function mockCanPlayType(result: string | (() => string)) {
  const canPlayType = typeof result === "function" ? result : () => result;
  const createElement = vi.spyOn(document, "createElement").mockImplementation((tag: string) => {
    if (tag === "video") return { canPlayType } as unknown as HTMLElement;
    return {} as HTMLElement;
  });
  return createElement;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("HEVC 播放能力检测", () => {
  it("canPlayType 返回 probably → 支持", () => {
    mockCanPlayType("probably");
    expect(supportsHevc()).toBe(true);
  });

  it("canPlayType 返回空串（系统无 HEVC 扩展）→ 不支持", () => {
    mockCanPlayType("");
    expect(supportsHevc()).toBe(false);
  });

  it("maybe 也算支持（宁可试播不要误杀）", () => {
    mockCanPlayType("maybe");
    expect(supportsHevc()).toBe(true);
  });

  it("createElement 抛错 → 保守 false（回退系统播放）", () => {
    vi.spyOn(document, "createElement").mockImplementation(() => {
      throw new Error("no dom");
    });
    expect(supportsHevc()).toBe(false);
  });
});
