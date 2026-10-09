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

// 门禁要拉数据库注册表与照片库登记表：按契约 mock（内容由各用例给定）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseList: vi.fn(),
    photoLibraryList: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import { databaseList, photoLibraryList } from "@/ipc/api";
import type { DatabaseList, PhotoLibrary } from "@/ipc/api";

const dbListMock = vi.mocked(databaseList);
const listMock = vi.mocked(photoLibraryList);

/**
 * 主壳守卫（2026-10-09 多数据库修正）：设置未加载完成/注册表未拉到时空白等待；
 * 加载后**尚无数据库，或无任何照片库且未完成引导** → /onboarding（创建数据
 * 库 → 引导建立第一个照片库）；已有数据库且引导已完成（或有照片库）直进主壳。
 */

function databases(count = 1): DatabaseList {
  return {
    databases: Array.from({ length: count }, (_, i) => ({
      id: `db-${i + 1}`,
      name: `数据库 ${i + 1}`,
      dbDir: `D:\db-${i + 1}`,
    })),
    activeId: "db-1",
  };
}

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
  dbListMock.mockReset().mockResolvedValue(databases());
  listMock.mockReset().mockResolvedValue([]);
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
  });
});

describe("GatedShell（无数据库或无照片库 → 引导；否则直进主壳）", () => {
  it("设置未加载完成时空白等待", () => {
    listMock.mockResolvedValue([library()]);
    useSettingsStore.setState({ loaded: false });
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });

  it("注册表未拉到（首帧）时空白等待", () => {
    dbListMock.mockReturnValue(new Promise<DatabaseList>(() => {})); // 永不 resolve
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });

  it("尚无数据库（即使引导未完成）→ 送 /onboarding", async () => {
    dbListMock.mockResolvedValue({ databases: [], activeId: null });
    listMock.mockResolvedValue([library()]);
    renderAtGallery();

    expect(await screen.findByTestId("onboarding-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
  });

  it("有数据库但尚无照片库且未完成引导 → 送 /onboarding", async () => {
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
