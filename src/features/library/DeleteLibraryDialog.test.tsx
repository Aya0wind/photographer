import { beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import DeleteLibraryDialog from "./DeleteLibraryDialog";
import NewLibraryDialog from "./NewLibraryDialog";
import { libraryDelete } from "@/ipc/api";
import { useSettingsStore, type Library } from "@/stores/settingsStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, libraryDelete: vi.fn() };
});

const deleteMock = vi.mocked(libraryDelete);

const LIB: Library = {
  id: "lib-x",
  name: "测试库",
  dbDir: "I:\\SmartPhoto\\测试库",
  photoRoot: "Y:\\照片",
  dirTemplate: "{YYYY}/{MM-DD}",
  importSubdir: "SmartPhoto",
  streams: 4,
  configured: true,
};

function renderDelete(lib: Library = LIB) {
  const onDeleted = vi.fn();
  const onClose = vi.fn();
  const view = render(
    <I18nextProvider i18n={i18n}>
      <DeleteLibraryDialog library={lib} onClose={onClose} onDeleted={onDeleted} />
    </I18nextProvider>,
  );
  return { onDeleted, onClose, unmount: view.unmount };
}

beforeEach(() => {
  deleteMock.mockReset().mockResolvedValue({ dbDeleted: true, photoRootDeleted: false });
  useSettingsStore.setState({
    settings: {
      ...useSettingsStore.getState().settings,
      libraries: [LIB],
      activeLibraryId: null,
    },
    libraryChosen: false,
  });
});

describe("删除库对话框", () => {
  it("名字不匹配：确认禁用；逐字匹配后可点", () => {
    renderDelete();
    const confirm = screen.getByTestId("delete-library-confirm");
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试" } });
    expect(confirm).toBeDisabled();
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试库" } });
    expect(confirm).not.toBeDisabled();
  });

  it("默认不勾选：不传照片目录；勾选后随负载传递", async () => {
    const user = userEvent.setup();
    const first = renderDelete();
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试库" } });
    await user.click(screen.getByTestId("delete-library-confirm"));
    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith(LIB.dbDir, undefined));
    await waitFor(() => expect(first.onClose).toHaveBeenCalled());

    first.unmount();
    renderDelete();
    fireEvent.click(screen.getByTestId("delete-library-photos"));
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试库" } });
    await user.click(screen.getByTestId("delete-library-confirm"));
    await waitFor(() => expect(deleteMock).toHaveBeenLastCalledWith(LIB.dbDir, LIB.photoRoot));
  });

  it("删除成功：注册表移除该库（含激活态清除）+ onDeleted 回调", async () => {
    useSettingsStore.setState({
      settings: {
        ...useSettingsStore.getState().settings,
        activeLibraryId: "lib-x",
      },
    });
    const { onDeleted } = renderDelete();
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试库" } });
    fireEvent.click(screen.getByTestId("delete-library-confirm"));
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    const s = useSettingsStore.getState().settings;
    expect(s.libraries).toHaveLength(0);
    expect(s.activeLibraryId).toBeNull();
  });

  it("后端失败：错误行内展示、对话框保留", async () => {
    deleteMock.mockRejectedValue(new Error("目录不像库数据目录"));
    renderDelete();
    fireEvent.change(screen.getByTestId("delete-library-input"), { target: { value: "测试库" } });
    fireEvent.click(screen.getByTestId("delete-library-confirm"));
    expect(await screen.findByTestId("delete-library-error")).toHaveTextContent("目录不像库数据目录");
    expect(screen.getByTestId("delete-library-dialog")).toBeInTheDocument();
  });
});

describe("新建库重名闸", () => {
  it("已存在同名：禁建 + 行内提示", async () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <NewLibraryDialog open onClose={() => undefined} />
        </MemoryRouter>
      </I18nextProvider>,
    );
    const nameInput = await screen.findByDisplayValue("主库");
    await userEvent.clear(nameInput);
    await userEvent.type(nameInput, "测试库");
    expect(screen.getByText("已存在同名库，请换一个名称")).toBeInTheDocument();
    expect(screen.getByTestId("new-library-submit")).toBeDisabled();
  });
});
