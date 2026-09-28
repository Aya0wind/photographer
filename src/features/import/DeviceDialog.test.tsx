import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useLocation } from "react-router";

import i18n from "@/i18n";
import DeviceDialog from "./DeviceDialog";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import { cameraCapture, cameraProbe, type CameraCaptureResult, type CameraInfo, type DeviceSnapshot } from "@/ipc/api";

// 联拍（E-1）：探测/拍摄命令按 api 模块 mock；事件通道与刷新走真实链路
//（subscribeAppEvents → 全局 listen mock；refreshDevice → device_scan invoke mock）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    cameraProbe: vi.fn(),
    cameraCapture: vi.fn(),
  };
});

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const probeMock = vi.mocked(cameraProbe);
const captureMock = vi.mocked(cameraCapture);
const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

function snapshot(id: string, name: string, kind: "volume" | "mtp", newFiles: number): DeviceSnapshot {
  return {
    id,
    name,
    kind,
    filesByKind: { photo: 100, raw: 40, other: 1 },
    bytesTotal: 8_000_000_000,
    newFiles,
  };
}

/** 相机能力载荷（默认 Nikon：厂商码可拍 + 自动收片） */
function nikonCaps(over: Partial<CameraInfo["capabilities"]> = {}): CameraInfo["capabilities"] {
  return {
    fileTransfer: true,
    standardCapture: false,
    vendorCaptureNikon: true,
    objectAddedEvents: true,
    liveView: false,
    ...over,
  };
}

/** 向 DeviceDialog 的事件订阅发射一条 app://event 负载 */
function emitAppEvent(payload: unknown): void {
  const call = listenMock.mock.calls.find(([name]) => name === "app://event");
  expect(call, "DeviceDialog 应订阅 app://event").toBeDefined();
  const handler = call![1] as (event: unknown) => void;
  act(() => {
    handler({ event: { id: 0 }, id: 0, payload });
  });
}

function ImportProbe() {
  const location = useLocation();
  return <div data-testid="import-probe">{`${location.pathname}${location.search}`}</div>;
}

function renderDialog() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/"]}>
        <Routes>
          <Route path="/" element={<DeviceDialog />} />
          <Route path="/import" element={<ImportProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  resetImportStoreForTests();
  probeMock.mockReset().mockResolvedValue(null); // 缺省：探测失败=未知（未探测态）
  captureMock.mockReset();
  invokeMock.mockReset();
  listenMock.mockReset().mockResolvedValue(() => {});
});

describe("DeviceDialog", () => {
  it("队列为空时不渲染弹窗", () => {
    renderDialog();

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("展示设备名/类型徽标/分色统计/总量/新增数", () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SanDisk 64G", "volume", 12)],
      promptQueue: ["E:"],
    });

    renderDialog();

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.getByText("SanDisk 64G")).toBeInTheDocument();
    expect(screen.getByText("读卡器")).toBeInTheDocument();
    // 分色统计：照片 100 / RAW 40
    expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("100");
    expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("40");
    // 总量 8 GB + 新增
    expect(screen.getByText("7.5 GB")).toBeInTheDocument();
    expect(screen.getByTestId("device-dialog-new")).toHaveTextContent("12 个新文件");
  });

  it("MTP 设备显示相机徽标", () => {
    useImportStore.setState({
      devices: [snapshot("MTP_CAM", "EOS R5", "mtp", 5)],
      promptQueue: ["MTP_CAM"],
    });

    renderDialog();

    expect(screen.getByText("相机")).toBeInTheDocument();
    expect(screen.getByText("EOS R5")).toBeInTheDocument();
  });

  it("通过 MTP 暴露的存储卡仍显示为读卡器", () => {
    useImportStore.setState({
      devices: [snapshot("MTP_CARD", "SanDisk SDXC 存储卡", "mtp", 5)],
      promptQueue: ["MTP_CARD"],
    });

    renderDialog();
    expect(screen.getByText("读卡器")).toBeInTheDocument();
    expect(screen.queryByText("相机")).not.toBeInTheDocument();
  });

  it("开始导入：出队并携带 device 参数跳转导入向导", async () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SanDisk 64G", "volume", 12)],
      promptQueue: ["E:"],
    });

    renderDialog();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "开始导入" }));

    expect(await screen.findByTestId("import-probe")).toHaveTextContent("/import?device=E%3A");
    expect(useImportStore.getState().promptQueue).toEqual([]);
  });

  it("忽略：关闭当前弹窗，队列中下一台接着弹出", async () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SD 卡", "volume", 1), snapshot("F:", "CF 卡", "volume", 2)],
      promptQueue: ["E:", "F:"],
    });

    renderDialog();
    expect(screen.getByText("还有 1 台设备待处理")).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "忽略" }));

    await waitFor(() => {
      expect(screen.getByText("CF 卡")).toBeInTheDocument();
    });
    expect(useImportStore.getState().promptQueue).toEqual(["F:"]);
  });
});

describe("DeviceDialog 联拍区（阶段 E-1）", () => {
  it("相机设备弹出即懒探测一次（camera_probe 传设备 id），展示能力 chips 与拍摄按钮", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_NIKON", "Nikon D750", "mtp", 5)],
      promptQueue: ["MTP_NIKON"],
    });
    probeMock.mockResolvedValue({ pnpId: "MTP_NIKON", name: "Nikon D750", capabilities: nikonCaps() });

    renderDialog();

    const chips = await screen.findAllByTestId("device-dialog-cap");
    expect(chips.map((c) => [c.dataset.capability, c.dataset.supported])).toEqual([
      ["fileTransfer", "true"],
      ["triggerCapture", "true"],
      ["autoIngest", "true"],
    ]);
    expect(screen.getByText("文件传输")).toBeInTheDocument();
    expect(screen.getByText("可触发拍摄")).toBeInTheDocument();
    expect(screen.getByText("自动收片")).toBeInTheDocument();
    expect(screen.getByTestId("device-dialog-capture")).toHaveTextContent("拍摄");
    expect(probeMock).toHaveBeenCalledWith("MTP_NIKON");
    expect(probeMock).toHaveBeenCalledTimes(1);
  });

  it("标准拍摄（standardCapture）单独成立也显示拍摄按钮；探测以设备 id 为 pnpId", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_STD", "Generic PTP Camera", "mtp", 3)],
      promptQueue: ["MTP_STD"],
    });
    probeMock.mockResolvedValue({
      pnpId: "MTP_STD",
      name: "Generic PTP Camera",
      capabilities: nikonCaps({ standardCapture: true, vendorCaptureNikon: false }),
    });

    renderDialog();

    expect(await screen.findByTestId("device-dialog-capture")).toBeInTheDocument();
    expect(
      screen.getAllByTestId("device-dialog-cap").find((c) => c.dataset.capability === "triggerCapture"),
    ).toHaveAttribute("data-supported", "true");
  });

  it("读卡器（volume）不显示联拍区、不探测", () => {
    useImportStore.setState({
      devices: [snapshot("E:", "SanDisk 64G", "volume", 12)],
      promptQueue: ["E:"],
    });

    renderDialog();

    expect(screen.getByRole("dialog")).toBeInTheDocument();
    expect(screen.queryByTestId("device-dialog-tethering")).not.toBeInTheDocument();
    expect(probeMock).not.toHaveBeenCalled();
  });

  it("探测在途/失败显示「未探测」，无拍摄入口", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_CAM", "EOS R5", "mtp", 5)],
      promptQueue: ["MTP_CAM"],
    });
    probeMock.mockReturnValue(new Promise(() => {})); // 在途不返回

    renderDialog();

    expect(await screen.findByTestId("device-dialog-cap-unprobed")).toHaveTextContent("未探测");
    expect(screen.queryByTestId("device-dialog-cap")).not.toBeInTheDocument();
    expect(screen.queryByTestId("device-dialog-capture")).not.toBeInTheDocument();
  });

  it("能力全 false：chips 全 unsupported 且无拍摄入口", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_CAM", "EOS R5", "mtp", 5)],
      promptQueue: ["MTP_CAM"],
    });
    probeMock.mockResolvedValue({
      pnpId: "MTP_CAM",
      name: "EOS R5",
      capabilities: nikonCaps({
        fileTransfer: false, standardCapture: false, vendorCaptureNikon: false, objectAddedEvents: false,
      }),
    });

    renderDialog();

    const chips = await screen.findAllByTestId("device-dialog-cap");
    expect(chips.every((c) => c.dataset.supported === "false")).toBe(true);
    expect(screen.queryByTestId("device-dialog-capture")).not.toBeInTheDocument();
  });

  it("拍摄流转：进行中禁用 → 成功 toast 已收片并刷新设备快照", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_NIKON", "Nikon D750", "mtp", 5)],
      promptQueue: ["MTP_NIKON"],
    });
    probeMock.mockResolvedValue({ pnpId: "MTP_NIKON", name: "Nikon D750", capabilities: nikonCaps() });
    let resolveCapture!: (value: CameraCaptureResult) => void;
    captureMock.mockReturnValue(
      new Promise<CameraCaptureResult>((resolve) => {
        resolveCapture = resolve;
      }),
    );
    // refreshDevice → device_scan 返回照片数 +1 的快照
    invokeMock.mockResolvedValue({
      ...snapshot("MTP_NIKON", "Nikon D750", "mtp", 6),
      filesByKind: { photo: 101, raw: 40, other: 1 },
    });

    renderDialog();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId("device-dialog-capture"));

    // 进行中态：按钮禁用 + 文案「拍摄中…」
    const busy = screen.getByTestId("device-dialog-capture");
    expect(busy).toBeDisabled();
    expect(busy).toHaveTextContent("拍摄中…");

    resolveCapture({ objectName: "DSC_0001.JPG", objectSize: 1024, error: null });

    // 成功态：toast + 快照刷新（照片计数 100 → 101）
    expect(await screen.findByText("已收片：DSC_0001.JPG")).toBeInTheDocument();
    expect(invokeMock).toHaveBeenCalledWith("device_scan", { id: "MTP_NIKON" });
    await waitFor(() => {
      expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("101");
    });
    const restored = screen.getByTestId("device-dialog-capture");
    expect(restored).toBeEnabled();
    expect(restored).toHaveTextContent("拍摄");
  });

  it("拍摄失败（error 非空）：toast 显示后端错误文案", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_NIKON", "Nikon D750", "mtp", 5)],
      promptQueue: ["MTP_NIKON"],
    });
    probeMock.mockResolvedValue({ pnpId: "MTP_NIKON", name: "Nikon D750", capabilities: nikonCaps() });
    captureMock.mockResolvedValue({ objectName: null, objectSize: null, error: "设备被资源管理器占用" });

    renderDialog();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId("device-dialog-capture"));

    expect(await screen.findByText("设备被资源管理器占用")).toBeInTheDocument();
    expect(invokeMock).not.toHaveBeenCalledWith("device_scan", { id: "MTP_NIKON" });
  });

  it("拍摄失败（error=null，invoke 不可用）：toast 显示通用失败文案", async () => {
    useImportStore.setState({
      devices: [snapshot("MTP_NIKON", "Nikon D750", "mtp", 5)],
      promptQueue: ["MTP_NIKON"],
    });
    probeMock.mockResolvedValue({ pnpId: "MTP_NIKON", name: "Nikon D750", capabilities: nikonCaps() });
    captureMock.mockResolvedValue({ objectName: null, objectSize: null, error: null });

    renderDialog();
    const user = userEvent.setup();
    await user.click(await screen.findByTestId("device-dialog-capture"));

    expect(await screen.findByText("拍摄失败，请重试")).toBeInTheDocument();
  });

  it("tetheringObjectAdded 命中当前设备（PnP 路径大小写归一）即刷新快照；其他设备不刷新", async () => {
    const pnpId = "\\\\?\\USB#VID_04B0&PID_0438#000#{6AC27778-...";
    useImportStore.setState({
      devices: [snapshot(pnpId.toLowerCase(), "Nikon D750", "mtp", 5)],
      promptQueue: [pnpId.toLowerCase()],
    });
    probeMock.mockResolvedValue({ pnpId, name: "Nikon D750", capabilities: nikonCaps() });
    invokeMock.mockResolvedValue({
      ...snapshot(pnpId.toLowerCase(), "Nikon D750", "mtp", 6),
      filesByKind: { photo: 102, raw: 40, other: 1 },
    });

    renderDialog();
    await screen.findByTestId("device-dialog-capture");

    // 其他设备的事件：不刷新
    emitAppEvent({ type: "tetheringObjectAdded", pnpId: "MTP_OTHER", objectName: "X.JPG", objectSize: 1 });
    expect(invokeMock).not.toHaveBeenCalled();

    // 命中当前设备：事件到达即刷新（照片 100 → 102）
    emitAppEvent({ type: "tetheringObjectAdded", pnpId, objectName: "DSC_0002.JPG", objectSize: 2048 });
    expect(invokeMock).toHaveBeenCalledWith("device_scan", { id: pnpId.toLowerCase() });
    await waitFor(() => {
      expect(screen.getByTestId("device-dialog-stats")).toHaveTextContent("102");
    });
  });
});
