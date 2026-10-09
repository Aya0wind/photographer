import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes } from "react-router";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：store 依赖 settings_set；@/ipc/api 的 photoLibraryCreate 按 P0 契约 mock
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    photoLibraryCreate: vi.fn(),
    photoLibraryList: vi.fn(async () => []),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import OnboardingPage from "./OnboardingPage";
import { I18nextProvider } from "react-i18next";
import i18n from "@/i18n";
import { useSettingsStore } from "@/stores/settingsStore";
import { photoLibraryCreate } from "@/ipc/api";

/**
 * 首次引导（M5 重构，2026-10-09 单库多照片库定案 §七）：
 * 数据库就位（默认应用数据目录/自定义 databaseDir 只读展示）→ 引导建立第一个
 * 照片库（新建 / 从已有文件夹建立，photoLibraryCreate）→ 完成写 onboardingCompleted
 * 进画廊；可「稍后再建」跳过。
 */

const createMock = vi.mocked(photoLibraryCreate);

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
  useSettingsStore.setState({
    settings: {
      ...useSettingsStore.getState().settings,
      onboardingCompleted: false,
      databaseDir: null,
    },
    loaded: true,
  });
});

describe("OnboardingPage（M5：数据库就位 → 建立第一个照片库）", () => {
  it("设置未加载完成前不渲染", () => {
    useSettingsStore.setState({ loaded: false });
    renderOnboarding();
    expect(screen.queryByTestId("onboarding-card")).not.toBeInTheDocument();
  });

  it("步骤 1 数据库就位：默认应用数据目录展示 + 可跳下一步", async () => {
    renderOnboarding();
    const card = screen.getByTestId("onboarding-card");
    expect(card).toHaveAttribute("data-step", "welcome");
    expect(screen.getByTestId("onboarding-db-location")).toHaveTextContent("默认：应用数据目录");

    fireEvent.click(screen.getByTestId("onboarding-next"));
    await waitFor(() =>
      expect(screen.getByTestId("onboarding-card")).toHaveAttribute("data-step", "library"),
    );
  });

  it("自定义数据库位置（databaseDir）只读展示", () => {
    useSettingsStore.setState({
      settings: {
        ...useSettingsStore.getState().settings,
        databaseDir: "D:\\PhotoHubDB",
      },
    });
    renderOnboarding();
    expect(screen.getByTestId("onboarding-db-location")).toHaveTextContent("D:\\PhotoHubDB");
  });

  it("建立照片库：新建模式 photoLibraryCreate(reference=false) → 完成步 → 开始使用", async () => {
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

    fireEvent.click(screen.getByTestId("onboarding-next"));
    // 新建模式为默认
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
    createMock.mockResolvedValue({ ok: false, error: "路径与其他照片库重叠" });
    const user = userEvent.setup();
    renderOnboarding();

    fireEvent.click(screen.getByTestId("onboarding-next"));
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

  it("invoke 不可用（error=null）：内联提示后端未连接", async () => {
    createMock.mockResolvedValue({ ok: false, error: null });
    const user = userEvent.setup();
    renderOnboarding();

    fireEvent.click(screen.getByTestId("onboarding-next"));
    await user.type(screen.getByTestId("onboarding-library-root"), "D:\\照片");
    await user.click(screen.getByTestId("onboarding-library-create"));

    expect(await screen.findByTestId("onboarding-library-error")).toHaveTextContent(
      "后端未连接",
    );
  });

  it("稍后再建：跳过建库直接完成引导进画廊", async () => {
    const user = userEvent.setup();
    renderOnboarding();

    fireEvent.click(screen.getByTestId("onboarding-next"));
    await user.click(screen.getByTestId("onboarding-later"));

    await waitFor(() => expect(screen.getByTestId("gallery-probe")).toBeInTheDocument());
    expect(useSettingsStore.getState().settings.onboardingCompleted).toBe(true);
    expect(createMock).not.toHaveBeenCalled();
  });
});
