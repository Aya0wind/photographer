import { describe, expect, it } from "vitest";

import { previewTemplate, unknownTokens } from "./onboardingConfig";

describe("unknownTokens", () => {
  it("returns empty for valid templates", () => {
    expect(unknownTokens("{YYYY}/{MM-DD}/{原文件名}")).toEqual([]);
    expect(unknownTokens("{YYYY}/{相机}/{镜头}/{HH}{mm}{ss}")).toEqual([]);
  });

  it("detects unknown tokens", () => {
    expect(unknownTokens("{YYYY}/{XX}")).toEqual(["XX"]);
    expect(unknownTokens("{bad}/{YYYY}")).toEqual(["bad"]);
  });

  it("handles template without tokens", () => {
    expect(unknownTokens("photos")).toEqual([]);
  });

  it("keeps duplicate unknown tokens", () => {
    expect(unknownTokens("{AA}/{AA}")).toEqual(["AA", "AA"]);
  });
});

describe("previewTemplate", () => {
  it("renders default template with sample values", () => {
    expect(previewTemplate("{YYYY}/{MM-DD}/{原文件名}", "Y:\\照片")).toBe(
      "Y:\\照片\\2026\\09-18\\IMG_0001.CR3",
    );
  });

  it("appends sample file when template lacks filename token", () => {
    expect(previewTemplate("{YYYY}", "Y:\\照片")).toBe("Y:\\照片\\2026\\IMG_0001.CR3");
  });

  it("trims trailing separators of photo root", () => {
    expect(previewTemplate("{YYYY}", "Y:\\照片\\")).toBe("Y:\\照片\\2026\\IMG_0001.CR3");
  });

  it("leaves unknown tokens as-is in preview", () => {
    expect(previewTemplate("{XX}", "Y:\\照片")).toBe("Y:\\照片\\{XX}\\IMG_0001.CR3");
  });
});
