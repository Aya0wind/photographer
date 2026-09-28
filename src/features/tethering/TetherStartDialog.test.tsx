/** 联拍启动弹窗：相机清单渲染 / 无相机器态 / 能力门控 / 启动成功关闭 */
import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import {
  tetheringCameraList,
  tetheringStart,
  type CameraInfo,
} from "@/ipc/api";
import TetherStartDialog from "./TetherStartDialog";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    tetheringCameraList: vi.fn(),
    tetheringStart: vi.fn(),
  };
});

const listMock = vi.mocked(tetheringCameraList);
const startMock = vi.mocked(tetheringStart);

const CAPS_FULL = {
  fileTransfer: true,
  standardCapture: true,
  vendorCaptureNikon: false,
  objectAddedEvents: true,
  liveView: true,
};
const CAPS_NONE = {
  fileTransfer: true,
  standardCapture: false,
  vendorCaptureNikon: false,
  objectAddedEvents: false,
  liveView: false,
};

function camera(pnpId: string, name: string, capabilities = CAPS_FULL): CameraInfo {
  return { pnpId, name, capabilities };
}

function renderDialog(albumId = 7, albumName = "棚拍") {
  const onClose = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <TetherStartDialog albumId={albumId} albumName={albumName} onClose={onClose} />
    </I18nextProvider>,
  );
  return { onClose };
}

beforeEach(() => {
  listMock.mockReset().mockResolvedValue([]);
  startMock.mockReset();
});

describe("TetherStartDialog 联拍启动弹窗", () => {
  it("扫描后渲染相机清单；可触发拍摄的相机可点", async () => {
    listMock.mockResolvedValue([
      camera("CAM_A", "Nikon D750"),
      camera("CAM_B", "老相机", CAPS_NONE),
    ]);
    renderDialog();

    const a = await screen.findByTestId("tether-start-camera-CAM_A");
    expect(a).toBeInTheDocument();
    expect(screen.getByTestId("tether-start-camera-CAM_B")).toBeInTheDocument();
    expect(a).not.toBeDisabled();
    expect(screen.getByTestId("tether-start-camera-CAM_B")).toBeDisabled();
  });

  it("无相机：空态提示 + 重新扫描按钮可重试", async () => {
    listMock.mockResolvedValue([]);
    renderDialog();

    expect(await screen.findByTestId("tether-start-empty")).toHaveTextContent("未发现可联拍的相机");
    expect(screen.getByTestId("tether-start-sony-hint")).toHaveTextContent("PC Remote");
    listMock.mockResolvedValue([camera("CAM_A", "A7R V")]);
    fireEvent.click(screen.getByTestId("tether-start-refresh"));
    expect(await screen.findByTestId("tether-start-camera-CAM_A")).toBeInTheDocument();
  });

  it("选择相机 → tetheringStart(albumId, pnpId)；成功即关闭弹窗", async () => {
    listMock.mockResolvedValue([camera("CAM_A", "Nikon D750")]);
    const { onClose } = renderDialog(7, "棚拍");

    fireEvent.click(await screen.findByTestId("tether-start-camera-CAM_A"));
    await waitFor(() => expect(startMock).toHaveBeenCalledWith(7, "CAM_A"));
    startMock.mockResolvedValue({
      ok: true,
      session: {
        id: "s1",
        libraryId: "lib-1",
        albumId: 7,
        albumName: "棚拍",
        camera: camera("CAM_A", "Nikon D750"),
        settings: [],
        photos: [],
        connected: true,
        receiving: false,
        error: null,
      },
    });
    // 上一轮 start 调用发生在 mock 配置前，补一轮成功路径
    fireEvent.click(screen.getByTestId("tether-start-camera-CAM_A"));
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it("启动失败：弹窗内联错误，不关闭", async () => {
    listMock.mockResolvedValue([camera("CAM_A", "Nikon D750")]);
    startMock.mockResolvedValue({ ok: false, error: "相机被占用" });
    const { onClose } = renderDialog();

    fireEvent.click(await screen.findByTestId("tether-start-camera-CAM_A"));
    expect(await screen.findByTestId("tether-start-error")).toHaveTextContent("相机被占用");
    expect(onClose).not.toHaveBeenCalled();
  });
});
