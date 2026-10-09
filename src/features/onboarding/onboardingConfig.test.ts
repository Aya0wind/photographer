import { describe, expect, it } from "vitest";

import { FIXED_TIME_LAYOUT, importRootOf, timeLayoutPreview } from "./onboardingConfig";

describe("importRootOf", () => {
  it("导入目标根 = 照片库 root 本身（无子目录前缀）", () => {
    expect(importRootOf("Y:\\照片")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片\\")).toBe("Y:\\照片");
    expect(importRootOf("Y:\\照片/")).toBe("Y:\\照片");
  });
});

describe("FIXED_TIME_LAYOUT", () => {
  it("落盘布局固定为纯时间公式（物理层无相册/子组维度，2026-10-09 定案）", () => {
    expect(FIXED_TIME_LAYOUT).toBe("{拍摄年}\\{拍摄月}\\{原文件名}");
  });
});

describe("timeLayoutPreview", () => {
  it("目标路径预览 = 库根 + 纯时间公式（尾随分隔符跟随根的形态）", () => {
    expect(timeLayoutPreview("Y:\\照片")).toBe(
      "Y:\\照片\\{拍摄年}\\{拍摄月}\\{原文件名}\\",
    );
    expect(timeLayoutPreview("Y:\\照片\\")).toBe(
      "Y:\\照片\\{拍摄年}\\{拍摄月}\\{原文件名}\\",
    );
  });
});
