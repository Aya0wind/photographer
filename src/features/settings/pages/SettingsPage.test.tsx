import { describe, expect, it, vi } from "vitest";

import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import SettingsPage from "./SettingsPage";

/**
 * 整体 mock settingsStore 模块（不改 store 源码）。
 * 变量名以 mock 开头，vi.mock 提升后仍可引用。
 * 组件实际只消费 settings.activeLibraryId / settings.libraries。
 */
interface MockLibrary {
  id: string;
  name: string;
  dbDir: string;
  photoRoot: string;
}

interface MockStoreState {
  settings: { libraries: MockLibrary[]; activeLibraryId: string | null };
}

const mockState: MockStoreState = {
  settings: { libraries: [], activeLibraryId: null },
};

vi.mock("@/stores/settingsStore", () => ({
  useSettingsStore: (selector: (s: MockStoreState) => unknown) => selector(mockState),
}));

const LIBRARY: MockLibrary = {
  id: "lib-1",
  name: "主库",
  dbDir: "D:\\SmartPhoto\\db",
  photoRoot: "D:\\Photos",
};

function setStore(settings: MockStoreState["settings"]): void {
  mockState.settings = settings;
}

function renderSettingsPage() {
  return render(
    <I18nextProvider i18n={i18n}>
      <SettingsPage />
    </I18nextProvider>,
  );
}

/** 三行信息（当前库/照片存储目录/数据库目录）；返回每行根 div */
function infoRows(): Record<string, HTMLElement> {
  const labels = ["当前库", "照片存储目录", "数据库目录"] as const;
  const rows = {} as Record<string, HTMLElement>;
  for (const label of labels) {
    rows[label] = screen.getByText(label).closest("div") as HTMLElement;
  }
  return rows;
}

describe("SettingsPage", () => {
  it("无激活库时三行信息均渲染且值为空（未设置态）", () => {
    setStore({ libraries: [], activeLibraryId: null });
    renderSettingsPage();

    expect(screen.getByText("设置")).toBeInTheDocument();
    const rows = infoRows();
    // 行内仅有标签文本，值为空
    expect(rows["当前库"].textContent).toBe("当前库");
    expect(rows["照片存储目录"].textContent).toBe("照片存储目录");
    expect(rows["数据库目录"].textContent).toBe("数据库目录");
    expect(screen.queryByText("主库")).not.toBeInTheDocument();
    expect(screen.queryByText("D:\\Photos")).not.toBeInTheDocument();
  });

  it("激活库存在时显示库信息（名称/照片目录/数据库目录）", () => {
    setStore({ libraries: [LIBRARY], activeLibraryId: "lib-1" });
    renderSettingsPage();

    expect(screen.getByText("主库")).toBeInTheDocument();
    expect(screen.getByText("D:\\Photos")).toBeInTheDocument();
    expect(screen.getByText("D:\\SmartPhoto\\db")).toBeInTheDocument();
    expect(screen.getByText("当前库")).toBeInTheDocument();
  });

  it("activeLibraryId 指向不存在的库时回落为空（find ?? null 兜底路径）", () => {
    setStore({ libraries: [LIBRARY], activeLibraryId: "missing-id" });
    renderSettingsPage();

    const rows = infoRows();
    expect(rows["当前库"].textContent).toBe("当前库");
    expect(rows["照片存储目录"].textContent).toBe("照片存储目录");
    expect(rows["数据库目录"].textContent).toBe("数据库目录");
    expect(screen.queryByText("主库")).not.toBeInTheDocument();
  });
});
