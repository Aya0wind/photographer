import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import { GatedShell } from "./routes";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
} from "@/stores/settingsStore";

// 门禁要拉照片库登记表：按 P0 契约 mock（列表内容由各用例给定）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    photoLibraryList: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import { photoLibraryList } from "@/ipc/api";
import type { PhotoLibrary } from "@/ipc/api";

const listMock = vi.mocked(photoLibraryList);

/**
 * 主壳守卫（M5 收尾，2026-10-09 单库多照片库定案）：设置未加载完成时空白等待；
 * 加载后**尚无任何照片库且未完成引导** → /onboarding（数据库就位 → 引导建立
 * 第一个照片库）；已有照片库或引导已完成 → 直进主壳（画廊全局跨库混排）。
 */

function library(id = "lib-1"): PhotoLibrary {
  return {
    id,
    name: "主照片库",
    rootPath: "D:\\照片",
    createdAt: "2026-10-09T00:00:00Z",
    status: "online",
    assetCount: 10,
    sizeBytes: 1024,
  };
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY</div>;
}

function OnboardingProbe() {
  return <div data-testid="onboarding-probe">ONBOARDING</div>;
}

function renderAtGallery() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <Routes>
          <Route path="/" element={<GatedShell />}>
            <Route path="gallery" element={<GalleryProbe />} />
          </Route>
          <Route path="/onboarding" element={<OnboardingProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  listMock.mockReset().mockResolvedValue([]);
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
  });
});

describe("GatedShell（尚无照片库 → 引导；有库直进主壳）", () => {
  it("设置未加载完成时空白等待", () => {
    listMock.mockResolvedValue([library()]);
    useSettingsStore.setState({ loaded: false });
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });

  it("尚无任何照片库且未完成引导 → 送 /onboarding", async () => {
    listMock.mockResolvedValue([]);
    renderAtGallery();

    expect(await screen.findByTestId("onboarding-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
  });

  it("已有照片库 → 直进主壳（不再有选库门禁）", async () => {
    listMock.mockResolvedValue([library()]);
    renderAtGallery();

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
  });

  it("引导已完成（即使登记表为空/后端不可用）→ 直进主壳，不回引导页", async () => {
    listMock.mockResolvedValue([]);
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, onboardingCompleted: true },
    }));
    renderAtGallery();

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });
});
