/** 联拍独立窗口：会话快照渲染 / 参数设置 / 拍摄 / 事件驱动胶片条 / 断连与结束态 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import {
  assetsByIds,
  assetDetail,
  assetVersions,
  editRecipeGet,
  subscribeAppEvents,
  tetheringCapture,
  tetheringFocusAt,
  tetheringFrame,
  tetheringPhotoPreview,
  tetheringSession,
  tetheringSettingSet,
  tetheringSettings,
  type TetherCaptureResult,
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
    // 点击胶片条 → 查看器（复用画廊 ViewerOverlay）所需的 API 面
    assetsByIds: vi.fn(),
    assetDetail: vi.fn(),
    assetVersions: vi.fn(),
    editRecipeGet: vi.fn(),
    assetThumbGet: vi.fn(),
    assetFlagSet: vi.fn(),
    assetRatingSet: vi.fn(),
    assetLabelSet: vi.fn(),
    assetRejectSet: vi.fn(),
    clipboardCopyFiles: vi.fn(),
    revealInExplorer: vi.fn(),
  };
});

vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: vi.fn(() => ({
    close: vi.fn(),
    minimize: vi.fn(),
  })),
}));

const sessionMock = vi.mocked(tetheringSession);
const settingsRefreshMock = vi.mocked(tetheringSettings);
const settingSetMock = vi.mocked(tetheringSettingSet);
const focusAtMock = vi.mocked(tetheringFocusAt);
const captureMock = vi.mocked(tetheringCapture);
const frameMock = vi.mocked(tetheringFrame);
const previewMock = vi.mocked(tetheringPhotoPreview);
const subscribeMock = vi.mocked(subscribeAppEvents);
const assetsByIdsMock = vi.mocked(assetsByIds);
const assetDetailMock = vi.mocked(assetDetail);
const assetVersionsMock = vi.mocked(assetVersions);
const editRecipeMock = vi.mocked(editRecipeGet);

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
      choice("f-number", "2.8", false),
    ],
    photos: [],
    connected: true,
    receiving: false,
    error: null,
    pendingReceipts: 0,
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

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

beforeEach(() => {
  vi.clearAllMocks();
  handler = null;
  unsubscribe = vi.fn();
  // 轮询默认无返回（不影响其他测试的静态快照）
  settingsRefreshMock.mockResolvedValue(null);
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
  it("旧刷新回包不能覆盖正在修改的值；改参期间暂停取景和拍摄", async () => {
    sessionMock.mockResolvedValue(dto());
    const oldRefresh = deferred<TetherCameraSetting[] | null>();
    const write = deferred<TetherCameraSetting[]>();
    settingsRefreshMock.mockReturnValueOnce(oldRefresh.promise);
    settingSetMock.mockReturnValueOnce(write.promise);
    renderPage();
    await screen.findByTestId("tether-window");
    handler!({ type: "tetheringSettingsChanged", sessionId: "s1" });
    await waitFor(() => expect(settingsRefreshMock).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByTestId("tether-setting-shutterspeed"));
    fireEvent.click(screen.getByTestId("tether-quick-shutterspeed-menu").querySelector('[data-value="250"]')!);
    const frameCalls = frameMock.mock.calls.length;
    expect(screen.getByTestId("tether-shutter")).toBeDisabled();
    expect(screen.getByTestId("tether-setting-shutterspeed")).toHaveAttribute("aria-busy", "true");
    await act(async () => {
      oldRefresh.resolve(dto().settings);
      await new Promise((done) => window.setTimeout(done, 100));
    });
    expect(screen.getByTestId("tether-setting-shutterspeed")).toHaveTextContent("250");
    expect(frameMock).toHaveBeenCalledTimes(frameCalls);
    await act(async () => write.resolve([choice("shutterspeed", "250", true, ["125", "250"])]));
    expect(screen.getByTestId("tether-shutter")).toBeEnabled();
  });

  it("快速改同一参数只发送当前在途值和最后一个待发送值", async () => {
    const iso = (value: string) => choice("iso", value, true, ["100", "200", "400", "800"]);
    sessionMock.mockResolvedValue(dto({ settings: [iso("100")] }));
    const first = deferred<TetherCameraSetting[]>();
    settingSetMock.mockReturnValueOnce(first.promise).mockResolvedValueOnce([iso("800")]);
    renderPage();
    await screen.findByTestId("tether-setting-iso");
    for (const value of ["200", "400", "800"]) {
      fireEvent.click(screen.getByTestId("tether-setting-iso"));
      fireEvent.click(screen.getByTestId("tether-quick-iso-menu").querySelector(`[data-value="${value}"]`)!);
    }
    expect(settingSetMock).toHaveBeenCalledTimes(1);
    expect(screen.getByTestId("tether-setting-iso")).toHaveTextContent("800");
    await act(async () => first.resolve([iso("200")]));
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledTimes(2));
    expect(settingSetMock.mock.calls.map((call) => call[2])).toEqual(["200", "800"]);
    await waitFor(() => expect(screen.getByTestId("tether-setting-iso")).toHaveAttribute("aria-busy", "false"));
    expect(screen.getByTestId("tether-setting-iso")).toHaveTextContent("800");
  });

  it("变化通知合并且刷新在途时不叠加新读取", async () => {
    sessionMock.mockResolvedValue(dto());
    const read = deferred<TetherCameraSetting[] | null>();
    settingsRefreshMock.mockReturnValue(read.promise);
    renderPage();
    await screen.findByTestId("tether-window");
    for (let i = 0; i < 10; i++) handler!({ type: "tetheringSettingsChanged", sessionId: "s1" });
    await waitFor(() => expect(settingsRefreshMock).toHaveBeenCalledTimes(1));
    handler!({ type: "tetheringSettingsChanged", sessionId: "s1" });
    await act(async () => { await new Promise((done) => window.setTimeout(done, 300)); });
    expect(settingsRefreshMock).toHaveBeenCalledTimes(1);
    await act(async () => read.resolve(dto().settings));
  });

  it("连续点击快门只触发一次，失败后可以再次拍摄", async () => {
    sessionMock.mockResolvedValue(dto());
    const capture = deferred<TetherCaptureResult>();
    captureMock.mockReturnValueOnce(capture.promise).mockResolvedValueOnce({ ok: true });
    renderPage();
    const shutter = await screen.findByTestId("tether-shutter");
    act(() => { fireEvent.click(shutter); fireEvent.click(shutter); fireEvent.click(shutter); });
    expect(captureMock).toHaveBeenCalledTimes(1);
    await act(async () => capture.resolve({ ok: false, error: "相机正忙" }));
    expect(screen.getByTestId("tether-shutter")).toBeEnabled();
    fireEvent.click(screen.getByTestId("tether-shutter"));
    await waitFor(() => expect(captureMock).toHaveBeenCalledTimes(2));
  });

  it("拍摄请求异常也会恢复按钮，不会永久停在拍摄中", async () => {
    sessionMock.mockResolvedValue(dto());
    captureMock.mockRejectedValueOnce(new Error("USB unavailable"));
    renderPage();
    fireEvent.click(await screen.findByTestId("tether-shutter"));
    await waitFor(() => expect(screen.getByTestId("tether-capture-error")).toHaveTextContent("USB unavailable"));
    expect(screen.getByTestId("tether-shutter")).toBeEnabled();
    expect(screen.queryByTestId("tether-capturing")).toBeNull();
  });
  it("会话快照渲染：标题栏相册名/相机名 + 悬浮工具栏（快门本地化）", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-window")).toBeInTheDocument());
    expect(screen.getByTestId("tether-titlebar").textContent).toContain("棚拍");
    expect(screen.getByTestId("tether-titlebar").textContent).toContain("Nikon D750");
    expect(screen.getByTestId("tether-setting-shutterspeed")).toHaveTextContent("快门");
    // live view 支持但还没帧：等待画面提示
    expect(screen.getByTestId("tether-view").textContent).toContain("等待取景画面");
  });

  it("悬浮工具栏：只读参数隐藏（ISO），光圈例外保留且禁用；拍摄按钮常驻", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();

    await screen.findByTestId("tether-quickbar");
    // ISO 只读 → 不渲染（不能调的参数不占位）
    expect(screen.queryByTestId("tether-setting-iso")).toBeNull();
    // 光圈只读 → 例外保留（镜头环控制的机身要看到当前值），按钮禁用
    const aperture = screen.getByTestId("tether-setting-f-number");
    expect(aperture).toHaveTextContent("光圈");
    expect(aperture).toHaveTextContent("2.8");
    expect(aperture).toBeDisabled();
    // 快门常驻工具栏（无需翻面板）
    expect(screen.getByTestId("tether-setting-shutterspeed")).toBeInTheDocument();
    expect(screen.getByTestId("tether-shutter")).toBeInTheDocument();
  });

  it("只读拍摄模式显示机身转盘提示，不提交无效修改", async () => {
    sessionMock.mockResolvedValue(
      dto({ settings: [choice("expprogram", "P", false, ["P", "A", "S", "M"]), choice("iso", "400", false)] }),
    );
    settingSetMock.mockResolvedValue([choice("expprogram", "M", true, ["P", "A", "S", "M"])]);
    renderPage();
    const mode = await screen.findByTestId("tether-setting-expprogram");
    expect(mode).toHaveTextContent("拍摄模式");
    expect(mode).toHaveTextContent("P");
    expect(mode).toBeDisabled();
    expect(mode).toHaveAttribute("title", i18n.t("tether.modeDialHint"));
    fireEvent.click(mode);
    expect(screen.queryByTestId("tether-quick-expprogram-menu")).toBeNull();
    expect(settingSetMock).not.toHaveBeenCalled();
    // 其他只读参数（ISO）依旧隐藏
    expect(screen.queryByTestId("tether-setting-iso")).toBeNull();
  });

  it("拍摄模式可写（支持远程切换的机型）：弹层直接切档", async () => {
    sessionMock.mockResolvedValue(
      dto({ settings: [choice("expprogram", "P", true, ["P", "A", "S", "M"])] }),
    );
    settingSetMock.mockResolvedValue([choice("expprogram", "M", true, ["P", "A", "S", "M"])]);
    renderPage();
    const mode = await screen.findByTestId("tether-setting-expprogram");
    fireEvent.click(mode);
    fireEvent.click(screen.getByTestId("tether-quick-expprogram-menu").querySelector('[data-value="M"]')!);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "expprogram", "M"));
  });

  it("模式、三要素和快门保留浮条，其他参数和自动对焦在右侧", async () => {
    sessionMock.mockResolvedValue(dto({ settings: [
      choice("expprogram", "M", true, ["P", "A", "S", "M"]),
      choice("shutterspeed", "125", true, ["125", "250"]), choice("f-number", "2.8", true, ["2.8", "4"]), choice("iso", "100", true, ["100", "200"]),
      choice("whitebalance", "Auto", true), choice("focusmode", "AF-S", true),
    ] }));
    renderPage();
    const bar = await screen.findByTestId("tether-quickbar");
    const panel = screen.getByTestId("tether-settings-panel");
    for (const id of ["expprogram", "shutterspeed", "f-number", "iso"]) {
      expect(bar).toContainElement(screen.getByTestId(`tether-setting-${id}`));
      expect(panel).not.toContainElement(screen.getByTestId(`tether-setting-${id}`));
    }
    for (const id of ["whitebalance", "focusmode"]) {
      expect(panel).toContainElement(screen.getByTestId(`tether-setting-${id}`));
    }
    expect(panel).toContainElement(screen.getByTestId("tether-af"));
    expect(bar).toContainElement(screen.getByTestId("tether-shutter"));
  });

  it("取景铺满可用区域后，黑边不触发对焦且图片坐标正确", async () => {
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue({ url: "data:image/jpeg;base64,FRAME", af: null });
    focusAtMock.mockResolvedValue(undefined);
    renderPage();
    const img = await screen.findByTestId("tether-view-img");
    const view = screen.getByTestId("tether-view");
    view.getBoundingClientRect = () => ({ left: 0, top: 0, width: 400, height: 400 }) as DOMRect;
    img.getBoundingClientRect = view.getBoundingClientRect;
    Object.defineProperties(img, { naturalWidth: { value: 600 }, naturalHeight: { value: 400 } });
    fireEvent.load(img);
    fireEvent.click(view, { clientX: 200, clientY: 20 });
    expect(focusAtMock).not.toHaveBeenCalled();
    fireEvent.click(view, { clientX: 300, clientY: 200 });
    await waitFor(() => expect(focusAtMock).toHaveBeenCalledWith("s1", 0.75, 0.5));
  });

  it("实时取景：帧轮询填充画面（data URL）", async () => {
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue({ url: "data:image/jpeg;base64,FRAME", af: null });
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-view-img")).toHaveAttribute("src", "data:image/jpeg;base64,FRAME"));
  });

  it("轮询节奏按 tick 起点对齐：链路耗时计入帧周期，不叠加在间隔之上", async () => {
    sessionMock.mockResolvedValue(dto());
    // 显式 30fps 档（默认已是 15fps，间隔 67ms 会吞掉本测试的延迟差）：
    // 间隔 33ms；帧回包延迟 40ms > 间隔 → 下一帧应立即发出（≈40ms/帧）。
    // 若回包后再排 33ms，实际周期 73ms → 200ms 只够 ~2.7 帧。下界 4 帧
    // 区分两种节奏。
    window.localStorage.setItem("tethering.frameFps", "30");
    frameMock.mockImplementation(async () => {
      await new Promise((done) => window.setTimeout(done, 40));
      return { url: "data:image/jpeg;base64,FRAME", af: null };
    });
    renderPage();
    await screen.findByTestId("tether-window");
    await new Promise((done) => window.setTimeout(done, 200));
    expect(frameMock.mock.calls.length).toBeGreaterThanOrEqual(4);
    window.localStorage.removeItem("tethering.frameFps");
  });

  it("帧率选择器：15/30/60 三档，点击切换 aria-pressed 并持久化 localStorage", async () => {
    window.localStorage.clear();
    sessionMock.mockResolvedValue(dto());
    frameMock.mockResolvedValue(null);
    renderPage();

    await waitFor(() => expect(screen.getByTestId("tether-window")).toBeInTheDocument());
    expect(screen.getByTestId("tether-fps")).toHaveAttribute("role", "group");
    // 默认档：非法/缺失存储回退 15
    expect(screen.getByTestId("tether-fps-15")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("tether-fps-30")).toHaveAttribute("aria-pressed", "false");

    fireEvent.click(screen.getByTestId("tether-fps-60"));
    expect(screen.getByTestId("tether-fps-60")).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByTestId("tether-fps-15")).toHaveAttribute("aria-pressed", "false");
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

  it("工具栏改参数：弹层选值 → tetheringSettingSet；失败浮条下提示错误", async () => {
    sessionMock.mockResolvedValue(dto());
    const updated = dto({
      settings: [
        choice("shutterspeed", "250", true, ["125", "250"]),
        choice("iso", "400", false),
        choice("f-number", "2.8", false),
      ],
    });
    settingSetMock.mockResolvedValueOnce(updated.settings);
    renderPage();

    // 展开快门弹层 → 点 250
    fireEvent.click(await screen.findByTestId("tether-setting-shutterspeed"));
    const menu = await screen.findByTestId("tether-quick-shutterspeed-menu");
    expect(menu).toBeInTheDocument();
    const opt = menu.querySelector('[data-value="250"]') as HTMLElement;
    fireEvent.click(opt);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "shutterspeed", "250"));
    // 成功后按钮值刷新、弹层收起
    await waitFor(() => expect(screen.getByTestId("tether-setting-shutterspeed")).toHaveTextContent("250"));
    expect(screen.queryByTestId("tether-quick-shutterspeed-menu")).toBeNull();

    // 失败：错误显示在浮条下方
    settingSetMock.mockRejectedValueOnce(new Error("无效拍摄参数"));
    fireEvent.click(screen.getByTestId("tether-setting-shutterspeed"));
    fireEvent.click(screen.getByTestId("tether-quick-shutterspeed-menu").querySelector('[data-value="125"]')!);
    await waitFor(() => expect(screen.getByTestId("tether-quick-error")).toHaveTextContent("设置失败"));
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
    frameMock.mockResolvedValue({ url: "data:image/jpeg;base64,FRAME", af: null });
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

  it("按快门 → tetheringCapture；失败显示浮层错误", async () => {    sessionMock.mockResolvedValue(dto());
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

  it("拍摄完成但未收到事件：根据后端快照显示新片和缩略图", async () => {
    sessionMock.mockResolvedValueOnce(dto()).mockResolvedValue(dto({
      photos: [{ id: 42, name: "DSC_0002.JPG", kind: "photo" }],
    }));
    captureMock.mockResolvedValue({ ok: true });
    previewMock.mockResolvedValue("data:image/jpeg;base64,THUMB");
    renderPage();
    fireEvent.click(await screen.findByTestId("tether-shutter"));
    const film = await screen.findByTestId("tether-film-42");
    await waitFor(() => expect(film.querySelector("img")).toHaveAttribute("src", "data:image/jpeg;base64,THUMB"));
    expect(screen.queryByText(i18n.t("tether.noPhotos"))).toBeNull();
  });

  it("tetheringPhotoAdded 事件：胶片条追加新片，重复通知不重复添加", async () => {
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
      kind: "photo",
    });
    expect(await screen.findByTestId("tether-film-42")).toBeInTheDocument();
    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.JPG",
      kind: "photo",
    });
    await waitFor(() => expect(screen.getAllByTestId("tether-film-42")).toHaveLength(1));
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

  it("改参成功后联动全量刷新（参数间可写性/取值联动）", async () => {
    sessionMock.mockResolvedValue(dto());
    settingSetMock.mockResolvedValue([choice("shutterspeed", "250", true, ["125", "250"])]);
    settingsRefreshMock.mockClear();
    renderPage();
    fireEvent.click(await screen.findByTestId("tether-setting-shutterspeed"));
    fireEvent.click(screen.getByTestId("tether-quick-shutterspeed-menu").querySelector('[data-value="250"]')!);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "shutterspeed", "250"));
    // set 返回后 400ms 触发一次全量快照刷新
    await waitFor(() => expect(settingsRefreshMock).toHaveBeenCalledWith("s1"), { timeout: 2000 });
  });

  it("AF 按钮触发画面中心对焦", async () => {
    sessionMock.mockResolvedValue(dto());
    focusAtMock.mockResolvedValue(undefined);
    renderPage();

    fireEvent.click(await screen.findByTestId("tether-af"));
    await waitFor(() => expect(focusAtMock).toHaveBeenCalledWith("s1", 0.5, 0.5));
  });

  it("A 档快门只读隐藏 → 切回 M 档：settings 轮询自动恢复快门", async () => {
    window.localStorage.clear();
    // A 档快照：快门由机身控制（只读 → 隐藏），光圈只读例外保留
    sessionMock.mockResolvedValue(
      dto({ settings: [choice("shutterspeed", "125", false), choice("f-number", "2.8", false)] }),
    );
    renderPage();
    await screen.findByTestId("tether-quickbar");
    expect(screen.queryByTestId("tether-setting-shutterspeed")).toBeNull();
    expect(screen.getByTestId("tether-setting-f-number")).toBeInTheDocument();

    // 轮询返回 M 档快照：快门可写 → 回到工具栏
    settingsRefreshMock.mockResolvedValue([
      choice("shutterspeed", "125", true, ["125", "250"]),
      choice("f-number", "2.8", true, ["2.8", "4"]),
    ]);
    await waitFor(
      () => expect(screen.getByTestId("tether-setting-shutterspeed")).toBeInTheDocument(),
      { timeout: 6000 },
    );
  }, 12000);

  it("工具栏默认居中，旧拖动位置不影响新窗口，当前窗口仍可拖动", async () => {
    window.localStorage.setItem("tethering.quickbar.pos", JSON.stringify({ x: 700, y: 500 }));
    sessionMock.mockResolvedValue(dto());
    renderPage();
    const grip = await screen.findByTestId("tether-quickbar-grip");
    expect(screen.getByTestId("tether-quickbar")).toHaveClass("bottom-3", "left-1/2", "-translate-x-1/2");

    fireEvent.pointerDown(grip, { button: 0, clientX: 400, clientY: 500 });
    fireEvent.pointerMove(window, { clientX: 320, clientY: 430 });
    fireEvent.pointerUp(window);

    const bar = screen.getByTestId("tether-quickbar");
    expect(bar.style.left).not.toBe("");
    window.localStorage.clear();
  });

  it("拖动后缩小再放大窗口，工具条保持可见", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();
    const bar = await screen.findByTestId("tether-quickbar");
    const view = screen.getByTestId("tether-view");
    Object.defineProperties(bar, { offsetWidth: { value: 300 }, offsetHeight: { value: 50 } });
    view.getBoundingClientRect = () => ({ left: 0, top: 0, width: 1000, height: 700 }) as DOMRect;
    fireEvent.pointerDown(screen.getByTestId("tether-quickbar-grip"), { button: 0, clientX: 0, clientY: 0 });
    fireEvent.pointerMove(window, { clientX: 700, clientY: 500 });
    fireEvent.pointerUp(window);
    view.getBoundingClientRect = () => ({ width: 500, height: 300 }) as DOMRect;
    fireEvent(window, new Event("resize"));
    await waitFor(() => {
      expect(bar.style.left).toBe("200px");
      expect(bar.style.top).toBe("250px");
    });
    view.getBoundingClientRect = () => ({ width: 1000, height: 700 }) as DOMRect;
    fireEvent(window, new Event("resize"));
    expect(bar.style.left).toBe("200px");
    expect(bar.style.top).toBe("250px");
    window.localStorage.clear();
  });

  it("Range 型参数（无 options + 嵌套 range）：渲染滑条、拖动入草稿、松手提交", async () => {
    // 后端新契约：ISO 无离散候选时带 range { min, max, step }
    const settings: TetherCameraSetting[] = [
      {
        id: "iso",
        label: "ISO",
        kind: "choice",
        current: "400",
        writable: true,
        options: [],
        range: { min: 100, max: 6400, step: 100 },
      },
    ];
    sessionMock.mockResolvedValue(dto({ settings }));
    settingSetMock.mockResolvedValue(settings);
    renderPage();

    const row = await screen.findByTestId("tether-setting-iso");
    const slider = row.querySelector("input[type=range]") as HTMLInputElement;
    expect(slider).not.toBeNull();
    expect(slider.getAttribute("min")).toBe("100");
    expect(slider.getAttribute("max")).toBe("6400");
    expect(slider.getAttribute("step")).toBe("100");
    expect(slider).toBeEnabled();
    expect(slider).toHaveAttribute("aria-label", i18n.t("tether.param.iso"));
    // 无候选的 ISO 不进悬浮工具栏（只在右栏），值显示当前值
    expect(row).toHaveTextContent("400");

    // 拖动只更新草稿数值显示，不下发
    fireEvent.input(slider, { target: { value: "800" } });
    expect(screen.getByTestId("tether-range-value-iso")).toHaveTextContent("800");
    expect(settingSetMock).not.toHaveBeenCalled();
    // 松手提交归一范围内的数字字符串
    fireEvent.pointerUp(slider);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "iso", "800"));
  });

  it("相机无远程拍摄能力位：快门禁用 + 原因 tooltip，点击不触发 tetheringCapture", async () => {
    sessionMock.mockResolvedValue(
      dto({
        camera: { ...CAMERA, capabilities: { ...CAMERA.capabilities, standardCapture: false, vendorCaptureNikon: false } },
      }),
    );
    renderPage();

    const shutter = await screen.findByTestId("tether-shutter");
    expect(shutter).toBeDisabled();
    expect(shutter).toHaveAttribute("title", i18n.t("tether.captureUnsupported"));
    fireEvent.click(shutter);
    await new Promise((done) => window.setTimeout(done, 50));
    expect(captureMock).not.toHaveBeenCalled();
  });

  it("无 live view 能力：AF 禁用带原因、取景容器 tooltip、点击画面不发送对焦坐标", async () => {
    sessionMock.mockResolvedValue(
      dto({
        camera: { ...CAMERA, capabilities: { ...CAMERA.capabilities, liveView: false } },
        photos: [{ id: 9, name: "DSC_0001.JPG", kind: "photo" }],
        settings: [choice("iso", "400", true)],
      }),
    );
    previewMock.mockResolvedValue("data:image/jpeg;base64,THUMB");
    renderPage();

    // AF 依赖 live view：禁用 + tooltip 说明；点击不触发中心对焦
    const af = await screen.findByTestId("tether-af");
    expect(af).toBeDisabled();
    expect(af).toHaveAttribute("title", i18n.t("tether.focusNeedsLiveView"));
    fireEvent.click(af);
    expect(focusAtMock).not.toHaveBeenCalled();

    // 取景不可用的原因 tooltip 挂在取景容器上
    const view = screen.getByTestId("tether-view");
    expect(view).toHaveAttribute("title", i18n.t("tether.liveViewUnsupported"));

    // 回落成片预览上点击：不发送对焦坐标（无 live view 无点对焦）
    const img = await waitFor(() => {
      const el = view.querySelector("img") as HTMLImageElement | null;
      expect(el).not.toBeNull();
      return el!;
    });
    img.getBoundingClientRect = () =>
      ({ left: 0, top: 0, width: 200, height: 100, right: 200, bottom: 100, x: 0, y: 0, toJSON: () => ({}) }) as DOMRect;
    fireEvent.click(view, { clientX: 50, clientY: 25 });
    await new Promise((done) => window.setTimeout(done, 50));
    expect(focusAtMock).not.toHaveBeenCalled();
  });

  it("待收轻提示：快门成功后显示「待收 N 张」，photoAdded 到账后消失", async () => {
    sessionMock.mockResolvedValue(dto());
    captureMock.mockResolvedValue({ ok: true });
    renderPage();

    fireEvent.click(await screen.findByTestId("tether-shutter"));
    // 触发成功但收片未到（快照 photos 空、无 photoAdded 事件）
    expect(await screen.findByTestId("tether-pending-receipts")).toHaveTextContent("待收 1 张");

    // 收片到账（事件路径）：计数冲抵、轻提示消失
    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.JPG",
      kind: "photo",
    });
    await waitFor(() => expect(screen.queryByTestId("tether-pending-receipts")).toBeNull());
  });

  it("待收轻提示：收片遇错转为「重试中」，到账后消失", async () => {
    sessionMock.mockResolvedValue(dto());
    captureMock.mockResolvedValue({ ok: true });
    renderPage();

    fireEvent.click(await screen.findByTestId("tether-shutter"));
    expect(await screen.findByTestId("tether-pending-receipts")).toHaveTextContent("待收 1 张");

    // 会话报错（USB 抖动等）：待收中的收片转为重试中提示
    handler!({ type: "tetheringStatus", sessionId: "s1", connected: true, error: "USB 传输不稳" });
    await waitFor(() => expect(screen.getByTestId("tether-pending-receipts")).toHaveTextContent(i18n.t("tether.receivingRetry")));

    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.JPG",
      kind: "photo",
    });
    await waitFor(() => expect(screen.queryByTestId("tether-pending-receipts")).toBeNull());
  });

  it("待收计数与拍摄后快照对账：快照已含新片时不残留待收提示", async () => {
    // 拍摄触发成功且收片极快：photoAdded 事件丢失，靠快照新片冲抵
    sessionMock.mockResolvedValueOnce(dto()).mockResolvedValue(dto({
      photos: [{ id: 42, name: "DSC_0002.JPG", kind: "photo" }],
    }));
    captureMock.mockResolvedValue({ ok: true });
    renderPage();

    fireEvent.click(await screen.findByTestId("tether-shutter"));
    await screen.findByTestId("tether-film-42");
    // 快照新片已到账：不显示待收提示（事件丢失由对账兜底）
    await waitFor(() => expect(screen.queryByTestId("tether-pending-receipts")).toBeNull());
  });

  it("胶片条合并卡：RAW+JPG 同 stem 合一张卡（代表=JPG）+ 格式角标", async () => {
    sessionMock.mockResolvedValue(dto({
      photos: [
        { id: 9, name: "DSC_0001.JPG", kind: "photo" },
        { id: 10, name: "DSC_0001.NEF", kind: "raw" },
        { id: 11, name: "DSC_0002.NEF", kind: "raw" },
        { id: 12, name: "DSC_0003.JPG", kind: "photo" },
      ],
    }));
    renderPage();

    // RAW+JPG 合并：一张卡（代表 JPG id9），RAW 成员不再单独出卡
    expect(await screen.findByTestId("tether-film-9")).toBeInTheDocument();
    expect(screen.queryByTestId("tether-film-10")).toBeNull();
    expect(screen.getByTestId("tether-film-badge-9")).toHaveTextContent("RAW+JPG");
    // 单 RAW / 单 JPG 如实标注
    expect(screen.getByTestId("tether-film-badge-11")).toHaveTextContent("RAW");
    expect(screen.getByTestId("tether-film-badge-12")).toHaveTextContent("JPG");
    // 合并卡 tooltip 列出两个成员文件名
    expect(screen.getByTestId("tether-film-9")).toHaveAttribute("title", "DSC_0001.JPG + DSC_0001.NEF");
  });

  it("photoAdded 事件携带 kind：RAW 新片到账即标 RAW（不等快照纠正）", async () => {
    sessionMock.mockResolvedValue(dto());
    renderPage();
    await screen.findByTestId("tether-window");

    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.NEF",
      kind: "raw",
    });
    expect(await screen.findByTestId("tether-film-42")).toBeInTheDocument();
    expect(screen.getByTestId("tether-film-badge-42")).toHaveTextContent("RAW");
  });

  it("点击胶片条卡片 → 全屏查看器（会话照片为结果集，代表卡打开）", async () => {
    sessionMock.mockResolvedValue(dto({
      photos: [
        { id: 9, name: "DSC_0001.JPG", kind: "photo" },
        { id: 10, name: "DSC_0001.NEF", kind: "raw" },
      ],
    }));
    const { assetFixture } = await import("@/test/fixtures");
    assetsByIdsMock.mockResolvedValue([
      assetFixture(9, { name: "DSC_0001.JPG" }),
      assetFixture(10, { name: "DSC_0001.NEF", kind: "raw" }),
    ]);
    assetDetailMock.mockResolvedValue(null);
    assetVersionsMock.mockResolvedValue(null);
    editRecipeMock.mockResolvedValue({ recipe: null, updatedAt: null });
    renderPage();

    fireEvent.click(await screen.findByTestId("tether-film-9"));
    // 打开时按会话照片全量拉取（查看器内左右切换 = 会话集）
    await waitFor(() => expect(assetsByIdsMock).toHaveBeenCalledWith([9, 10]));
      const viewer = await screen.findByTestId("viewer");
    expect(viewer).toHaveAttribute("aria-label", "DSC_0001.JPG");
  });

  it("待收提示用后端 FIFO 真值：快照 pendingReceipts 直接显示（RAW+JPG 第二张在途不失真）", async () => {
    sessionMock.mockResolvedValue(dto({ pendingReceipts: 2 }));
    renderPage();

    // 无本地快门计数，快照真值即显示（RAW 已到、JPG 在途 = 待收 2）
    expect(await screen.findByTestId("tether-pending-receipts")).toHaveTextContent("待收 2 张");
  });

  it("photoAdded 事件后去抖重拉快照，pendingReceipts 归零后提示消失", async () => {
    sessionMock.mockResolvedValue(dto({ pendingReceipts: 1 }));
    renderPage();
    expect(await screen.findByTestId("tether-pending-receipts")).toBeInTheDocument();

    const callsBefore = sessionMock.mock.calls.length;
    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.NEF",
      kind: "raw",
    });
    // 事件增量先落胶片条；去抖重拉前把快照真值切到「队列已清空」，
    // 400ms 的重拉带着 pendingReceipts=0 到达 → 提示消失
    sessionMock.mockResolvedValue(dto({ photos: [{ id: 42, name: "DSC_0002.NEF", kind: "raw" }], pendingReceipts: 0 }));
    await waitFor(() => expect(sessionMock.mock.calls.length).toBeGreaterThan(callsBefore), { timeout: 2000 });
    await waitFor(() => expect(screen.queryByTestId("tether-pending-receipts")).toBeNull(), { timeout: 2000 });
  });

  it("取景画面叠加相机回报的 AF 框（内容区内定位）", async () => {
    sessionMock.mockResolvedValue(dto());
    // jsdom 无布局：伪造 view（400×200）与 img（100×100 居中）的矩形
    const rectSpy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function (this: HTMLElement) {
        if (this.dataset.testid === "tether-view") {
          return { left: 0, top: 0, width: 400, height: 200, right: 400, bottom: 200, x: 0, y: 0, toJSON: () => ({}) } as DOMRect;
        }
        return { left: 150, top: 50, width: 100, height: 100, right: 250, bottom: 150, x: 150, y: 50, toJSON: () => ({}) } as DOMRect;
      });
    frameMock.mockResolvedValue({ url: "data:image/jpeg;base64,FRAME", af: [0.25, 0.25, 0.5, 0.5] });
    renderPage();

    const box = await screen.findByTestId("tether-af-region");
    // 内容区（view 的 37.5%×25% 起，100×100/400×200）内按 af 归一化定位
    expect(box.style.left).toBe("43.75%");
    expect(box.style.top).toBe("37.5%");
    expect(box.style.width).toBe("12.5%");
    expect(box.style.height).toBe("25%");
    rectSpy.mockRestore();
  });
});
