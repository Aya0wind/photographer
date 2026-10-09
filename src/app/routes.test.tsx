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

// 门禁要拉数据库注册表：按契约 mock（内容由各用例给定）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseList: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import { databaseList } from "@/ipc/api";
import type { DatabaseList } from "@/ipc/api";

const dbListMock = vi.mocked(databaseList);

/**
 * 主壳守卫（达芬奇式启动流，老语义恢复）：设置未加载完成/注册表未拉到时
 * 空白等待；本会话未选数据库（databaseChosen 会话级标志，非持久——每次
 * 启动都先 /database-picker）或激活数据库无效 → 送 /database-picker；
 * 选完数据库才进主壳。首启无数据库由选择页直送 /onboarding（不在本守卫）。
 */

function databases(count = 1): DatabaseList {
  return {
    databases: Array.from({ length: count }, (_, i) => ({
      id: `db-${i + 1}`,
      name: `数据库 ${i + 1}`,
      dbDir: `D:\\db-${i + 1}`,
    })),
    activeId: "db-1",
  };
}

function GalleryProbe() {
  return <div data-testid="gallery-probe">GALLERY</div>;
}

function PickerProbe() {
  return <div data-testid="picker-probe">PICKER</div>;
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
          <Route path="/database-picker" element={<PickerProbe />} />
          <Route path="/onboarding" element={<OnboardingProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  dbListMock.mockReset().mockResolvedValue(databases());
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    databaseChosen: false,
  });
});

describe("GatedShell（未选数据库或激活库无效 → 选择页；否则直进主壳）", () => {
  it("设置未加载完成时空白等待", () => {
    useSettingsStore.setState({ loaded: false });
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("picker-probe")).not.toBeInTheDocument();
  });

  it("注册表未拉到（首帧）时空白等待", () => {
    dbListMock.mockReturnValue(new Promise<DatabaseList>(() => {})); // 永不 resolve
    renderAtGallery();

    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(screen.queryByTestId("picker-probe")).not.toBeInTheDocument();
  });

  it("每次启动（会话未选数据库）都先送 /database-picker", async () => {
    renderAtGallery();

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
  });

  it("激活数据库无效（activeId=null 或不在注册表）→ 送 /database-picker", async () => {
    useSettingsStore.setState({ databaseChosen: true });
    dbListMock.mockResolvedValue({ databases: databases().databases, activeId: null });
    renderAtGallery();

    expect(await screen.findByTestId("picker-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
  });

  it("本会话已选数据库且激活库有效 → 直进主壳", async () => {
    useSettingsStore.setState({ databaseChosen: true });
    renderAtGallery();

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("picker-probe")).not.toBeInTheDocument();
  });
});
