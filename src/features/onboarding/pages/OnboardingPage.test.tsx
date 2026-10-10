import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：store 依赖 settings_set；@/ipc/api 的 databaseCreate 按契约 mock
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseCreate: vi.fn(),
    databaseList: vi.fn(async () => ({ databases: [], activeId: null })),
    photoLibraryList: vi.fn(async () => []),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import OnboardingPage from "./OnboardingPage";
import { I18nextProvider } from "react-i18next";
import i18n from "@/i18n";
import { useSettingsStore } from "@/stores/settingsStore";
import { databaseCreate, databaseList } from "@/ipc/api";

/**
 * 首次引导（2026-10-10 定案：创建数据库 → AI 功能开关 → 完成）：
 * 创建数据库（名称必填 + 位置可选，databaseCreate；已有数据库时只读展示直进）
 * → AI 功能开关（两卡选择写入 settings.ai 三开关，模型不在引导中下载）→
 * 完成写 onboardingCompleted 进画廊。照片库不在引导中建立（存储页自建）。
 */

const dbCreateMock = vi.mocked(databaseCreate);
const dbListMock = vi.mocked(databaseList);

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
  dbListMock.mockReset().mockResolvedValue({ databases: [], activeId: null });
  useSettingsStore.setState({
    settings: {
      ...useSettingsStore.getState().settings,
      onboardingCompleted: false,
    },
    loaded: true,
  });
});

/** 走到 AI 步（创建数据库或重入直进共用） */
async function reachAiStep(user: ReturnType<typeof userEvent.setup>): Promise<void> {
  mockDbCreateOk();
  await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
  await user.click(screen.getByTestId("onboarding-db-create"));
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

    // 位置留空 → dbDir 传 undefined（后端走默认约定路径）
    expect(dbCreateMock).toHaveBeenCalledWith("主数据库", undefined);
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

    expect(dbCreateMock).toHaveBeenCalledWith("坏库", "D:\\db");
    expect(await screen.findByTestId("onboarding-db-error")).toHaveTextContent(
      "数据库位置与已有数据库相同或互相包含",
    );
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "welcome");
  });

  it("步骤 1 invoke 不可用（error=null）：内联提示后端未连接", async () => {
    dbCreateMock.mockResolvedValue({ ok: false, error: null });
    const user = userEvent.setup();
    renderOnboarding();
    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));

    expect(await screen.findByTestId("onboarding-db-error")).toHaveTextContent("后端未连接");
  });

  it("已有数据库（引导重入）：只读展示当前库名 + 直接进 AI 步", async () => {
    dbListMock.mockResolvedValue({
      databases: [{ id: "db-1", name: "现有数据库", dbDir: "D:\\db" }],
      activeId: "db-1",
    });
    const user = userEvent.setup();
    renderOnboarding();
    // 注册表异步拉取：等待只读展示态出现（null 首帧短暂渲染创建表单）
    expect(await screen.findByTestId("onboarding-db-current")).toHaveTextContent("现有数据库");
    expect(screen.queryByTestId("onboarding-db-create")).not.toBeInTheDocument();

    await user.click(screen.getByTestId("onboarding-next"));
    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "ai"),
    );
    expect(dbCreateMock).not.toHaveBeenCalled();
  });

  it("AI 步默认「启用」选中：点选写入三开关全开 → 完成步 → 开始使用", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    expect(screen.getByTestId("onboarding-ai-enable")).toHaveAttribute("data-selected", "true");
    await user.click(screen.getByTestId("onboarding-ai-enable"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(true);
    expect(ai.enableFace).toBe(true);
    expect(ai.enableSceneTags).toBe(true);

    await user.click(screen.getByTestId("onboarding-start"));
    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toBeInTheDocument());
    expect(useSettingsStore.getState().settings.onboardingCompleted).toBe(true);
  });

  it("AI 步「暂不启用」：写入三开关全关 → 完成步", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    await user.click(screen.getByTestId("onboarding-ai-disable"));

    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "done"),
    );
    const ai = useSettingsStore.getState().settings.ai;
    expect(ai.enableClip).toBe(false);
    expect(ai.enableFace).toBe(false);
    expect(ai.enableSceneTags).toBe(false);
  });

  it("AI 步保存失败：错误内联展示且不前进", async () => {
    const user = userEvent.setup();
    renderOnboarding();
    await reachAiStep(user);

    const store = useSettingsStore.getState();
    useSettingsStore.setState({
      save: vi.fn().mockRejectedValue(new Error("settings_set 失败")),
    });
    await user.click(screen.getByTestId("onboarding-ai-disable"));

    expect(await screen.findByTestId("onboarding-ai-error")).toHaveTextContent("settings_set 失败");
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "ai");
    useSettingsStore.setState({ save: store.save });
  });
});
