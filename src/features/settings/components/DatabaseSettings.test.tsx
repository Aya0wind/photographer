import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";

// ipc mock：数据库卡片走 database_* 命令（useDatabases 拉 database_list）
vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    databaseList: vi.fn(),
    databaseSwitch: vi.fn(),
    databaseCreate: vi.fn(),
    databaseRemove: vi.fn(),
    subscribeAppEvents: vi.fn(async () => () => {}),
  };
});

import i18n from "@/i18n";
import DatabaseSettings from "./DatabaseSettings";
import {
  databaseList,
  databaseRemove,
  databaseSwitch,
} from "@/ipc/api";

const listMock = vi.mocked(databaseList);
const switchMock = vi.mocked(databaseSwitch);
const removeMock = vi.mocked(databaseRemove);

/**
 * 设置页数据库卡片（2026-10-09 多数据库修正）：当前库名下拉切换
 * database_switch、新建/移除对话框入口、切换失败文案透传。
 * 切换成功后的全量刷新由 databasesChanged 事件驱动（AppShell 重挂内容区，
 * 见 AppShell.databaseRefresh.test.tsx），本文件只验证卡片自身行为。
 */

const DB_A = { id: "db-1", name: "主数据库", dbDir: "D:\\db-a" };
const DB_B = { id: "db-2", name: "协作库", dbDir: "E:\\db-b" };

function renderCard() {
  return render(
    <I18nextProvider i18n={i18n}>
      <DatabaseSettings />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  listMock.mockReset().mockResolvedValue({
    databases: [DB_A, DB_B],
    activeId: DB_A.id,
  });
  switchMock.mockReset().mockResolvedValue(undefined);
  removeMock.mockReset().mockResolvedValue({ dataDirsDeleted: 0 });
});

describe("DatabaseSettings（设置页数据库卡片）", () => {
  it("展示当前库与全部注册表条目；切换下拉下发 database_switch", async () => {
    renderCard();
    const select = await screen.findByTestId("settings-database-select");
    expect(select).toHaveValue(DB_A.id);
    expect(screen.getByTestId("settings-database-dir")).toHaveTextContent(DB_A.dbDir);

    await userEvent.setup().selectOptions(select, DB_B.id);
    await waitFor(() => expect(switchMock).toHaveBeenCalledWith(DB_B.id));
  });

  it("切换失败：后端 Err 文案内联展示", async () => {
    switchMock.mockRejectedValue(new Error("导入进行中：请完成后再切换数据库"));
    renderCard();
    const select = await screen.findByTestId("settings-database-select");
    await userEvent.setup().selectOptions(select, DB_B.id);
    expect(await screen.findByTestId("settings-database-error")).toHaveTextContent(
      "导入进行中：请完成后再切换数据库",
    );
  });

  it("新建入口弹出创建对话框（名称必填 → databaseCreate）", async () => {
    const { databaseCreate } = await import("@/ipc/api");
    const createMock = vi.mocked(databaseCreate);
    createMock.mockResolvedValue({
      ok: true,
      database: { id: "db-3", name: "新库", dbDir: "F:\\db-3" },
    });
    const user = userEvent.setup();
    renderCard();

    await user.click(await screen.findByTestId("settings-database-new"));
    const dialog = screen.getByTestId("database-create-dialog");
    expect(dialog).toBeInTheDocument();
    // 名称必填：未填不能确认
    expect(screen.getByTestId("database-create-confirm")).toBeDisabled();
    await user.type(screen.getByTestId("database-create-name"), "新库");
    await user.click(screen.getByTestId("database-create-confirm"));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith("新库", undefined));
    // 创建成功对话框关闭（databasesChanged 事件驱动列表重拉）
    await waitFor(() =>
      expect(screen.queryByTestId("database-create-dialog")).not.toBeInTheDocument(),
    );
  });

  it("移除入口弹出两档确认对话框（仅摘登记 / 连数据目录删）", async () => {
    removeMock.mockResolvedValue({ dataDirsDeleted: 1 });
    const user = userEvent.setup();
    renderCard();

    await user.click(await screen.findByTestId("settings-database-remove"));
    expect(screen.getByTestId("database-remove-dialog")).toBeInTheDocument();
    expect(screen.getByTestId("database-remove-never-delete")).toHaveTextContent("绝不会删除");

    await user.click(screen.getByTestId("database-remove-keep-data"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith(DB_A.id, false));

    // 两档之二：连数据目录删
    await user.click(await screen.findByTestId("settings-database-remove"));
    await user.click(screen.getByTestId("database-remove-delete-data"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith(DB_A.id, true));
  });

  it("注册表为空：展示「尚无数据库」占位，移除按钮禁用", async () => {
    listMock.mockResolvedValue({ databases: [], activeId: null });
    renderCard();
    expect(await screen.findByTestId("settings-database-select")).toBeInTheDocument();
    expect(screen.getByTestId("settings-database-select")).toBeDisabled();
    expect(screen.getByTestId("settings-database-remove")).toBeDisabled();
    expect(screen.queryByTestId("settings-database-dir")).not.toBeInTheDocument();
  });
});
