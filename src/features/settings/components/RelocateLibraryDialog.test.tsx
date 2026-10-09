import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import RelocateLibraryDialog from "./RelocateLibraryDialog";
import { photoLibraryRelocate } from "@/ipc/api";
import type { PhotoLibrary } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return { ...actual, photoLibraryRelocate: vi.fn() };
});

const relocateMock = vi.mocked(photoLibraryRelocate);

// 照片库登记项（2026-10-09 单库多照片库：rootPath 替代 photoRoot，归属不变）
const LIB: PhotoLibrary = {
  id: "lib-x",
  name: "测试照片库",
  rootPath: "Y:\\Old",
  createdAt: "2026-10-09T00:00:00Z",
  status: "online",
  assetCount: 15,
  sizeBytes: 1024,
};

function renderDialog(lib: PhotoLibrary = LIB) {
  const onClose = vi.fn();
  render(
    <I18nextProvider i18n={i18n}>
      <RelocateLibraryDialog library={lib} onClose={onClose} />
    </I18nextProvider>,
  );
  return { onClose };
}

beforeEach(() => {
  relocateMock.mockReset();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("RelocateLibraryDialog 整体重定位", () => {
  it("输入新根 → 600ms 防抖预检（apply=false）显示受影响计数与新根缺失提示", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    relocateMock.mockResolvedValue({ affected: 12, unaffected: 3, rootExists: false });
    renderDialog();
    expect(screen.getByText("Y:\\Old")).toBeInTheDocument();

    fireEvent.change(screen.getByTestId("relocate-new-root"), { target: { value: "I:\\New" } });
    expect(relocateMock).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(700);
    await waitFor(() => expect(screen.getByTestId("relocate-preview")).toBeInTheDocument());
    expect(relocateMock).toHaveBeenCalledWith(LIB.id, "I:\\New", false);
    expect(screen.getByTestId("relocate-preview").textContent).toContain("12");
    expect(screen.getByTestId("relocate-preview").textContent).toContain("新目录当前不存在");
  });

  it("未改动/预检报错 → 确认禁用并显示错误", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    renderDialog();
    expect(screen.getByTestId("relocate-confirm")).toBeDisabled();

    relocateMock.mockRejectedValue(new Error("路径必须是绝对路径：I:relative"));
    fireEvent.change(screen.getByTestId("relocate-new-root"), { target: { value: "I:relative" } });
    await vi.advanceTimersByTimeAsync(700);
    await waitFor(() => expect(screen.getByTestId("relocate-error")).toBeInTheDocument());
    expect(screen.getByTestId("relocate-error").textContent).toContain("绝对路径");
    expect(screen.getByTestId("relocate-confirm")).toBeDisabled();
  });

  it("确认 → apply=true → 完成面板（登记变更由 photoLibrariesChanged 事件驱动刷新）", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    relocateMock
      .mockResolvedValueOnce({ affected: 12, unaffected: 0, rootExists: true })
      .mockResolvedValueOnce({ affected: 12, unaffected: 0, rootExists: true });
    renderDialog();
    fireEvent.change(screen.getByTestId("relocate-new-root"), { target: { value: "I:\\New" } });
    await vi.advanceTimersByTimeAsync(700);
    await waitFor(() => expect(screen.getByTestId("relocate-preview")).toBeInTheDocument());

    fireEvent.click(screen.getByTestId("relocate-confirm"));
    await waitFor(() => expect(screen.getByTestId("relocate-done")).toBeInTheDocument());
    expect(relocateMock).toHaveBeenLastCalledWith(LIB.id, "I:\\New", true);
    expect(screen.getByTestId("relocate-done").textContent).toContain("I:\\New");
  });
});
