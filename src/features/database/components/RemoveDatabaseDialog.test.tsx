import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import RemoveDatabaseDialog from "./RemoveDatabaseDialog";
import { databaseRemove } from "@/ipc/api";
import type { DatabaseEntry } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, databaseRemove: vi.fn() };
});

const removeMock = vi.mocked(databaseRemove);

const DB: DatabaseEntry = {
  id: "db-x",
  name: "测试库",
  dbDir: "D:\\databases\\测试库",
};

function renderRemove(database: DatabaseEntry = DB) {
  const onRemoved = vi.fn();
  const onClose = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <RemoveDatabaseDialog database={database} onClose={onClose} onRemoved={onRemoved} />
    </I18nextProvider>,
  );
  return { onRemoved, onClose, unmount: view.unmount };
}

beforeEach(() => {
  removeMock.mockReset().mockResolvedValue({ dataDirsDeleted: 0 });
});

describe("删除数据库对话框（老 DeleteLibraryDialog 形态 + 两档内核）", () => {
  it("名字不匹配：确认禁用；逐字匹配后可点", () => {
    renderRemove();
    const confirm = screen.getByTestId("delete-database-confirm");
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByTestId("delete-database-input"), { target: { value: "测试" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByTestId("delete-database-input"), { target: { value: "测试库" } });
    expect(confirm).not.toBeDisabled();
  });

  it("默认不勾选：仅摘登记（deleteData=false）；勾选后连数据目录删（true）", async () => {
    const first = renderRemove();
    fireEvent.change(screen.getByTestId("delete-database-input"), { target: { value: "测试库" } });
    await userEvent.click(screen.getByTestId("delete-database-confirm"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith(DB.id, false));
    await waitFor(() => expect(first.onClose).toHaveBeenCalled());
    expect(first.onRemoved).toHaveBeenCalledWith({ dataDirsDeleted: 0 });

    first.unmount();
    renderRemove();
    fireEvent.click(screen.getByTestId("delete-database-data"));
    fireEvent.change(screen.getByTestId("delete-database-input"), { target: { value: "测试库" } });
    fireEvent.click(screen.getByTestId("delete-database-confirm"));
    await waitFor(() => expect(removeMock).toHaveBeenLastCalledWith(DB.id, true));
  });

  it("红线文案：绝不删除照片库文件夹", () => {
    renderRemove();
    expect(screen.getByTestId("delete-database-never")).toHaveTextContent("绝不会删除照片库");
  });

  it("后端失败：模态提示、删除对话框保留", async () => {
    removeMock.mockRejectedValue(new Error("数据目录与照片库重叠"));
    const { onClose } = renderRemove();
    fireEvent.change(screen.getByTestId("delete-database-input"), { target: { value: "测试库" } });
    fireEvent.click(screen.getByTestId("delete-database-confirm"));
    expect(await screen.findByRole("alertdialog")).toHaveTextContent("数据目录与照片库重叠");
    expect(screen.getByTestId("delete-database-dialog")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "确定" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(onClose).not.toHaveBeenCalled();
  });
});
