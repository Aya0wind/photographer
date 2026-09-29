/** 联拍独立窗口：会话快照渲染 / 参数设置 / 拍摄 / 事件驱动胶片条 / 断连与结束态 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import {
  subscribeAppEvents,
  tetheringCapture,
  tetheringFocusAt,
  tetheringFrame,
  tetheringPhotoPreview,
  tetheringSession,
  tetheringSettingSet,
  type TetherSessionDto,
} from "@/ipc/api";
import type { CameraInfo, TetherCameraSetting } from "@/ipc/api";
import TetheringWindowPage from "./TetheringWindowPage";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    subscribeAppEvents: vi.fn(),
    tetheringSession: vi.fn(),
    tetheringSettings: vi.fn(),
    tetheringSettingSet: vi.fn(),
    tetheringFocusAt: vi.fn(),
    tetheringCapture: vi.fn(),
    tetheringFrame: vi.fn(),
    tetheringPhotoPreview: vi.fn(),
    tetheringStop: vi.fn(),
  };
});

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    close: vi.fn(),
    minimize: vi.fn(),
  })),
}));

const sessionMock = vi.mocked(tetheringSession);
const settingSetMock = vi.mocked(tetheringSettingSet);
const focusAtMock = vi.mocked(tetheringFocusAt);
const captureMock = vi.mocked(tetheringCapture);
const frameMock = vi.mocked(tetheringFrame);
const previewMock = vi.mocked(tetheringPhotoPreview);
const subscribeMock = vi.mocked(subscribeAppEvents);

function choice(id: string, current: string, writable = true, values: string[] = []): TetherCameraSetting {
  return {
    id,
    label: id,
    kind: "choice",
    current,
    writable,
    options: values.map((v) => ({ value: v, label: v })),
  };
}

const CAMERA: CameraInfo = {
  pnpId: "CAM_A",
  name: "Nikon D750",
  capabilities: {
    fileTransfer: true,
    standardCapture: true,
    vendorCaptureNikon: false,
    objectAddedEvents: true,
    liveView: true,
  },
};

function dto(overrides: Partial<TetherSessionDto> = {}): TetherSessionDto {
  return {
    id: "s1",
    libraryId: "lib-1",
    albumId: 7,
    albumName: "棚拍",
    camera: CAMERA,
    settings: [
      choice("shutterspeed", "125", true, ["125", "250"]),
      choice("iso", "400", false),
    ],
    photos: [],
    connected: true,
    receiving: false,
    error: null,
    ...overrides,
  };
}

let unsubscribe: (() => void) | null = null;
let handler: ((event: unknown) => void) | null = null;

function renderPage(sessionId = "s1") {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={[`/tethering?session=${sessionId}`]}>
        <TetheringWindowPage />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  handler = null;
  unsubscribe = vi.fn();
  subscribeMock.mockImplementation((fn) => {
    handler = fn as (event: unknown) => void;
    return Promise.resolve(unsubscribe!);
  });
  frameMock.mockResolvedValue(null);
  previewMock.mockResolvedValue(null);
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("TetheringWindowPage 联拍独立窗口", () => {
  it("会话快照渲染：标题栏相册名/相机名 + 参数面板（shutter/aperture/iso 本地化）", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-window")).toBeInTheDocument());
    expect(screen.getByTestId("tether-titlebar").textContent).toContain("棚拍");
    expect(screen.getByTestId("tether-titlebar").textContent).toContain("Nikon D750");
    expect(screen.getByTestId("tether-setting-shutterspeed")).toHaveTextContent("快门");
    expect(screen.getByTestId("tether-setting-iso")).toHaveTextContent("ISO");
    // live view 支持但还没帧：等待画面提示
    expect(screen.getByTestId("tether-view").textContent).toContain("等待取景画面");
  });

  it("实时取景：帧轮询填充画面（data URL）", async () => {
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue("data:image/jpeg;base64,FRAME");
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-view-img")).toHaveAttribute("src", "data:image/jpeg;base64,FRAME"));
  });

  it("帧率选择器：15/30/60 三档，点击切换 aria-pressed 并持久化 localStorage", async () => {
    window.localStorage.clear();
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue(null);
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-window")).toBeInTheDocument());
    expect(screen.getByTestId("tether-fps")).toHaveAttribute("role", "group");
    // 默认档：非法/缺失存储回退 30
    expect(screen.getByTestId("tether-fps-30")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("tether-fps-15")).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(screen.getByTestId("tether-fps-60"));
    expect(screen.getByTestId("tether-fps-60")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("tether-fps-30")).toHaveAttribute("aria-pressed", "false");
    expect(window.localStorage.getItem("tethering.frameFps")).toBe("60");
    window.localStorage.clear();
  });

  it("帧率选择器：不支持的相机（liveView=false）不渲染", async () => {
    sessionMock.mockResolvedValue(
      dto({ camera: { ...CAMERA, capabilities: { ...CAMERA.capabilities, liveView: false } } }),
    );
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-window")).toBeInTheDocument());
    expect(screen.queryByTestId("tether-fps")).toBeNull();
  });

  it("不支持 live view 的相机：回落最近一张成片预览", async () => {
    sessionMock.mockResolvedValue(
      dto({
        camera: { ...CAMERA, capabilities: { ...CAMERA.capabilities, liveView: false } },
        photos: [{ id: 9, name: "DSC_0001.JPG", kind: "photo" }],
      }),
    );
    previewMock.mockResolvedValue("data:image/jpeg;base64,THUMB");
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-view").textContent).toContain("不支持实时取景"));
    await waitFor(() => expect(screen.getByTestId("tether-view-img")).toHaveAttribute("src", "data:image/jpeg;base64,THUMB"));
    expect(previewMock).toHaveBeenCalledWith("s1", 9, 256);
  });

  it("改参数 → tetheringSettingSet；成功刷新面板；失败显示内联错误", async () => {
    sessionMock.mockResolvedValue(dto());
    const updated = dto({
      settings: [
        choice("shutterspeed", "250", true, ["125", "250"]),
        choice("iso", "400", false),
      ],
    });
    settingSetMock.mockResolvedValueOnce(updated.settings);
    renderPage();

    const select = (await screen.findByTestId("tether-setting-shutterspeed")).querySelector("select");
    expect(select).not.toBeNull();
    fireEvent.change(select!, { target: { value: "250" } });
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "shutterspeed", "250"));
    await waitFor(() => expect((screen.getByTestId("tether-setting-shutterspeed").querySelector("select") as HTMLSelectElement).value).toBe("250"));

    settingSetMock.mockRejectedValueOnce(new Error("无效拍摄参数"));
    fireEvent.change(screen.getByTestId("tether-setting-shutterspeed").querySelector("select")!, { target: { value: "125" } });
    await waitFor(() =>
      expect(screen.getByTestId("tether-setting-shutterspeed").textContent).toContain("设置失败"),
    );
  });

  it("全参数面板：range 滑条松手才提交、action 按钮直接触发、未知 id 显示后端 label", async () => {
    const settings: TetherCameraSetting[] = [
      { id: "manualfocus", label: "Manual-Focus", kind: "range", current: "0", writable: true, options: [], min: -7, max: 7, step: 1 },
      { id: "autofocus", label: "Autofocus", kind: "action", current: "2", writable: true, options: [] },
      choice("exposuremetermode", "Multi", true, ["Multi", "Spot Standard"]),
    ];
    sessionMock.mockResolvedValue(dto({ settings }));
    settingSetMock.mockResolvedValue(settings);
    renderPage();

    const range = await screen.findByTestId("tether-setting-manualfocus");
    const slider = range.querySelector("input[type=range]") as HTMLInputElement;
    expect(slider).not.toBeNull();
    expect(slider.getAttribute("min")).toBe("-7");
    // 拖动只入草稿，不下发
    fireEvent.input(slider, { target: { value: "3" } });
    expect(range.textContent).toContain("3");
    expect(settingSetMock).not.toHaveBeenCalled();
    // 松手提交
    fireEvent.pointerUp(slider);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "manualfocus", "3"));

    // action：本地化标签 + 点击即触发
    const action = screen.getByTestId("tether-setting-autofocus");
    expect(action).toHaveTextContent("自动对焦");
    fireEvent.click(action);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "autofocus", "1"));

    // choice 正常渲染
    expect(screen.getByTestId("tether-setting-exposuremetermode")).toHaveTextContent("测光模式");
  });

  it("点击取景画面对焦：归一化坐标下发 + 十字标记出现", async () => {
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue("data:image/jpeg;base64,FRAME");
    focusAtMock.mockResolvedValue(undefined);
    renderPage();

    await screen.findByTestId("tether-view-img");
    const view = screen.getByTestId("tether-view");
    // jsdom 无布局：直接伪造 img 的 getBoundingClientRect
    const img = view.querySelector("img") as HTMLImageElement;
    img.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 200, height: 100, right: 200, bottom: 100, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    fireEvent.click(view, { clientX: 100, clientY: 50 });
    await waitFor(() => expect(focusAtMock).toHaveBeenCalledWith("s1", 0.5, 0.5));
    expect(screen.getByTestId("tether-focus-marker")).toBeInTheDocument();
  });

  it("按快门 → tetheringCapture；失败显示浮层错误", async () => {
    sessionMock.mockResolvedValue(dto());
    captureMock.mockResolvedValueOnce({ ok: true }).mockResolvedValueOnce({
      ok: false,
      error: "相机无响应",
    });
    renderPage();
    const shutter = await screen.findByTestId("tether-shutter");
    fireEvent.click(shutter);
    await waitFor(() => expect(captureMock).toHaveBeenCalledWith("s1"));

    fireEvent.click(screen.getByTestId("tether-shutter"));
    await waitFor(() => expect(screen.getByTestId("tether-capture-error")).toHaveTextContent("相机无响应"));
  });

  it("tetheringPhotoAdded 事件：胶片条追加新片", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();
    await screen.findByTestId("tether-window");

    expect(handler).not.toBeNull();
    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.JPG",
    });
    expect(await screen.findByTestId("tether-film-42")).toBeInTheDocument();
  });

  it("tetheringStatus 事件：断连横幅 + 快门禁用", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();
    await screen.findByTestId("tether-window");

    handler!({ type: "tetheringStatus", sessionId: "s1", connected: false, error: null });
    await waitFor(() => expect(screen.getByTestId("tether-disconnect-banner")).toBeInTheDocument());
    expect(screen.getByTestId("tether-shutter")).toBeDisabled();
  });

  it("会话已结束（快照 null）：结束态 + 关窗按钮", async () => {
    sessionMock.mockResolvedValue(null);
    renderPage();

    expect(await screen.findByTestId("tether-ended")).toHaveTextContent("拍摄会话已结束");
    expect(screen.getByTestId("tether-ended-close")).toBeInTheDocument();
  });
});
