import { describe, expect, it } from "vitest";

import { canTriggerCapture, sameCameraDevice, tetheringChips } from "./tetheringCapabilities";

const caps = (over: Partial<Parameters<typeof canTriggerCapture>[0]>) => ({
  fileTransfer: false,
  standardCapture: false,
  vendorCaptureNikon: false,
  objectAddedEvents: false,
  liveView: false,
  ...over,
});

describe("tetheringChips / canTriggerCapture（能力→chips 映射与拍摄门控）", () => {
  it("拍摄门控：vendorCaptureNikon 与 standardCapture 任一真即可触发", () => {
    expect(canTriggerCapture(caps({}))).toBe(false);
    expect(canTriggerCapture(caps({ vendorCaptureNikon: true }))).toBe(true);
    expect(canTriggerCapture(caps({ standardCapture: true }))).toBe(true);
    expect(canTriggerCapture(caps({ vendorCaptureNikon: true, standardCapture: true }))).toBe(true);
    // 其他能力位不影响门控
    expect(canTriggerCapture(caps({ fileTransfer: true, objectAddedEvents: true, liveView: true }))).toBe(false);
  });

  it("chips 固定三枚：文件传输 / 可触发拍摄（合并厂商与标准）/ 自动收片", () => {
    expect(tetheringChips(caps({
      fileTransfer: true,
      vendorCaptureNikon: true,
      objectAddedEvents: true,
    }))).toEqual([
      { id: "fileTransfer", supported: true },
      { id: "triggerCapture", supported: true },
      { id: "autoIngest", supported: true },
    ]);

    expect(tetheringChips(caps({ standardCapture: true }))).toEqual([
      { id: "fileTransfer", supported: false },
      { id: "triggerCapture", supported: true },
      { id: "autoIngest", supported: false },
    ]);

    // 能力全 false：三枚 chips 全 unsupported（无拍摄入口由调用方按门控隐藏）
    expect(tetheringChips(caps({}))).toEqual([
      { id: "fileTransfer", supported: false },
      { id: "triggerCapture", supported: false },
      { id: "autoIngest", supported: false },
    ]);
  });

  it("liveView 不参与 chips（后续阶段能力）", () => {
    const chips = tetheringChips(caps({ liveView: true }));
    expect(chips.map((c) => c.id)).not.toContain("liveView");
  });
});

describe("sameCameraDevice（事件 pnpId ↔ 设备 id 匹配）", () => {
  it("普通 id 精确比较；PnP 路径大小写归一比较", () => {
    expect(sameCameraDevice("MTP_CAM", "MTP_CAM")).toBe(true);
    expect(sameCameraDevice("MTP_CAM", "MTP_OTHER")).toBe(false);
    expect(sameCameraDevice("\\\\?\\USB#VID_04B0#1#{AA}", "\\\\?\\usb#vid_04b0#1#{aa}")).toBe(true);
    expect(sameCameraDevice("\\\\?\\USB#VID_04B0#1", "\\\\?\\USB#VID_04B0#2")).toBe(false);
  });
});
