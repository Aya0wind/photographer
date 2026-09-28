import { describe, expect, it } from "vitest";

import { FIXED_ALBUM_LAYOUT, importRootOf } from "./onboardingConfig";

describe("importRootOf", () => {
  it("导入目标根 = photoRoot 本身（importSubdir 退役后无子目录前缀）", () => {
    expect(importRootOf("Y:\\照片")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片\\")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片/")).toBe("Y:\\照片");
  });
});

describe("FIXED_ALBUM_LAYOUT", () => {
  it("目录布局固定为时间/相册公式（外层相册创建年月，相册目录内不再分层）", () => {
    expect(FIXED_ALBUM_LAYOUT).toBe("{相册创建年}\\{相册创建月}\\{相册目录}");
  });
});
