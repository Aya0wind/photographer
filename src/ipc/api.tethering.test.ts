import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import {
  cameraCapture,
  cameraProbe,
  tetheringCameraList,
  tetheringSession,
  tetheringSettingSet,
  tetheringSettings,
  tetheringStart,
  tetheringCapture,
  tetheringFrame,
  tetheringPhotoPreview,
  tetheringStop,
  type CameraInfo,
} from "./api";

const invokeMock = vi.mocked(invoke);

const NIKON_CAPS = {
  fileTransfer: true,
  standardCapture: false,
  vendorCaptureNikon: true,
  objectAddedEvents: true,
  liveView: false,
};

const NIKON_CAMERA: CameraInfo = {
  pnpId: "\\\\?\\usb#vid_04b0&pid_0438#000000000000##{...}",
  name: "Nikon D750",
  capabilities: NIKON_CAPS,
};

beforeEach(() => {
  invokeMock.mockReset();
});

describe("联拍 IPC 封装（阶段 E-1）", () => {
  it("tetheringCameraList：成功透传命令名并归一条目", async () => {
    invokeMock.mockResolvedValue([
      { ...NIKON_CAMERA },
      { pnpId: "", name: "坏条目", capabilities: NIKON_CAPS }, // pnpId 为空 → 剔除
      null, // 形状异常 → 剔除
    ]);

    await expect(tetheringCameraList()).resolves.toEqual([NIKON_CAMERA]);
    expect(invokeMock).toHaveBeenCalledWith("tethering_camera_list", undefined);
  });

  it("tetheringCameraList：能力位缺省按 false 归一（不因部分缺失丢整条）", async () => {
    invokeMock.mockResolvedValue([{ pnpId: "CAM_A", name: "A" }]);

    await expect(tetheringCameraList()).resolves.toEqual([
      { pnpId: "CAM_A", name: "A", capabilities: {
        fileTransfer: false, standardCapture: false, vendorCaptureNikon: false,
        objectAddedEvents: false, liveView: false,
      } },
    ]);
  });

  it("tetheringCameraList：命令失败/非数组回退 []", async () => {
    invokeMock.mockRejectedValue(new Error("__TAURI_INTERNALS__"));

    await expect(tetheringCameraList()).resolves.toEqual([]);
  });

  it("cameraProbe：传 pnpId（camelCase 负载）并归一返回", async () => {
    invokeMock.mockResolvedValue({ ...NIKON_CAMERA });

    await expect(cameraProbe(NIKON_CAMERA.pnpId)).resolves.toEqual(NIKON_CAMERA);
    expect(invokeMock).toHaveBeenCalledWith("camera_probe", { pnpId: NIKON_CAMERA.pnpId });
  });

  it("cameraProbe：name 缺失回退 pnpId；非对象负载返回 null", async () => {
    invokeMock.mockResolvedValueOnce({ pnpId: "CAM_B", capabilities: {} });
    invokeMock.mockResolvedValueOnce("oops");

    await expect(cameraProbe("CAM_B")).resolves.toEqual({
      pnpId: "CAM_B",
      name: "CAM_B",
      capabilities: {
        fileTransfer: false, standardCapture: false, vendorCaptureNikon: false,
        objectAddedEvents: false, liveView: false,
      },
    });
    await expect(cameraProbe("CAM_B")).resolves.toBeNull();
  });

  it("cameraProbe：命令失败（后端在途）返回 null", async () => {
    invokeMock.mockRejectedValue(new Error("command camera_probe not found"));

    await expect(cameraProbe("CAM_B")).resolves.toBeNull();
  });

  it("cameraCapture：省略 timeoutMs 时只传 pnpId；携带时透传", async () => {
    invokeMock.mockResolvedValue({ objectName: "DSC_0001.JPG", objectSize: 1024, error: null });

    await cameraCapture("CAM_B");
    expect(invokeMock).toHaveBeenCalledWith("camera_capture", { pnpId: "CAM_B" });

    await cameraCapture("CAM_B", 8000);
    expect(invokeMock).toHaveBeenCalledWith("camera_capture", { pnpId: "CAM_B", timeoutMs: 8000 });
  });

  it("cameraCapture：成功/后端业务错误的负载形状原样归一", async () => {
    invokeMock.mockResolvedValueOnce({ objectName: "DSC_0001.JPG", objectSize: 1024, error: null });
    await expect(cameraCapture("CAM_B")).resolves.toEqual({
      objectName: "DSC_0001.JPG", objectSize: 1024, error: null,
    });

    invokeMock.mockResolvedValueOnce({ objectName: null, objectSize: null, error: "设备被占用" });
    await expect(cameraCapture("CAM_B")).resolves.toEqual({
      objectName: null, objectSize: null, error: "设备被占用",
    });
  });

  it("cameraCapture：invoke 抛业务文案时透传为 error；不可用类失败 error=null", async () => {
    invokeMock.mockRejectedValueOnce(new Error("相机无响应"));
    await expect(cameraCapture("CAM_B")).resolves.toMatchObject({ error: "相机无响应" });

    invokeMock.mockRejectedValueOnce(new Error("command camera_capture not found"));
    await expect(cameraCapture("CAM_B")).resolves.toEqual({
      objectName: null, objectSize: null, error: null,
    });
  });

  it("cameraCapture：非对象/缺字段的返回载荷归一为 null 字段", async () => {
    invokeMock.mockResolvedValueOnce(null);
    invokeMock.mockResolvedValueOnce({ objectName: "", objectSize: "x", error: "" });

    await expect(cameraCapture("CAM_B")).resolves.toEqual({ objectName: null, objectSize: null, error: null });
    await expect(cameraCapture("CAM_B")).resolves.toEqual({ objectName: null, objectSize: null, error: null });
  });
});

const SESSION_DTO = {
  id: "s1",
  libraryId: "lib-1",
  albumId: 7,
  albumName: "棚拍",
  camera: { ...NIKON_CAMERA },
  settings: [
    {
      id: "shutter",
      current: "125",
      writable: true,
      options: [
        { value: "125", label: "1/125" },
        { value: "250", label: "1/250" },
      ],
    },
    { id: "iso", current: "400", writable: false, options: [] },
  ],
  photos: [{ id: 9, name: "DSC_0001.JPG", kind: "photo" }],
  connected: true,
  receiving: false,
  error: null,
};

describe("联拍会话 IPC 封装（tethering_*）", () => {
  it("tetheringStart：{albumId, cameraId} 负载；成功归一会话；业务错误透传", async () => {
    invokeMock.mockResolvedValueOnce(SESSION_DTO);
    const ok = await tetheringStart(7, NIKON_CAMERA.pnpId);
    expect(invokeMock).toHaveBeenCalledWith("tethering_start", {
      albumId: 7,
      cameraId: NIKON_CAMERA.pnpId,
    });
    expect(ok.ok).toBe(true);
    if (ok.ok) {
      expect(ok.session.albumName).toBe("棚拍");
      expect(ok.session.settings).toHaveLength(2);
      expect(ok.session.settings[0].options[1].label).toBe("1/250");
    }

    invokeMock.mockRejectedValueOnce(new Error("已有联机拍摄窗口"));
    const failed = await tetheringStart(7, "x");
    expect(failed.ok).toBe(false);
    if (!failed.ok) expect(failed.error).toBe("已有联机拍摄窗口");
  });

  it("tetheringSession/tetheringSettings：形状异常归 null（会话已结束语义）", async () => {
    invokeMock.mockResolvedValueOnce(SESSION_DTO);
    expect((await tetheringSession("s1"))?.albumId).toBe(7);

    invokeMock.mockResolvedValueOnce({ nonsense: true });
    expect(await tetheringSession("s1")).toBeNull();

    invokeMock.mockRejectedValueOnce(new Error("command tethering_session not found"));
    expect(await tetheringSession("s1")).toBeNull();

    invokeMock.mockResolvedValueOnce(SESSION_DTO.settings);
    expect((await tetheringSettings("s1"))?.[1].writable).toBe(false);
  });

  it("tetheringSettingSet：载荷完整；业务错误不 catch 直接上抛", async () => {
    invokeMock.mockResolvedValueOnce(SESSION_DTO.settings);
    await tetheringSettingSet("s1", "shutter", "250");
    expect(invokeMock).toHaveBeenCalledWith("tethering_setting_set", {
      sessionId: "s1",
      id: "shutter",
      value: "250",
    });

    invokeMock.mockRejectedValueOnce(new Error("无效拍摄参数"));
    await expect(tetheringSettingSet("s1", "iso", "abc")).rejects.toThrow("无效拍摄参数");
  });

  it("tetheringCapture：ok / 业务错误文案 / 不可用 error=null", async () => {
    invokeMock.mockResolvedValueOnce(null);
    expect((await tetheringCapture("s1")).ok).toBe(true);

    invokeMock.mockRejectedValueOnce(new Error("相机已断开"));
    const failed = await tetheringCapture("s1");
    expect(failed.ok).toBe(false);
    if (!failed.ok) expect(failed.error).toBe("相机已断开");

    invokeMock.mockRejectedValueOnce(new Error("__TAURI_INTERNALS__"));
    const unavailable = await tetheringCapture("s1");
    expect(unavailable.ok).toBe(false);
    if (!unavailable.ok) expect(unavailable.error).toBeNull();
  });

  it("tetheringFrame/tetheringPhotoPreview：data URL 直传；null/异形归 null", async () => {
    invokeMock.mockResolvedValueOnce("data:image/jpeg;base64,AAA");
    expect(await tetheringFrame("s1")).toBe("data:image/jpeg;base64,AAA");

    invokeMock.mockResolvedValueOnce(null);
    expect(await tetheringFrame("s1")).toBeNull();

    invokeMock.mockResolvedValueOnce("not-a-url");
    expect(await tetheringFrame("s1")).toBeNull();

    invokeMock.mockResolvedValueOnce("data:image/jpeg;base64,BBB");
    expect(await tetheringPhotoPreview("s1", 9, 256)).toBe("data:image/jpeg;base64,BBB");
    expect(invokeMock).toHaveBeenCalledWith("tethering_photo_preview", {
      sessionId: "s1",
      assetId: 9,
      size: 256,
    });
  });

  it("tetheringStop：失败静默（窗口销毁路径由后端兜底）", async () => {
    invokeMock.mockRejectedValueOnce(new Error("anything"));
    await expect(tetheringStop("s1")).resolves.toBeUndefined();
  });
});
