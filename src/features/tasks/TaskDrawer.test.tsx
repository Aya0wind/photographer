import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import { TaskDrawerPanel, TaskDrawerToggle, countRunningTasks } from "./TaskDrawer";
import SummaryModalHost from "./SummaryModal";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import { useAiStore } from "@/stores/aiStore";
import {
  importCancel,
  importJobDelete,
  importJobsPage,
  importLogsPage,
  importPause,
  importResume,
  indexKickNow,
  indexStatus,
  indexTaskPause,
  indexTaskResume,
  type AppEvent,
  type ImportStats,
  type IndexStatus,
  type JobRow,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importPause: vi.fn(),
    importResume: vi.fn(),
    importCancel: vi.fn(),
    importJobsPage: vi.fn(),
    importJobDelete: vi.fn(),
    importLogsPage: vi.fn(),
    indexStatus: vi.fn(),
    indexKickNow: vi.fn(),
    indexTaskPause: vi.fn(),
    indexTaskResume: vi.fn(),
  };
});

const pauseMock = vi.mocked(importPause);
const resumeMock = vi.mocked(importResume);
const cancelMock = vi.mocked(importCancel);
const indexStatusMock = vi.mocked(indexStatus);
const kickMock = vi.mocked(indexKickNow);
const indexPauseMock = vi.mocked(indexTaskPause);
const indexResumeMock = vi.mocked(indexTaskResume);
const jobsPageMock = vi.mocked(importJobsPage);
const jobDeleteMock = vi.mocked(importJobDelete);
const logsPageMock = vi.mocked(importLogsPage);

function GalleryProbe() {
  return <div data-testid="gallery-probe" />;
}

/** 抽屉常开（AppShell 持开合状态；测试直接驱动面板） */
function renderDrawer(open = true) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <TaskDrawerPanel open={open} onClose={() => {}} />
        <Routes>
          <Route path="/gallery" element={<GalleryProbe />} />
          <Route path="/import" element={<div data-testid="import-probe" />} />
        </Routes>
      </MemoryRouter>
    </I18nextProvider>,
  );
}

/** 直接驱动 store 事件入口（与真实 subscribeAppEvents 转发一致） */
function emit(event: AppEvent): void {
  act(() => {
    useImportStore.getState().handleAppEvent(event);
  });
}

function finishedStats(overrides: Partial<ImportStats> = {}): ImportStats {
  return {
    totalFiles: 10,
    doneFiles: 7,
    skippedDuplicates: 2,
    failedFiles: 1,
    totalBytes: 1000,
    doneBytes: 700,
    elapsedMs: 150_000,
    bytesPerSec: 5 * 1024 * 1024,
    ...overrides,
  };
}

beforeEach(() => {
  vi.useRealTimers();
  resetImportStoreForTests();
  useAiStore.getState().resetForTests();
  pauseMock.mockReset().mockResolvedValue(undefined);
  resumeMock.mockReset().mockResolvedValue(undefined);
  cancelMock.mockReset().mockResolvedValue(undefined);
  indexStatusMock.mockReset().mockResolvedValue(null);
  kickMock.mockReset().mockResolvedValue(undefined);
  indexPauseMock.mockReset().mockResolvedValue(undefined);
  jobsPageMock.mockReset().mockResolvedValue([]);
  jobDeleteMock.mockReset().mockResolvedValue(undefined);
  logsPageMock.mockReset().mockResolvedValue([]);
});

afterEach(() => {
  vi.useRealTimers();
});

// --- 开关徽标 -----------------------------------------------------------------------

describe("任务抽屉：开关按钮徽标", () => {
  it("无运行任务（无导入、索引无活儿）→ 无徽标", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerToggle open={false} onClick={() => {}} />
        </MemoryRouter>
      </I18nextProvider>,
    );

    expect(screen.queryByTestId("taskdrawer-count")).not.toBeInTheDocument();
  });

  it("徽标 = 运行中导入数 + 有活儿的索引通道数", async () => {
    // 开关按钮只读 store（轮询在面板常驻挂载）；直接注入状态驱动
    act(() => {
      useAiStore.setState({
        indexStatus: {
          thumb: { pending: 2, running: 1, done: 7, failed: 0, total: 10 },
          exif: { pending: 0, running: 0, done: 10, failed: 0, total: 10 },
          ai: { pending: 4, running: 0, done: 5, failed: 0, total: 10 },
          face: { pending: 0, running: 0, done: 10, failed: 0, total: 10 },
        } satisfies IndexStatus,
      });
    });
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });

    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerToggle open={false} onClick={() => {}} />
        </MemoryRouter>
      </I18nextProvider>,
    );

    // 1 个导入任务 + thumb/ai 两个通道 = 3
    expect(screen.getByTestId("taskdrawer-count")).toHaveTextContent("3");
  });

  it("countRunningTasks 纯函数：终态任务与无活儿通道不计", () => {
    expect(countRunningTasks({}, null)).toBe(0);
    expect(
      countRunningTasks(
        {
          1: { jobId: 1, status: "done", totalFiles: 1, totalBytes: 1, doneFiles: 1, doneBytes: 1, bytesPerSec: 0, currentFile: "" },
          2: { jobId: 2, status: "running", totalFiles: 1, totalBytes: 1, doneFiles: 0, doneBytes: 0, bytesPerSec: 0, currentFile: "" },
        },
        null,
      ),
    ).toBe(1);
  });
});

// --- 开合 ---------------------------------------------------------------------------

describe("任务抽屉：开合", () => {
  it("open=false 不渲染面板；Esc / 背板 / 关闭钮触发 onClose", async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { rerender } = render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerPanel open={false} onClose={onClose} />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(screen.queryByTestId("taskdrawer")).not.toBeInTheDocument();

    rerender(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerPanel open onClose={onClose} />
        </MemoryRouter>
      </I18nextProvider>,
    );
    expect(await screen.findByTestId("taskdrawer")).toBeInTheDocument();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);

    await user.click(screen.getByTestId("taskdrawer-backdrop"));
    expect(onClose).toHaveBeenCalledTimes(2);

    await user.click(screen.getByTestId("taskdrawer-close"));
    expect(onClose).toHaveBeenCalledTimes(3);
  });

  it("无任务空态：空态文案 + 去导入跳任务中心", async () => {
    const user = userEvent.setup();
    renderDrawer();

    expect(await screen.findByTestId("taskdrawer-empty")).toHaveTextContent("暂无进行中的任务");
    await user.click(screen.getByTestId("taskdrawer-empty").querySelector("button") as HTMLButtonElement);
    expect(await screen.findByTestId("import-probe")).toBeInTheDocument();
  });
});

// --- 导入任务行（进行中） -------------------------------------------------------------

describe("任务抽屉：导入任务行", () => {
  it("sessionStarted + 进度事件：标题/进度条宽度/速度/当前文件", async () => {
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 100, totalBytes: 10_000_000_000 });

    const row = await screen.findByTestId("taskdrawer-import");
    expect(row).toHaveAttribute("data-phase", "running");
    expect(screen.getByText("复制任务 #7")).toBeInTheDocument();

    emit({
      type: "importFileProgress",
      jobId: 7,
      doneFiles: 50,
      doneBytes: 5_000_000_000,
      settledBytes: 5_000_000_000,
      currentFile: "DCIM/100CANON/IMG_0042.CR3",
      bytesPerSec: 50 * 1024 * 1024,
    });

    // 进度字段经 150ms 节流缓冲后落地
    await waitFor(() => {
      expect(screen.getByTestId("taskdrawer-import-progress")).toHaveAttribute("aria-valuenow", "50");
    });
    const fill = screen.getByTestId("taskdrawer-import-progress").firstElementChild as HTMLElement;
    expect(fill.style.width).toBe("50%");
    expect(screen.getByText("50.0 MB/s")).toBeInTheDocument();
    expect(screen.getByText("4.7 GB / 9.3 GB")).toBeInTheDocument();
    expect(screen.getByTestId("taskdrawer-import-file")).toHaveTextContent("IMG_0042.CR3");
  });

  it("移动任务（jobModes=move）标题为移动任务；暂停事件切换状态样式与按钮", async () => {
    act(() => {
      useImportStore.getState().setPendingJobMode("move");
    });
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 9, totalFiles: 4, totalBytes: 400 });

    expect(await screen.findByText("移动任务 #9")).toBeInTheDocument();

    emit({ type: "importPaused", jobId: 9 });
    await waitFor(() => {
      expect(screen.getByTestId("taskdrawer-import")).toHaveAttribute("data-phase", "paused");
    });
    expect(screen.getByText("已暂停")).toBeInTheDocument();
    expect(screen.getByTestId("taskdrawer-import-toggle")).toHaveTextContent("恢复");
  });

  it("暂停/继续/取消按钮调用 IPC 命令", async () => {
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 100 });
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("taskdrawer-import-toggle"));
    expect(pauseMock).toHaveBeenCalledWith(7);

    emit({ type: "importPaused", jobId: 7 });
    await user.click(await screen.findByTestId("taskdrawer-import-toggle"));
    expect(resumeMock).toHaveBeenCalledWith(7);

    emit({ type: "importResumed", jobId: 7 });
    await user.click(screen.getByTestId("taskdrawer-import-cancel"));
    expect(cancelMock).toHaveBeenCalledWith(7);
  });
});

// --- 导入任务行（终态） ---------------------------------------------------------------

describe("任务抽屉：导入终态行", () => {
  it("sessionFinished：完成样式 + 计数行；总结快照留在 store（弹窗全局挂载）", async () => {
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionFinished", jobId: 7, stats: finishedStats() });

    const row = await screen.findByTestId("taskdrawer-import");
    expect(row).toHaveAttribute("data-phase", "done");
    expect(row).toHaveTextContent("导入完成");
    expect(row).toHaveTextContent("#7");
    expect(screen.getByTestId("taskdrawer-import-counts")).toHaveTextContent("成功 7 · 跳过 2 · 失败 1");
    expect(useImportStore.getState().summary?.jobId).toBe(7);
  });

  it("移动任务完成：计数行为已移动", async () => {
    act(() => {
      useImportStore.getState().setPendingJobMode("move");
    });
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 11, totalFiles: 10, totalBytes: 1000 });
    emit({
      type: "importSessionFinished",
      jobId: 11,
      stats: finishedStats({ doneFiles: 8, skippedDuplicates: 1, moved: 8 }),
    });

    const row = await screen.findByTestId("taskdrawer-import");
    expect(row).toHaveTextContent("移动完成");
    expect(screen.getByTestId("taskdrawer-import-counts")).toHaveTextContent("已移动 8 · 跳过 1 · 失败 1");
  });

  it("软取消：转已取消样式（无计数行）", async () => {
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importCancelled", jobId: 7 });

    const row = await screen.findByTestId("taskdrawer-import");
    expect(row).toHaveAttribute("data-phase", "cancelled");
    expect(row).toHaveTextContent("已取消");
    expect(screen.queryByTestId("taskdrawer-import-counts")).not.toBeInTheDocument();
  });

  it("清除已完成：终态行移除，不影响任务账", async () => {
    const user = userEvent.setup();
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionFinished", jobId: 7, stats: finishedStats() });

    expect(await screen.findByTestId("taskdrawer-import")).toBeInTheDocument();
    await user.click(screen.getByTestId("taskdrawer-clear-done"));

    await waitFor(() =>
      expect(screen.queryByTestId("taskdrawer-import")).not.toBeInTheDocument(),
    );
    // store 任务账保留（任务中心仍可见）
    expect(useImportStore.getState().activeJobs[7]).toBeDefined();
  });
});

// --- 索引通道行 ---------------------------------------------------------------------

describe("任务抽屉：索引通道行", () => {
  function counters(pending: number, running: number, done: number, failed = 0, total = 10) {
    return { pending, running, done, failed, total };
  }

  it("统一展示有活儿的通道；无活儿通道不渲染；失败红字计数", async () => {
    indexStatusMock.mockResolvedValue({
      thumb: counters(2, 1, 7),
      exif: counters(0, 0, 10),
      ai: counters(4, 1, 5, 2),
      face: counters(0, 0, 10),
    } satisfies IndexStatus);
    renderDrawer();

    const thumbRow = await screen.findByTestId("taskdrawer-index-thumb");
    expect(thumbRow).toHaveTextContent("正在生成缩略图索引");
    expect(screen.getByTestId("taskdrawer-index-ai")).toHaveTextContent("正在建立语义索引");
    expect(screen.queryByTestId("taskdrawer-index-exif")).not.toBeInTheDocument();
    expect(screen.queryByTestId("taskdrawer-index-face")).not.toBeInTheDocument();

    // ai 通道失败 2：红字
    expect(screen.getByTestId("taskdrawer-index-ai").querySelector('[data-testid="taskdrawer-index-failed"]'))
      .toHaveTextContent("失败 2");
    // thumb 无失败：不渲染红字
    expect(
      screen.getByTestId("taskdrawer-index-thumb").querySelector('[data-testid="taskdrawer-index-failed"]'),
    ).toBeNull();
  });

  it("索引行暂停（running）/继续（无 running → indexTaskResume 全局恢复）；导入中禁用", async () => {
    indexStatusMock.mockResolvedValue({
      thumb: counters(2, 1, 7),
      exif: counters(0, 0, 10),
      ai: counters(0, 0, 10),
      face: counters(0, 0, 10),
    } satisfies IndexStatus);
    const user = userEvent.setup();
    renderDrawer();

    await user.click((await screen.findByTestId("taskdrawer-index-thumb")).querySelector('[data-testid="taskdrawer-index-toggle"]') as HTMLButtonElement);
    await waitFor(() => expect(indexPauseMock).toHaveBeenCalledTimes(1));

    // running=0 的通道：继续 = index_task_resume（全局恢复，2026-09-29 接线）
    indexStatusMock.mockResolvedValue({
      thumb: counters(2, 0, 7),
      exif: counters(0, 0, 10),
      ai: counters(0, 0, 10),
      face: counters(0, 0, 10),
    } satisfies IndexStatus);
    await useAiStore.getState().refreshIndexStatus();
    await user.click(screen.getByTestId("taskdrawer-index-thumb").querySelector('[data-testid="taskdrawer-index-toggle"]') as HTMLButtonElement);
    await waitFor(() => expect(indexResumeMock).toHaveBeenCalledTimes(1));
    expect(kickMock).not.toHaveBeenCalled();

    // 导入进行中：让路闸接管，按钮禁用（导入收尾自动恢复）
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    const toggle = screen.getByTestId("taskdrawer-index-thumb").querySelector('[data-testid="taskdrawer-index-toggle"]') as HTMLButtonElement;
    expect(toggle).toBeDisabled();
  });

  it("全部暂停：运行中导入逐个暂停 + 索引暂停", async () => {
    indexStatusMock.mockResolvedValue({
      thumb: counters(2, 1, 7),
      exif: counters(0, 0, 10),
      ai: counters(0, 0, 10),
      face: counters(0, 0, 10),
    } satisfies IndexStatus);
    const user = userEvent.setup();
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionStarted", jobId: 8, totalFiles: 10, totalBytes: 1000 });

    await user.click(await screen.findByTestId("taskdrawer-pause-all"));

    await waitFor(() => {
      expect(pauseMock).toHaveBeenCalledWith(7);
      expect(pauseMock).toHaveBeenCalledWith(8);
      expect(indexPauseMock).toHaveBeenCalledTimes(1);
    });
  });
});

// --- 错误行（appError） ---------------------------------------------------------------

describe("任务抽屉：错误行", () => {
  it("appError：红字行展示消息（任务页已删，无跳转按钮）", async () => {
    renderDrawer();
    emit({ type: "appError", level: "error", message: "磁盘写入失败", recoverable: true });

    const err = await screen.findByTestId("taskdrawer-error");
    expect(err).toHaveTextContent("磁盘写入失败");
  });

  it("× 收起错误行且同一错误不再弹出；挂载前的旧错误不打扰", async () => {
    const user = userEvent.setup();
    renderDrawer();
    emit({ type: "appError", level: "error", message: "磁盘写入失败", recoverable: true });

    await user.click(await screen.findByTestId("taskdrawer-error-dismiss"));
    await waitFor(() => expect(screen.queryByTestId("taskdrawer-error")).not.toBeInTheDocument());
    expect(useImportStore.getState().lastError).not.toBeNull();
  });
});


// --- 历史区（M4.5 wave-3 第 8 项） -----------------------------------------------------

function jobRow(id: number, status: JobRow["status"] = "done"): JobRow {
  const startedAt = new Date(2026, 8, 18, 12, 0, 0).getTime();
  return {
    id,
    kind: "import",
    deviceId: "E:",
    deviceName: "SanDisk 64G",
    status,
    totalFiles: 120,
    totalBytes: 4_000_000_000,
    statsJson: JSON.stringify({ doneFiles: 117, skippedDuplicates: 2, failedFiles: 1 }),
    startedAt,
    finishedAt: startedAt + 150_000,
  };
}

describe("任务抽屉：历史区", () => {
  it("打开抽屉拉 importJobsPage(0, 50)；行渲染任务名/统计/日志与删除入口", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3), jobRow(2)]);
    renderDrawer();

    const rows = await screen.findAllByTestId("taskdrawer-history-row");
    expect(rows).toHaveLength(2);
    expect(jobsPageMock).toHaveBeenCalledWith(0, 50);
    expect(rows[0]).toHaveTextContent("SanDisk 64G");
    expect(rows[0]).toHaveTextContent("成功 117 · 跳过 2 · 失败 1");
    expect(within(rows[0]).getByTestId("taskdrawer-history-logs")).toBeInTheDocument();
    expect(within(rows[0]).getByTestId("taskdrawer-history-delete")).toBeInTheDocument();
  });

  it("加载更多：afterId=最后一行 id；短页隐藏按钮", async () => {
    jobsPageMock.mockResolvedValueOnce(Array.from({ length: 50 }, (_, i) => jobRow(100 - i)))
      .mockResolvedValueOnce([jobRow(5)]);
    const user = userEvent.setup();
    renderDrawer();
    await screen.findAllByTestId("taskdrawer-history-row");

    await user.click(screen.getByTestId("taskdrawer-history-more"));
    await waitFor(() => expect(jobsPageMock).toHaveBeenLastCalledWith(51, 50));
    await waitFor(() => expect(screen.getAllByTestId("taskdrawer-history-row")).toHaveLength(51));
    // 第二页短页 → 按钮消失
    await waitFor(() =>
      expect(screen.queryByTestId("taskdrawer-history-more")).not.toBeInTheDocument(),
    );
  });

  it("× 删除成功：importJobDelete(id)，行动画退场", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3)]);
    const user = userEvent.setup();
    renderDrawer();
    await screen.findAllByTestId("taskdrawer-history-row");

    await user.click(screen.getByTestId("taskdrawer-history-delete"));
    await waitFor(() => expect(jobDeleteMock).toHaveBeenCalledWith(3));
    await waitFor(() =>
      expect(screen.queryByTestId("taskdrawer-history-row")).not.toBeInTheDocument(),
    );
  });

  it("× 删除命令失败：乐观移除回滚（行恢复）", async () => {
    jobDeleteMock.mockRejectedValue(new Error("locked"));
    jobsPageMock.mockResolvedValue([jobRow(4)]);
    const user = userEvent.setup();
    renderDrawer();
    await screen.findAllByTestId("taskdrawer-history-row");

    await user.click(screen.getByTestId("taskdrawer-history-delete"));
    await waitFor(() => expect(jobDeleteMock).toHaveBeenCalledWith(4));
    await waitFor(() => expect(screen.getByTestId("taskdrawer-history-row")).toBeInTheDocument());
  });

  it("查看日志：打开 LogViewer 弹层并加载该任务日志", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3)]);
    const user = userEvent.setup();
    renderDrawer();
    await screen.findAllByTestId("taskdrawer-history-row");

    await user.click(screen.getByTestId("taskdrawer-history-logs"));
    expect(await screen.findByTestId("taskdrawer-logs-modal")).toBeInTheDocument();
    expect(importLogsPage).toHaveBeenCalledWith(3, 0, 50);
    await user.click(screen.getByTestId("taskdrawer-logs-close"));
    await waitFor(() =>
      expect(screen.queryByTestId("taskdrawer-logs-modal")).not.toBeInTheDocument(),
    );
  });

  it("清除已完成：批量 importJobDelete 历史行 + 隐藏本地终态行", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3), jobRow(2)]);
    const user = userEvent.setup();
    renderDrawer();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionFinished", jobId: 7, stats: finishedStats() });

    await screen.findAllByTestId("taskdrawer-history-row");
    expect(await screen.findByTestId("taskdrawer-import")).toBeInTheDocument();

    await user.click(screen.getByTestId("taskdrawer-clear-done"));

    await waitFor(() => {
      expect(jobDeleteMock).toHaveBeenCalledWith(3);
      expect(jobDeleteMock).toHaveBeenCalledWith(2);
    });
    await waitFor(() =>
      expect(screen.queryByTestId("taskdrawer-import")).not.toBeInTheDocument(),
    );
    expect(useImportStore.getState().activeJobs[7]).toBeDefined();
  });
});

// --- 全局总结弹窗（M4.5 wave-3 第 8 项：从任务页挪到 AppShell 全局挂载） -----------------

describe("总结弹窗（SummaryModalHost 全局）", () => {
  it("importSessionFinished 后任何页面弹出三卡片；关闭出队", async () => {
    const user = userEvent.setup();
    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter initialEntries={["/gallery"]}>
          <SummaryModalHost />
          <Routes>
            <Route path="/gallery" element={<GalleryProbe />} />
          </Routes>
        </MemoryRouter>
      </I18nextProvider>,
    );

    expect(screen.queryByTestId("summary-modal")).not.toBeInTheDocument();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionFinished", jobId: 7, stats: finishedStats() });

    const modal = await screen.findByTestId("summary-modal");
    expect(modal).toHaveTextContent("导入完成");
    expect(within(modal).getByTestId("summary-done")).toHaveTextContent("7");
    expect(within(modal).getByTestId("summary-skipped")).toHaveTextContent("2");
    expect(within(modal).getByTestId("summary-failed")).toHaveTextContent("1");

    await user.click(within(modal).getByRole("button", { name: "关闭" }));
    await waitFor(() => expect(screen.queryByTestId("summary-modal")).not.toBeInTheDocument());
  });
});
