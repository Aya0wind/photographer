import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：store 依赖 settings_set；@/ipc/api 的 databaseCreate /
// aiModelsStatus / aiModelDownload 按契约 mock
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseCreate: vi.fn(),
    photoLibraryList: vi.fn(async () => []),
    photoLibraryReorganize:vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
    aiModelsStatus: vi.fn(async () => []),
    aiModelDownload: vi.fn(async () => undefined),
  };
});

it("照片库是单独步骤，使用已有文件夹时整理开关默认关闭",async()=>{
  const user=userEvent.setup();renderOnboarding();await user.type(screen.getByTestId("onboarding-db-name"),"新数据库");await user.click(screen.getByTestId("onboarding-db-create"));
  expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step","library");expect(dbCreateMock).not.toHaveBeenCalled();expect(screen.getByRole("heading",{name:"设置照片库"})).toBeInTheDocument();expect(screen.queryByTestId("onboarding-reorganize-existing")).not.toBeInTheDocument();
  await user.click(screen.getByTestId("onboarding-library-existing"));expect(screen.getByTestId("onboarding-reorganize-existing")).not.toBeChecked();expect(screen.getByText(/空文件夹也不会删除/)).toBeInTheDocument();expect(screen.getByTestId("onboarding-library-continue")).toBeDisabled();
});
it("先整理现有照片再进入 AI，整理失败重试不会重复建库",async()=>{
  const user=userEvent.setup();mockDbCreateOk();vi.mocked(photoLibraryList).mockResolvedValue([{id:"organizing",name:"Photos",rootPath:"I:/Photos",organizationTemplate:"{year}/{month}/{day}/{filename}",createdAt:"2026-10-11",status:"online",assetCount:0,sizeBytes:0,organizationPending:true}]);
  vi.mocked(photoLibraryReorganize).mockRejectedValueOnce(new Error("照片发生变化，已停止"));
  renderOnboarding();await user.type(screen.getByTestId("onboarding-db-name"),"新数据库");await user.click(screen.getByTestId("onboarding-db-create"));await user.click(screen.getByTestId("onboarding-library-existing"));await user.type(screen.getByTestId("onboarding-photo-root"),"I:/Photos");await user.click(screen.getByTestId("onboarding-reorganize-existing"));await user.click(screen.getByTestId("onboarding-library-continue"));
  expect(await screen.findByRole("alertdialog")).toHaveTextContent("照片发生变化");expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step","library");expect(dbCreateMock).toHaveBeenCalledWith("新数据库",undefined,expect.objectContaining({existingFolder:true,reorganizeExisting:true}));
  await user.click(screen.getByRole("button",{name:i18n.t("common.confirm")}));vi.mocked(photoLibraryReorganize).mockResolvedValue({libraryId:"organizing",phase:"done",completed:3,total:3});await user.click(screen.getByTestId("onboarding-library-continue"));await waitFor(()=>expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step","ai"));expect(dbCreateMock).toHaveBeenCalledTimes(1);expect(photoLibraryReorganize).toHaveBeenLastCalledWith("organizing");
});

import OnboardingPage from "./OnboardingPage";
import { I18nextProvider } from "react-i18next";
import i18n from "@/i18n";
import { useSettingsStore } from "@/stores/settingsStore";
import { useAiStore } from "@/stores/aiStore";
import { databaseCreate,photoLibraryList,photoLibraryReorganize, aiModelsStatus, aiModelDownload } from "@/ipc/api";
import type { AiModelStatus } from "@/ipc/api";

/**
 * 首次引导（2026-10-10 定案：创建数据库 → AI 功能开关 → 完成）：
 * 创建数据库恒为创建表单（名称必填 + 位置可选，databaseCreate——多库合法，
 * 创建即激活，不再按注册表只读直进）→ AI 功能开关（两卡选择写入
 * settings.ai 三开关，模型不在引导中下载）→ 完成写 onboardingCompleted
 * 进画廊。照片库不在引导中建立（存储页自建）。
 */

const dbCreateMock = vi.mocked(databaseCreate);
const modelsMock = vi.mocked(aiModelsStatus);
const downloadMock = vi.mocked(aiModelDownload);

/** 模型状态桩（state 默认 done） */
function model(id: string, state: AiModelStatus["state"] = "done"): AiModelStatus {
  return { id, installed: state === "done", bytesTotal: 1000, downloadedBytes: state === "done" ? 1000 : 0, version: "1", feature: "semantic", state, tier: null };
}

/** 普通档全件（语义三件 + scrfd + arcface）+ 精准档专用件 */
const ALL_DONE = [
  "siglip2-visual", "siglip2-text", "siglip2-tokenizer", "scrfd", "arcface", "facemesh",
  "siglip2-visual-fp16", "siglip2-text-fp16", "scrfd-10g",
].map((id) => model(id));

describe("建库 AI 功能独立选择", () => {
  it.each([1, 2, 3, 4, 5, 6, 7])("组合 %s 只下载所选功能的模型，共用模型仅下载一次", async mask => {
    const chosen = { face: Boolean(mask & 1), selection: Boolean(mask & 2), semantic: Boolean(mask & 4) };
    const expected = [...new Set([
      ...(chosen.face ? ["scrfd", "arcface"] : []),
      ...(chosen.selection ? ["scrfd", "facemesh"] : []),
      ...(chosen.semantic ? ["siglip2-visual", "siglip2-text", "siglip2-tokenizer"] : []),
    ])];
    modelsMock.mockImplementation(async () => downloadMock.mock.calls.length >= expected.length ? ALL_DONE : ALL_DONE.map(item => model(item.id, "idle")));
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);
    await user.click(screen.getByTestId("onboarding-ai-tier-normal"));
    for (const feature of ["face", "selection", "semantic"] as const) {
      if (!chosen[feature]) await user.click(screen.getByTestId(`onboarding-ai-feature-${feature}`));
    }
    await user.click(screen.getByTestId("onboarding-ai-continue"));
    await waitFor(() => expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"));
    expect(downloadMock.mock.calls.map(call => call[0]).sort()).toEqual(expected.sort());
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(chosen.semantic);
    expect(ai.enableFace).toBe(chosen.face);
    expect(ai.enableSelection).toBe(chosen.selection);
  });

  it("开启 AI 时不能空选功能", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);
    for (const feature of ["face", "selection", "semantic"]) await user.click(screen.getByTestId(`onboarding-ai-feature-${feature}`));
    expect(screen.getByTestId("onboarding-ai-continue")).toBeDisabled();
    expect(screen.getByTestId("onboarding-ai-later")).toBeDisabled();
    expect(downloadMock).not.toHaveBeenCalled();
  });
});

/** 建库成功桩（第一步直通用例共用） */
function mockDbCreateOk(): void {
  dbCreateMock.mockResolvedValue({
    ok: true,
    database: { id: "db-1", name: "主数据库", dbDir: "C:\\appdata\\databases\\db-1" },
  });
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY</div>;
}

function renderOnboarding() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/onboarding"]}>
        <Routes>
          <Route path="/onboarding" element={<OnboardingPage />} />
          <Route path="/gallery" element={<GalleryProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  dbCreateMock.mockReset();
  modelsMock.mockReset().mockResolvedValue(ALL_DONE);
  downloadMock.mockReset().mockResolvedValue(undefined);
  // aiStore 是模块级状态：清掉上个用例刷新进来的 models，防假就绪直通
  useAiStore.setState({ models: [], downloadProgress: {} });
  useSettingsStore.setState({
    settings: {
      ...useSettingsStore.getState().settings,
      onboardingCompleted: false,
    },
    loaded: true,
  });
});

/** 走到 AI 步（第一步表单建库） */
async function reachAiStep(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  mockDbCreateOk();
  await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
  await user.click(screen.getByTestId("onboarding-db-create"));
    await user.type(screen.getByTestId("onboarding-photo-root"), "I:\\Photos");
    await user.click(screen.getByTestId("onboarding-library-continue"));
  await waitFor(() =>
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "ai"),
  );
}

describe("OnboardingPage（创建数据库 → AI 功能开关 → 完成）", () => {
  it("设置未加载完成前不渲染", () => {
    useSettingsStore.setState({ loaded: false });
    renderOnboarding();
    expect(screen.queryByTestId("onboarding-card")).not.toBeInTheDocument();
  });

  it("步骤 1 创建数据库：名称必填、位置可空（默认约定路径）→ 成功进 AI 步", async () => {
    mockDbCreateOk();
    const user = userEvent.setup();
    renderOnboarding();
    const card = screen.getByTestId("onboarding-card");
    expect(card).toHaveAttribute("data-step", "welcome");

    // 未填名称不能创建
    expect(screen.getByTestId("onboarding-db-create")).toBeDisabled();
    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.type(screen.getByTestId("onboarding-photo-root"), "I:\\Photos");
    await user.click(screen.getByTestId("onboarding-library-continue"));

    // 位置留空 → dbDir 传 undefined（后端走默认约定路径）
    expect(dbCreateMock).toHaveBeenCalledWith("主数据库", undefined, expect.objectContaining({rootPath:"I:\\Photos",organizationTemplate:"{year}/{month}/{day}/{filename}"}));
    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "ai"),
    );
  });

  it("步骤 1 自定义位置：填入 dbDir 原样下发；业务错误内联展示", async () => {
    dbCreateMock.mockResolvedValue({ ok: false, error: "数据库位置与已有数据库相同或互相包含" });
    const user = userEvent.setup();
    renderOnboarding();
    await user.type(screen.getByTestId("onboarding-db-name"), "坏库");
    await user.type(screen.getByTestId("onboarding-db-location"), "D:\\db");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.type(screen.getByTestId("onboarding-photo-root"), "I:\\Photos");
    await user.click(screen.getByTestId("onboarding-library-continue"));

    expect(dbCreateMock).toHaveBeenCalledWith("坏库", "D:\\db", expect.objectContaining({rootPath:"I:\\Photos",organizationTemplate:"{year}/{month}/{day}/{filename}"}));
    expect(await screen.findByTestId("operation-error-dialog")).toHaveTextContent(
      "数据库位置与已有数据库相同或互相包含",
    );
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "library");
  });

  it("步骤 1 invoke 不可用（error=null）：内联提示后端未连接", async () => {
    dbCreateMock.mockResolvedValue({ ok: false, error: null });
    const user = userEvent.setup();
    renderOnboarding();
    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.type(screen.getByTestId("onboarding-photo-root"), "I:\\Photos");
    await user.click(screen.getByTestId("onboarding-library-continue"));

    expect(await screen.findByTestId("operation-error-dialog")).toHaveTextContent("后端未连接");
  });

  it("AI 步默认「启用+普通档」：模型全就绪 → 下载并继续即时完成，三开关全开 + 档位写入", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    expect(screen.getByTestId("onboarding-ai-enable")).toHaveAttribute("data-selected", "true");
    expect(screen.getByTestId("onboarding-ai-tier-normal")).toHaveAttribute("data-selected", "true");
    await user.click(screen.getByTestId("onboarding-ai-continue"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(true);
    expect(ai.enableFace).toBe(true);
    expect(ai.enableSelection).toBe(true);
    expect(ai.enableSceneTags).toBe(false);
    expect(ai.qualityTier).toBe("normal");
    // 已就绪资源不重复下载
    expect(downloadMock).not.toHaveBeenCalled();
  });

  it("档位切换：选精准档 → 下载并继续 → qualityTier=accurate 写入", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    await user.click(screen.getByTestId("onboarding-ai-tier-accurate"));
    await user.click(screen.getByTestId("onboarding-ai-continue"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("accurate");
  });

  it("稍后手动下载：不触发下载直接进完成步，设置已写入（逃生门）", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    await user.click(screen.getByTestId("onboarding-ai-later"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(true);
    expect(ai.enableFace).toBe(true);
    expect(downloadMock).not.toHaveBeenCalled();
  });

  it("暂不启用：主按钮=下一步 → 三开关全关，无下载", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    await user.click(screen.getByTestId("onboarding-ai-disable"));
    await user.click(screen.getByTestId("onboarding-ai-continue"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(false);
    expect(ai.enableFace).toBe(false);
    expect(ai.enableSceneTags).toBe(false);
    expect(downloadMock).not.toHaveBeenCalled();
  });

  it("缺口补齐：pending 清单 → 逐个 aiModelDownload → 就绪后自动进完成步", async () => {
    const pending = ["siglip2-visual", "siglip2-text", "siglip2-tokenizer", "scrfd", "arcface", "facemesh"].map(
      (id) => model(id, "idle"),
    );
    // 挂载即有两次刷新（轮询 effect + 下载 effect）：按下载发起数动态返回，
    // 五件全部发起前恒为 pending——避免 Once 被首次刷新消费造成假就绪
    modelsMock.mockReset().mockImplementation(async () =>
      downloadMock.mock.calls.length >= 6 ? ALL_DONE : pending,
    );
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);
    // 显式回普通档（前序用例可能已把 settings.ai.qualityTier 写成 accurate）
    await user.click(screen.getByTestId("onboarding-ai-tier-normal"));

    await user.click(screen.getByTestId("onboarding-ai-continue"));

    // 面板下载瞬完成即退场——以结果断言：五个缺件全部发起（strict=true）+ 前进
    await waitFor(() => expect(downloadMock).toHaveBeenCalledTimes(6));
    expect(downloadMock).toHaveBeenCalledWith("siglip2-visual", true);
    expect(downloadMock).toHaveBeenCalledWith("arcface", true);
    expect(downloadMock).toHaveBeenCalledWith("facemesh", true);

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    expect(useSettingsStore.getState().settings.ai.enableClip).toBe(true);
  });

  it("AI 步保存失败：错误内联展示且不前进", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    const store = useSettingsStore.getState();
    useSettingsStore.setState({
      save: vi.fn().mockRejectedValue(new Error("settings_set 失败")),
    });
    await user.click(screen.getByTestId("onboarding-ai-continue"));

    expect(await screen.findByTestId("onboarding-ai-error")).toHaveTextContent("settings_set 失败");
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "ai");
    useSettingsStore.setState({ save: store.save });
  });
});
