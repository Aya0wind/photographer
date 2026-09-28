import { describe, it, expect } from "vitest";
import { devicePresentationKind } from "./devicePresentation";

describe("设备展示类型", () => {
  it("WPD 盘符别名属于读卡器，相机型号仍显示为相机", () => {
    for (const name of ["G:", "J:\\", " l:/ ", "CFexpress", "MassStorageClass"]) {
      expect(devicePresentationKind({ kind: "mtp", name })).toBe("reader");
    }
    expect(devicePresentationKind({ kind: "mtp", name: "ILCE-7RM5" })).toBe("camera");
    expect(devicePresentationKind({ kind: "folder", name: "相机备份" })).toBe("folder");
  });
});
