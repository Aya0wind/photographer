import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes, useSearchParams } from "react-router";

import i18n from "@/i18n";
import DatabasePickerPage from "./DatabasePickerPage";
import {
  DEFAULT_SETTINGS,
  clone,
  useSettingsStore,
} from "@/stores/settingsStore";
import type { DatabaseList } from "@/ipc/api";

// 注册表/切换命令按契约 mock；resetLibrarySession mock 便于断言清会话时机
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseList: vi.fn(),
    databaseSwitch: vi.fn(),
    databaseRemove: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});
vi.mock("@/lib/librarySession", () => ({ resetLibrarySession: vi.fn() }));

import { databaseList, databaseSwitch } from "@/ipc/api";
import { resetLibrarySession } from "@/lib/librarySession";

const listMock = vi.mocked(databaseList);
const switchMock = vi.mocked(databaseSwitch);
const resetMock = vi.mocked(resetLibrarySession);

const DB_A = { id: "db-1", name: "主库", dbDir: "D:\\SmartPhoto\\主库" };
const DB_B = { id: "db-2", name: "工作库", dbDir: "E:\\db2" };

function databases(dbs = [DB_A, DB_B], activeId: string | null = "db-1"): DatabaseList {
  return { databases: dbs, activeId };
}

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
      <MemoryRouter initialEntries={["/database-picker"]}>
        <Routes>
          <Route path="/database-picker" element={<DatabasePickerPage />} />
          <Route path="/onboarding" element={<OnboardingProbe />} />
          <Route path="/gallery" element={<GalleryProbe />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

function setRegistry(snapshot: DatabaseList): void {
  listMock.mockResolvedValue(snapshot);
}

beforeEach(() => {
  switchMock.mockReset().mockResolvedValue(undefined);
  resetMock.mockReset();
  setRegistry(databases());
  useSettingsStore.setState({
    settings: clone(DEFAULT_SETTINGS),
    loaded: true,
    databaseChosen: false,
  });
});

describe("DatabasePickerPage（达芬奇式启动首屏）", () => {
  it("渲染数据库卡片：名称/dbDir + 上次使用标记 + 激活库预选", async () => {
    renderPicker();

    const cards = await screen.findAllByTestId("database-picker-card");
    expect(cards).toHaveLength(2);
    const cardA = cards.find((el) => el.getAttribute("data-db-id") === "db-1");
    const cardB = cards.find((el) => el.getAttribute("data-db-id") === "db-2");
    expect(cardA).toHaveTextContent("主库");
    expect(cardA).toHaveTextContent("D:\\SmartPhoto\\主库");
    expect(cardA).toHaveTextContent("上次使用");
    // 数据源 useDatabases 异步到位：激活库预选在 effect 修正帧生效
    await waitFor(() => expect(cardA).toHaveAttribute("data-selected", "true"));
    expect(cardB).toHaveAttribute("data-selected", "false");
    // DatabaseEntry 无 photoRoot/configured：副信息只显示 dbDir
    expect(cardA).not.toHaveTextContent("E:\\照片");
  });

  it("选中+打开：databaseSwitch 下发、置 databaseChosen 并进入主界面（跨库先清会话）", async () => {
    renderPicker();
    const user = userEvent.setup();

    const cardB = (await screen.findAllByTestId("database-picker-card")).find(
      (el) => el.getAttribute("data-db-id") === "db-2",
    );
    await user.click(cardB!);
    expect(cardB).toHaveAttribute("data-selected", "true");
    await user.click(screen.getByTestId("database-picker-open"));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    await waitFor(() => expect(switchMock).toHaveBeenCalledWith("db-2"));
    expect(useSettingsStore.getState().databaseChosen).toBe(true);
    // 目标不是当前激活库：切换前清会话态（防跨库张冠李戴）
    expect(resetMock).toHaveBeenCalledTimes(1);
  });

  it("双击数据库卡片直接进入", async () => {
    renderPicker();
    const user = userEvent.setup();

    await user.dblClick((await screen.findAllByTestId("database-picker-card"))[0]);

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(switchMock).toHaveBeenCalledWith("db-1");
  });

  it("打开当前激活库：不再清会话（同库重进）", async () => {
    renderPicker();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("database-picker-open"));

    expect(await screen.findByTestId("gallery-probe")).toBeInTheDocument();
    expect(resetMock).not.toHaveBeenCalled();
  });

  it("切换失败：后端 Err 文案内联展示、留在选择页", async () => {
    switchMock.mockRejectedValue(new Error("导入进行中：请完成后再切换数据库"));
    renderPicker();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("database-picker-open"));

    expect(await screen.findByTestId("database-picker-error")).toHaveTextContent(
      "导入进行中：请完成后再切换数据库",
    );
    expect(screen.queryByTestId("gallery-probe")).not.toBeInTheDocument();
    expect(useSettingsStore.getState().databaseChosen).toBe(false);
  });

  it("无任何数据库：直接进入新建数据库向导", async () => {
    setRegistry(databases([], null));
    renderPicker();

    expect(await screen.findByTestId("onboarding-probe")).toBeInTheDocument();
    expect(screen.queryByTestId("database-picker-grid")).not.toBeInTheDocument();
  });

  it("「新建数据库」按钮进入向导（不改变当前激活状态）", async () => {
    renderPicker();
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("database-picker-create"));

    expect(await screen.findByTestId("onboarding-probe")).toHaveTextContent("NEW");
    expect(switchMock).not.toHaveBeenCalled();
    expect(useSettingsStore.getState().databaseChosen).toBe(false);
  });

  it("设置未加载完成或注册表未拉到时等待（不闪空态直送）", () => {
    useSettingsStore.setState({ loaded: false, databaseChosen: false });
    const { container } = renderPicker();

    expect(container.querySelector("[data-testid='database-picker-grid']")).toBeNull();
    expect(screen.queryByTestId("onboarding-probe")).not.toBeInTheDocument();
  });

  it("卡片垃圾桶打开删除对话框（仅摘登记 / 连数据目录删两档）", async () => {
    const { databaseRemove } = await import("@/ipc/api");
    const removeMock = vi.mocked(databaseRemove);
    removeMock.mockResolvedValue({ dataDirsDeleted: 0 });
    renderPicker();
    const user = userEvent.setup();

    const cardA = (await screen.findAllByTestId("database-picker-card")).find(
      (el) => el.getAttribute("data-db-id") === "db-1",
    );
    await user.click(cardA!.querySelector("[data-testid='database-picker-delete']")!);
    expect(screen.getByTestId("delete-database-dialog")).toBeInTheDocument();

    await user.type(screen.getByTestId("delete-database-input"), "主库");
    await user.click(screen.getByTestId("delete-database-confirm"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith("db-1", false));
    await waitFor(() =>
      expect(screen.queryByTestId("delete-database-dialog")).not.toBeInTheDocument(),
    );
  });

  it("渲染自绘标题栏（主壳外全屏页：窗口可拖动/控制）", async () => {
    renderPicker();

    expect(await screen.findByTestId("titlebar")).toBeInTheDocument();
    expect(screen.getByTestId("titlebar-drag-region")).toHaveAttribute("data-tauri-drag-region");
    expect(screen.getByTestId("titlebar-close")).toBeInTheDocument();
    // 无菜单内容（菜单只在主壳 TitleBar 中）
    expect(screen.queryByTestId("menubar")).not.toBeInTheDocument();
  });
});
