import { beforeEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useSearchParams } from "react-router";

import i18n from "@/i18n";
import LibraryPickerPage from "./LibraryPickerPage";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
  type Library,
} from "@/stores/settingsStore";

const LIB_A: Library = {
  id: "lib-1",
  name: "主库",
  dbDir: "I:\\SmartPhoto\\主库",
  photoRoot: "Y:\\照片",
  importSubdir: "SmartPhoto",
  configured: true,
streams: 4,
};
const LIB_B: Library = {
  id: "lib-2",
  name: "工作库",
  dbDir: "E:\\db2",
  photoRoot: "E:\\照片",
  importSubdir: "",
  configured: true,
streams: 4,
};

function OnboardingProbe() {
  const [params] = useSearchParams();
  return <div data-testid="onboarding-probe">{params.get("library") ?? "NEW"}</div>;
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY</div>;
}

function renderPicker() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/library-picker"]}>
        <Routes>
          <Route path="/library-picker" element={<LibraryPickerPage />} />
          <Route path="/onboarding" element={<OnboardingProbe />} />
          <Route path="/gallery" element={<GalleryProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

function setLibraries(libraries: Library[], activeLibraryId: string | null): void {
  useSettingsStore.setState((s) => ({
    settings: { ...s.settings, libraries, activeLibraryId },
  }));
}

beforeEach(() => {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
});

describe("LibraryPickerPage（达芬奇式启动首屏）", () => {
  it("渲染库卡片：名称/目录摘要 + 上次使用标记 + 激活库预选（模板行已退役）", async () => {
    setLibraries([LIB_A, LIB_B], "lib-1");
    renderPicker();

    const cards = await screen.findAllByTestId("picker-card");
    expect(cards).toHaveLength(2);
    const cardA = cards.find((el) => el.getAttribute("data-library-id") === "lib-1");
    const cardB = cards.find((el) => el.getAttribute("data-library-id") === "lib-2");
    expect(cardA).toHaveTextContent("主库");
    expect(cardA).toHaveTextContent("Y:\\照片");
    // dirTemplate 配置退役：卡片不再展示模板摘要
    expect(cardA).not.toHaveTextContent("{YYYY}");
    expect(cardA).toHaveTextContent("上次使用");
    expect(cardA).toHaveAttribute("data-selected", "true");
    expect(cardB).toHaveAttribute("data-selected", "false");
  });

  it("选中+打开：设置 activeLibraryId、libraryChosen 并进入主界面", async () => {
    setLibraries([LIB_A, LIB_B], "lib-1");
    renderPicker();
    const user = userEvent.setup();

    const cardB = (await screen.findAllByTestId("picker-card")).find(
      (el) => el.getAttribute("data-library-id") === "lib-2",
    );
    await user.click(cardB!);
    expect(cardB).toHaveAttribute("data-selected", "true");
    await user.click(screen.getByTestId("picker-open"));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(useSettingsStore.getState().settings.activeLibraryId).toBe("lib-2");
    expect(useSettingsStore.getState().libraryChosen).toBe(true);
  });

  it("双击库卡片直接进入", async () => {
    setLibraries([LIB_A], "lib-1");
    renderPicker();
    const user = userEvent.setup();

    await user.dblClick((await screen.findAllByTestId("picker-card"))[0]);

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(useSettingsStore.getState().settings.activeLibraryId).toBe("lib-1");
  });

  it("打开未配置库（configured=false）：先进向导补完并带 ?library=", async () => {
    setLibraries([{ ...LIB_A, configured: false }], null);
    renderPicker();
    const user = userEvent.setup();

    await user.click(screen.getByTestId("picker-open"));

    const probe = await screen.findByTestId("onboarding-probe");
    expect(probe).toHaveTextContent("lib-1");
    // 打开即激活（补完提交后正式进入主界面）
    expect(useSettingsStore.getState().settings.activeLibraryId).toBe("lib-1");
    expect(useSettingsStore.getState().libraryChosen).toBe(true);
  });

  it("无任何库：直接进入新建库向导", async () => {
    setLibraries([], null);
    renderPicker();

    expect(await screen.findByTestId("onboarding-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("picker-grid")).not.toBeInTheDocument();
  });

  it("「新建库」按钮进入向导", async () => {
    setLibraries([LIB_A], "lib-1");
    renderPicker();
    const user = userEvent.setup();

    await user.click(screen.getByTestId("picker-create"));

    expect(await screen.findByTestId("onboarding-probe")).toHaveTextContent("NEW");
    // 新建入口不改变当前激活状态
    expect(useSettingsStore.getState().settings.activeLibraryId).toBe("lib-1");
    expect(useSettingsStore.getState().libraryChosen).toBe(false);
  });

  it("设置未加载完成时等待（不闪空态直送）", () => {
    useSettingsStore.setState({ loaded: false, libraryChosen: false });
    const { container } = renderPicker();

    expect(container.querySelector("[data-testid='picker-grid']")).toBeNull();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });

  it("渲染自绘标题栏（主壳外全屏页：窗口可拖动/控制）", () => {
    setLibraries([LIB_A], "lib-1");
    renderPicker();

    expect(screen.getByTestId("titlebar")).toBeInTheDocument();
    expect(screen.getByTestId("titlebar-drag-region")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByTestId("titlebar-close")).toBeInTheDocument();
    // 无菜单内容（菜单只在主壳 TitleBar 中）
    expect(screen.queryByTestId("menubar")).not.toBeInTheDocument();
  });
});
