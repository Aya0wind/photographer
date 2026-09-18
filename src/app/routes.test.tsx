import { beforeEach, describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import { GatedShell } from "./routes";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
  type Library,
} from "@/stores/settingsStore";

/**
 * 达芬奇式启动流守卫：主壳路由受「会话内已选库（libraryChosen，非持久）+
 * activeLibraryId 有效」双重保护，未选库一律重定向 /library-picker。
 * onboardingCompleted 不再作门禁。
 */

const LIB: Library = {
  id: "lib-1",
  name: "主库",
  dbDir: "I:\\SmartPhoto\\主库",
  photoRoot: "Y:\\照片",
  dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
  importSubdir: "SmartPhoto",
  configured: true,
};

function PickerProbe() {
  return <div data-testid="picker-probe">PICKER</div>;
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY</div>;
}

function renderAtGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/library-picker" element={<PickerProbe />} />
          <Route path="/" element={<GatedShell />}>
            <Route path="gallery" element={<GalleryProbe />} />
          </Route>
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    libraryChosen: false,
  });
});

describe("GatedShell（每次启动先选库）", () => {
  it("设置未加载完成时空白等待", () => {
    useSettingsStore.setState({ loaded: false });
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("picker-probe")).not.toBeInTheDocument();
  });

  it("本会话未选库（libraryChosen=false）→ 重定向 /library-picker", () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB], activeLibraryId: "lib-1" },
    }));
    renderAtGallery();

    expect(screen.getByTestId("picker-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
  });

  it("已选库但 activeLibraryId 无效 → 重定向选择器", () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, libraries: [LIB], activeLibraryId: "missing" },
      libraryChosen: true,
    }));
    renderAtGallery();

    expect(screen.getByTestId("picker-probe")).toBeInTheDocument();
  });

  it("已选库且激活有效 → 进入主壳（onboardingCompleted 不再门禁）", () => {
    useSettingsStore.setState((s) => ({
      settings: {
        ...s.settings,
        onboardingCompleted: false,
        libraries: [LIB],
        activeLibraryId: "lib-1",
      },
      libraryChosen: true,
    }));
    renderAtGallery();

    expect(screen.getByTestId("gallery-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("picker-probe")).not.toBeInTheDocument();
  });
});
