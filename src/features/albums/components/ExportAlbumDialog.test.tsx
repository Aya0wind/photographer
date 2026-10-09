import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import ExportAlbumDialog from "./ExportAlbumDialog";
import { LAST_EXPORT_DIR_KEY } from "@/features/albums/lib/exportTarget";
import {
  albumExportCancel,
  albumExportRun,
  albumExportStatus,
  photoLibraryList,
  subscribeAppEvents,
  type AlbumExportTask,
  type AppEvent,
  type PhotoLibrary,
} from "@/ipc/api";

/**
 * 相册/子组「导出为文件夹」对话框（M6 导出前端，计划 §六）：
 * - 默认建议库外路径 = 记住上次位置（localStorage）
 * - 目标在任一照片库内 → 提示「会被扫描忽略…建议选择照片库外」，不禁止
 * - 进度（albumExportProgress）与总结（albumExportFinished；跳过源缺失成员说明）
 * IPC 全按 P0 契约 mock（后端实现并行进行中，汇合后联调）。
 */

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    albumExportRun: vi.fn(),
    albumExportStatus: vi.fn(),
    albumExportCancel: vi.fn(),
    photoLibraryList: vi.fn(),
    subscribeAppEvents: vi.fn(),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const runMock = vi.mocked(albumExportRun);
const statusMock = vi.mocked(albumExportStatus);
const cancelMock = vi.mocked(albumExportCancel);
const listMock = vi.mocked(photoLibraryList);
const subscribeMock = vi.mocked(subscribeAppEvents);

/** 捕获的事件 handler（进度/收尾用例驱动） */
let eventHandlers: Array<(event: AppEvent) => void> = [];

/** 广播一条 app://event */
function emit(event: AppEvent): void {
  for (const handler of eventHandlers) handler(event);
}

function lib(overrides: Partial<PhotoLibrary> = {}): PhotoLibrary {
  return {
    id: "lib-1",
    name: "主照片库",
    rootPath: "I:\\Photos",
    createdAt: "2026-10-09T00:00:00Z",
    status: "online",
    assetCount: 10,
    sizeBytes: 0,
    ...overrides,
  };
}

function task(overrides: Partial<AlbumExportTask> = {}): AlbumExportTask {
  return {
    id: 7,
    albumId: 3,
    subgroup: null,
    outputDir: "D:\\LR Export",
    status: "running",
    total: 10,
    done: 0,
    linked: 0,
    error: null,
    ...overrides,
  };
}

function renderDialog(props: Partial<Parameters<typeof ExportAlbumDialog>[0]> = {}) {
  return render(
    <I18nextProvider i18n={i18n}>
      <ExportAlbumDialog
        albumId={3}
        albumName="婚礼跟拍"
        subgroup={null}
        itemCount={10}
        onClose={() => {}}
        {...props}
      />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  // run 缺省给一个成功响应：未显式设返回值的用例（只验调用参数）不产生悬空 promise
  runMock.mockReset().mockResolvedValue({ ok: true, task: task() });
  statusMock.mockReset().mockResolvedValue(null);
  cancelMock.mockReset().mockResolvedValue(undefined);
  listMock.mockReset().mockResolvedValue([lib()]);
  eventHandlers = [];
  subscribeMock.mockReset().mockImplementation(async (handler) => {
    eventHandlers.push(handler);
    return () => {};
  });
  localStorage.removeItem(LAST_EXPORT_DIR_KEY);
});

describe("配置段：默认建议与库内提示", () => {
  it("默认建议上次导出位置（localStorage 预填）；无记忆时空输入", async () => {
    localStorage.setItem(LAST_EXPORT_DIR_KEY, JSON.stringify("D:\\LR Export"));
    const first = renderDialog();
    expect(screen.getByTestId("album-export-dir")).toHaveValue("D:\\LR Export");
    first.unmount();

    // 无记忆：空输入 + 确认禁用
    localStorage.removeItem(LAST_EXPORT_DIR_KEY);
    renderDialog();
    await waitFor(() =>
      expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "config"),
    );
    expect(screen.getByTestId("album-export-dir")).toHaveValue("");
    expect(screen.getByTestId("album-export-confirm")).toBeDisabled();
  });

  it("目标在照片库内 → 提示会被扫描忽略（不禁止，允许继续导出）", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "I:\\Photos\\给LR");
    const hint = await screen.findByTestId("album-export-inside-hint");
    expect(hint).toHaveTextContent("该文件夹会被扫描忽略");
    expect(hint).toHaveTextContent("建议选择照片库外");
    expect(hint).toHaveAttribute("data-library", "主照片库");

    // 允许继续：确认按钮可用，照常发起导出
    await user.click(screen.getByTestId("album-export-confirm"));
    await waitFor(() =>
      expect(runMock).toHaveBeenCalledWith(3, null, "I:\\Photos\\给LR"),
    );
  });

  it("库外路径不提示", async () => {
    const user = userEvent.setup();
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await waitFor(() =>
      expect(screen.getByTestId("album-export-confirm")).not.toBeDisabled(),
    );
    expect(screen.queryByTestId("album-export-inside-hint")).not.toBeInTheDocument();
  });

  it("子组作用域：albumExportRun(albumId, subgroup, dir) 透传子分组名", async () => {
    const user = userEvent.setup();
    renderDialog({ subgroup: "精选" });

    expect(screen.getByTestId("album-export-scope")).toHaveTextContent("婚礼跟拍 ‹ 精选");
    expect(screen.getByTestId("album-export-scope")).toHaveTextContent("共 10 张");

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    await waitFor(() => expect(runMock).toHaveBeenCalledWith(3, "精选", "D:\\LR Export"));
  });

  it("启动成功即记住本次位置（下次默认建议）", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: true, task: task({ status: "done", done: 10, linked: 10 }) });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    await screen.findByTestId("album-export-summary");
    expect(localStorage.getItem(LAST_EXPORT_DIR_KEY)).toBe(JSON.stringify("D:\\LR Export"));
  });

  it("启动业务错误：后端 Err 文案透传；error=null 提示后端未连接", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: false, error: "目标文件夹不可写" });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    expect(await screen.findByTestId("album-export-error")).toHaveTextContent("目标文件夹不可写");
    expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "config");

    runMock.mockResolvedValue({ ok: false, error: null });
    await user.click(screen.getByTestId("album-export-confirm"));
    expect(await screen.findByTestId("album-export-error")).toHaveTextContent("后端未连接");
  });
});

describe("进度段与总结", () => {
  it("进度事件推进 done/total；取消走软信号；完成总结含硬链接/拷贝拆分", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: true, task: task() });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));

    const progress = await screen.findByTestId("album-export-progress");
    expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "running");
    expect(progress).toHaveAttribute("data-total", "10");
    expect(screen.getByTestId("album-export-output")).toHaveTextContent("D:\\LR Export");

    emit({ type: "albumExportProgress", taskId: 7, done: 5, total: 10 });
    await waitFor(() =>
      expect(screen.getByTestId("album-export-progress")).toHaveAttribute("data-done", "5"),
    );

    await user.click(screen.getByTestId("album-export-cancel"));
    await waitFor(() => expect(cancelMock).toHaveBeenCalledTimes(1));

    emit({
      type: "albumExportFinished",
      taskId: 7,
      ok: true,
      exported: 10,
      linked: 7,
    });
    const summary = await screen.findByTestId("album-export-summary");
    expect(summary).toHaveAttribute("data-ok", "true");
    expect(within(summary).getByTestId("album-export-summary-title")).toHaveTextContent("导出完成");
    expect(within(summary).getByTestId("album-export-summary-exported")).toHaveTextContent("成功导出 10 张");
    expect(within(summary).getByTestId("album-export-summary-linked")).toHaveTextContent("7 张硬链接");
    expect(within(summary).getByTestId("album-export-summary-linked")).toHaveTextContent("3 张拷贝");
    expect(within(summary).queryByTestId("album-export-summary-skipped")).not.toBeInTheDocument();
  });

  it("部分成员未导出：跳过源缺失说明（total-exported 差值）", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: true, task: task() });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    await screen.findByTestId("album-export-progress");

    emit({ type: "albumExportProgress", taskId: 7, done: 8, total: 10 });
    emit({
      type: "albumExportFinished",
      taskId: 7,
      ok: false,
      exported: 8,
      linked: 8,
    });

    const summary = await screen.findByTestId("album-export-summary");
    expect(summary).toHaveAttribute("data-ok", "false");
    expect(within(summary).getByTestId("album-export-summary-title")).toHaveTextContent("部分成员未导出");
    const skipped = within(summary).getByTestId("album-export-summary-skipped");
    expect(skipped).toHaveTextContent("跳过 2 张");
    expect(skipped).toHaveTextContent("源文件缺失或不可读");
  });

  it("收尾事件带错误文案：总结内联展示", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: true, task: task() });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    await screen.findByTestId("album-export-progress");

    emit({ type: "albumExportProgress", taskId: 7, done: 3, total: 10 });
    emit({
      type: "albumExportFinished",
      taskId: 7,
      ok: false,
      exported: 3,
      linked: 0,
      error: "磁盘已满",
    });

    const summary = await screen.findByTestId("album-export-summary");
    expect(within(summary).getByTestId("album-export-summary-error")).toHaveTextContent("磁盘已满");
    expect(within(summary).getByTestId("album-export-summary-skipped")).toHaveTextContent("跳过 7 张");
  });

  it("其他任务的进度/收尾事件不串扰（按 taskId 过滤）", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({ ok: true, task: task() });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));
    await screen.findByTestId("album-export-progress");

    emit({ type: "albumExportProgress", taskId: 999, done: 9, total: 9 });
    expect(screen.getByTestId("album-export-progress")).toHaveAttribute("data-done", "0");
    emit({ type: "albumExportFinished", taskId: 999, ok: true, exported: 9, linked: 9 });
    expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "running");
  });
});

describe("挂载与终态兜底", () => {
  it("挂载时同作用域任务已在跑（album_export_status）→ 直接进进度段", async () => {
    statusMock.mockResolvedValue(task({ id: 12, status: "queued", done: 2, total: 30 }));
    renderDialog({ subgroup: null });

    const progress = await screen.findByTestId("album-export-progress");
    expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "running");
    expect(progress).toHaveAttribute("data-done", "2");
    expect(progress).toHaveAttribute("data-total", "30");
  });

  it("挂载状态是他相册/他子组任务 → 不采纳，保持配置段", async () => {
    statusMock.mockResolvedValue(task({ albumId: 99, subgroup: null }));
    renderDialog();
    await waitFor(() => expect(statusMock).toHaveBeenCalled());
    expect(screen.getByTestId("album-export-dialog")).toHaveAttribute("data-phase", "config");
  });

  it("run 返回即终态（极小相册抢先完成）→ 直接出总结", async () => {
    const user = userEvent.setup();
    runMock.mockResolvedValue({
      ok: true,
      task: task({ status: "done", done: 4, linked: 4, total: 4 }),
    });
    renderDialog();

    await user.type(screen.getByTestId("album-export-dir"), "D:\\LR Export");
    await user.click(screen.getByTestId("album-export-confirm"));

    const summary = await screen.findByTestId("album-export-summary");
    expect(within(summary).getByTestId("album-export-summary-exported")).toHaveTextContent("成功导出 4 张");
    expect(within(summary).queryByTestId("album-export-summary-skipped")).not.toBeInTheDocument();
  });
});
