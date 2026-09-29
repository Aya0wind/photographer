/** 联拍独立窗口：会话快照渲染 / 参数设置 / 拍摄 / 事件驱动胶片条 / 断连与结束态 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
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
  };
});

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

  it("拍摄模式固定工具栏首位：只读标志下也不锁死（照常尝试切换）", async () => {
    // P 档后机身把 expprogram 上报为只读——控件仍显示、仍可点，尝试下发
    sessionMock.mockResolvedValue(
      dto({ settings: [choice("expprogram", "P", false, ["P", "A", "S", "M"]), choice("iso", "400", false)] }),
    );
    settingSetMock.mockResolvedValue([choice("expprogram", "M", true, ["P", "A", "S", "M"])]);
    renderPage();
    const mode = await screen.findByTestId("tether-setting-expprogram");
    expect(mode).toHaveTextContent("拍摄模式");
    expect(mode).toHaveTextContent("P");
    expect(mode).not.toBeDisabled();
    expect(mode).toHaveAttribute("title", i18n.t("tether.modeDialHint"));
    // 只读标志不拦截模式切换：点开弹层选 M，照常下发
    fireEvent.click(mode);
    fireEvent.click(screen.getByTestId("tether-quick-expprogram-menu").querySelector('[data-value="M"]')!);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "expprogram", "M"));
    // 其他只读参数（ISO）依旧隐藏
    expect(screen.queryByTestId("tether-setting-iso")).toBeNull();
  });

  it("拍摄模式可写（支持远程切换的机型）：弹层直接切档", async () => {
    sessionMock.mockResolvedValue(
      dto({ settings: [choice("expprogram", "P", true, ["P", "A", "S", "M"])] }),
    );
    settingSetMock.mockResolvedValue([choice("expprogram", "M", true, ["P", "A", "S", "M"])]);
    renderPage();
    fireEvent.click(await screen.findByTestId("tether-setting-expprogram"));
    fireEvent.click(screen.getByTestId("tether-quick-expprogram-menu").querySelector('[data-value="M"]')!);
    await waitFor(() => expect(settingSetMock).toHaveBeenCalledWith("s1", "expprogram", "M"));
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
    });
    expect(await screen.findByTestId("tether-film-42")).toBeInTheDocument();
    handler!({
      type: "tetheringPhotoAdded",
      sessionId: "s1",
      libraryId: "lib-1",
      albumId: 7,
      assetId: 42,
      name: "DSC_0002.JPG",
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

  it("工具栏拖动：grip 按住拖动改变定位（脱离底部居中）并持久化", async () => {
    window.localStorage.clear();
    sessionMock.mockResolvedValue(dto());
    renderPage();
    const grip = await screen.findByTestId("tether-quickbar-grip");

    fireEvent.pointerDown(grip, { button: 0, clientX: 400, clientY: 500 });
    fireEvent.pointerMove(window, { clientX: 320, clientY: 430 });
    fireEvent.pointerUp(window);

    const bar = screen.getByTestId("tether-quickbar");
    expect(bar.style.left).not.toBe("");
    expect(window.localStorage.getItem("tethering.quickbar.pos")).not.toBeNull();
    window.localStorage.clear();
  });
});
