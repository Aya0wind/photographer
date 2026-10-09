import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：store 依赖 settings_set；@/ipc/api 的 databaseCreate /
// photoLibraryCreate 按契约 mock
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseCreate: vi.fn(),
    databaseList: vi.fn(async () => ({ databases: [], activeId: null })),
    photoLibraryCreate: vi.fn(),
    photoLibraryList: vi.fn(async () => []),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import OnboardingPage from "./OnboardingPage";
import { I18nextProvider } from "react-i18next";
import i18n from "@/i18n";
import { useSettingsStore } from "@/stores/settingsStore";
import { databaseCreate, databaseList, photoLibraryCreate } from "@/ipc/api";

/**
 * 首次引导（2026-10-09 多数据库修正）：
 * 创建数据库（名称必填 + 位置可选，databaseCreate；已有数据库时只读展示直进）
 * → 引导建立第一个照片库（新建 / 从已有文件夹建立，photoLibraryCreate）→
 * 完成写 onboardingCompleted 进画廊；可「稍后再建」跳过。
 */

const createMock = vi.mocked(photoLibraryCreate);
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
  createMock.mockReset();
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

describe("OnboardingPage（多数据库修正：创建数据库 → 建立第一个照片库）", () => {
  it("设置未加载完成前不渲染", () => {
    useSettingsStore.setState({ loaded: false });
    renderOnboarding();
    expect(screen.queryByTestId("onboarding-card")).not.toBeInTheDocument();
  });

  it("步骤 1 创建数据库：名称必填、位置可空（默认约定路径）→ 成功进照片库步", async () => {
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
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "library"),
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

  it("已有数据库（引导重入）：只读展示当前库名 + 直接进下一步", async () => {
    dbListMock.mockResolvedValue({
      databases: [{ id: "db-1", name: "现有数据库", dbDir: "D:\\db" }],
      activeId: "db-1",
    });
    renderOnboarding();
    // 注册表异步拉取：等待只读展示态出现（null 首帧短暂渲染创建表单）
    expect(await screen.findByTestId("onboarding-db-current")).toHaveTextContent("现有数据库");
    expect(screen.queryByTestId("onboarding-db-create")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("onboarding-next"));
    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "library"),
    );
    expect(dbCreateMock).not.toHaveBeenCalled();
  });

  it("建立照片库：新建模式 photoLibraryCreate(reference=false) → 完成步 → 开始使用", async () => {
    mockDbCreateOk();
    createMock.mockResolvedValue({
      ok: true,
      library: {
        id: "lib-1",
        name: "主照片库",
        rootPath: "D:\\照片",
        createdAt: "2026-10-09T00:00:00Z",
        status: "online",
        assetCount: 0,
        sizeBytes: 0,
      },
    });
    const user = userEvent.setup();
    renderOnboarding();

    // 第一步：创建数据库（名称必填）→ 自动进照片库步；新建模式为默认
    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    expect(screen.getByTestId("onboarding-mode-new")).toHaveAttribute("data-selected", "true");
    // 未填路径不能建
    expect(screen.getByTestId("onboarding-library-create")).toBeDisabled();

    await user.type(screen.getByTestId("onboarding-library-root"), "D:\\照片");
    await user.click(screen.getByTestId("onboarding-library-create"));

    expect(createMock).toHaveBeenCalledWith("主库", "D:\\照片", false);
    const card = await screen.findByTestId("onboarding-card");
    expect(card).toHaveAttribute("data-step", "done");
    expect(screen.getByTestId("onboarding-finished-root")).toHaveTextContent("D:\\照片");

    await user.click(screen.getByTestId("onboarding-start"));
    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toBeInTheDocument());
    expect(useSettingsStore.getState().settings.onboardingCompleted).toBe(true);
  });

  it("从已有文件夹建立：reference=true 下发；后端业务错误内联展示", async () => {
    mockDbCreateOk();
    createMock.mockResolvedValue({ ok: false, error: "路径与其他照片库重叠" });
    const user = userEvent.setup();
    renderOnboarding();

    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.click(screen.getByTestId("onboarding-mode-reference"));
    await user.clear(screen.getByTestId("onboarding-library-name"));
    await user.type(screen.getByTestId("onboarding-library-name"), "备份库");
    await user.type(screen.getByTestId("onboarding-library-root"), "E:\\备份");
    await user.click(screen.getByTestId("onboarding-library-create"));

    expect(createMock).toHaveBeenCalledWith("备份库", "E:\\备份", true);
    expect(await screen.findByTestId("onboarding-library-error")).toHaveTextContent(
      "路径与其他照片库重叠",
    );
    expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "library");
  });

  it("照片库 invoke 不可用（error=null）：内联提示后端未连接", async () => {
    mockDbCreateOk();
    createMock.mockResolvedValue({ ok: false, error: null });
    const user = userEvent.setup();
    renderOnboarding();

    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.type(screen.getByTestId("onboarding-library-root"), "D:\\照片");
    await user.click(screen.getByTestId("onboarding-library-create"));

    expect(await screen.findByTestId("onboarding-library-error")).toHaveTextContent(
      "后端未连接",
    );
  });

  it("稍后再建：跳过建库直接完成引导进画廊（数据库已建）", async () => {
    mockDbCreateOk();
    const user = userEvent.setup();
    renderOnboarding();

    await user.type(screen.getByTestId("onboarding-db-name"), "主数据库");
    await user.click(screen.getByTestId("onboarding-db-create"));
    await user.click(screen.getByTestId("onboarding-later"));

    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toBeInTheDocument());
    expect(useSettingsStore.getState().settings.onboardingCompleted).toBe(true);
    expect(createMock).not.toHaveBeenCalled();
  });
});
