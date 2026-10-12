import { describe, expect, it } from "vitest";

import { importRootOf, timeLayoutPreview } from "./onboardingConfig";

describe("importRootOf", () => {
  it("导入目标根 = 照片库 root 本身（无子目录前缀）", () => {
    expect(importRootOf("Y:\\照片")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片\\")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片/")).toBe("Y:\\照片");
  });
});


describe("timeLayoutPreview", () => {
  it("目标路径预览 = 库根 + 纯时间公式（尾随分隔符跟随根的形态）", () => {
    expect(timeLayoutPreview("Y:\\照片")).toBe(
      "Y:\\照片\\{year}\\{month}\\{day}\\{filename}\\",
    );
    expect(timeLayoutPreview("Y:\\照片\\")).toBe(
      "Y:\\照片\\{year}\\{month}\\{day}\\{filename}\\",
    );
  });
});
