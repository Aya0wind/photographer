import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import TaskCenter from "./TaskCenter";
import { resetImportStoreForTests, useImportStore, type ActiveJob } from "@/stores/importStore";
import { useAiStore } from "@/stores/aiStore";
import {
  cleanCandidates,
  importCancel,
  importJobsPage,
  importLogsPage,
  importPause,
  importResume,
  importRetryFailed,
  indexKickNow,
  indexStatus,
  type IndexStatus,
  type JobRow,
  type LogRow,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importPause: vi.fn(),
    importResume: vi.fn(),
    importCancel: vi.fn(),
    importRetryFailed: vi.fn(),
    importJobsPage: vi.fn(),
    importLogsPage: vi.fn(),
    cleanCandidates: vi.fn(),
    cleanApply: vi.fn(),
    indexStatus: vi.fn(),
    indexKickNow: vi.fn(),
  };
});

const pauseMock = vi.mocked(importPause);
const resumeMock = vi.mocked(importResume);
const cancelMock = vi.mocked(importCancel);
const retryMock = vi.mocked(importRetryFailed);
const jobsPageMock = vi.mocked(importJobsPage);
const logsPageMock = vi.mocked(importLogsPage);
const cleanCandidatesMock = vi.mocked(cleanCandidates);
const indexStatusMock = vi.mocked(indexStatus);
const indexKickNowMock = vi.mocked(indexKickNow);

function runningJob(): ActiveJob {
  return {
    jobId: 7,
    status: "running",
    totalFiles: 100,
    totalBytes: 10_000_000_000,
    doneFiles: 50,
    doneBytes: 5_000_000_000,
    bytesPerSec: 50 * 1024 * 1024,
    currentFile: "DCIM/100CANON/IMG_0042.CR3",
  };
}

function jobRow(id: number, status: JobRow["status"]): JobRow {
  // 本地时区构造，避免 CI/本地时区差异影响时间显示断言
  const startedAt = new Date(2026, 8, 18, 12, 0, 0).getTime();
  return {
    id,
    kind: "import",
    deviceId: "E:",
    deviceName: "SanDisk 64G",
    status,
    totalFiles: 120,
    totalBytes: 4_000_000_000,
    statsJson: "{}",
    startedAt,
    finishedAt: status === "running" || status === "paused" ? null : startedAt + 150_000,
  };
}

function logRow(id: number, level: LogRow["level"], message: string): LogRow {
  return { id, ts: new Date(2026, 8, 18, 12, 0, id).getTime(), level, jobId: 3, message };
}

function renderCenter() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter>
        <TaskCenter />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

beforeEach(() => {
  resetImportStoreForTests();
  pauseMock.mockReset().mockResolvedValue(undefined);
  resumeMock.mockReset().mockResolvedValue(undefined);
  cancelMock.mockReset().mockResolvedValue(undefined);
  retryMock.mockReset().mockResolvedValue(null);
  jobsPageMock.mockReset().mockResolvedValue([]);
  logsPageMock.mockReset().mockResolvedValue([]);
  cleanCandidatesMock.mockReset().mockResolvedValue([]);
  indexStatusMock.mockReset().mockResolvedValue(null);
  indexKickNowMock.mockReset().mockResolvedValue(undefined);
  useAiStore.getState().resetForTests();
});

describe("当前任务卡", () => {
  it("无活跃任务时显示空态", () => {
    renderCenter();

    expect(screen.getByTestId("task-idle")).toHaveTextContent("暂无进行中的任务");
  });

  it("展示进度/速度/文件数/当前文件名", () => {
    useImportStore.setState({ activeJobs: { 7: runningJob() }, currentJobId: 7 });

    renderCenter();

    const progress = screen.getByTestId("task-progress");
    expect(progress).toHaveAttribute("aria-valuenow", "50");
    expect(screen.getByText("50.0 MB/s")).toBeInTheDocument();
    expect(screen.getByText("50 / 100 个文件")).toBeInTheDocument();
    expect(screen.getByTestId("current-file")).toHaveTextContent("IMG_0042.CR3");
    expect(screen.getByText("进行中")).toBeInTheDocument();
  });

  it("运行中可暂停/取消；暂停后变恢复", async () => {
    useImportStore.setState({ activeJobs: { 7: runningJob() }, currentJobId: 7 });
    const user = userEvent.setup();

    renderCenter();
    await user.click(await screen.findByRole("button", { name: "暂停" }));
    expect(pauseMock).toHaveBeenCalledWith(7);
    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(cancelMock).toHaveBeenCalledWith(7);

    // 事件驱动状态：paused 后按钮变为恢复
    act(() => {
      useImportStore.getState().handleAppEvent({ type: "importPaused", jobId: 7 });
    });
    await user.click(screen.getByRole("button", { name: "恢复" }));
    expect(resumeMock).toHaveBeenCalledWith(7);
  });

  it("已完成任务不再占据当前任务卡", () => {
    useImportStore.setState({
      activeJobs: { 7: { ...runningJob(), status: "done" } },
      currentJobId: 7,
    });

    renderCenter();

    expect(screen.getByTestId("task-idle")).toBeInTheDocument();
  });
});

describe("历史任务表", () => {
  it("渲染状态徽标/设备/文件数/耗时/开始时间", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3, "done"), jobRow(2, "cancelled")]);

    renderCenter();

    const row3 = await screen.findByTestId("history-row-3");
    expect(row3).toHaveTextContent("已完成");
    expect(row3).toHaveTextContent("SanDisk 64G");
    expect(row3).toHaveTextContent("120");
    expect(row3).toHaveTextContent("2分30秒");
    expect(row3).toHaveTextContent("2026-09-18 12:00:00");
    expect(screen.getByTestId("history-row-2")).toHaveTextContent("已取消");
  });

  it("点击行展开日志查看器", async () => {
    jobsPageMock.mockResolvedValue([jobRow(3, "done")]);
    logsPageMock.mockResolvedValue([logRow(1, "info", "copy ok")]);
    const user = userEvent.setup();

    renderCenter();
    await user.click(await screen.findByTestId("history-row-3"));

    expect(await screen.findByTestId("log-viewer-3")).toBeInTheDocument();
    expect(await screen.findByText("copy ok")).toBeInTheDocument();
  });

  it("未取尽时提供加载更多", async () => {
    // 首页满一页（20 行，id 20..1），游标 = 最后一行 id 1
    const fullPage = Array.from({ length: 20 }, (_, i) => jobRow(20 - i, "done"));
    jobsPageMock.mockResolvedValueOnce(fullPage).mockResolvedValueOnce([]);
    const user = userEvent.setup();

    renderCenter();
    await screen.findByTestId("history-row-20");

    await user.click(screen.getByRole("button", { name: "加载更多" }));
    await waitFor(() => expect(jobsPageMock).toHaveBeenCalledTimes(2));
    expect(jobsPageMock).toHaveBeenLastCalledWith(1, 20);
  });

  it("历史空态：引导文案 + 去导入按钮（标题不再重复空态文案）", async () => {
    const user = userEvent.setup();

    renderCenter();

    const empty = await screen.findByTestId("task-history-empty");
    expect(empty).toHaveTextContent("完成第一次导入后，历史记录会显示在这里");
    expect(screen.queryByText("暂无历史任务")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "去导入" }));
  });
});

describe("总结弹窗", () => {
  it("sessionFinished 后自动弹出：三卡片+耗时+平均速度", async () => {
    renderCenter();

    act(() => {
      useImportStore.getState().handleAppEvent({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
      useImportStore.getState().handleAppEvent({
        type: "importSessionFinished",
        jobId: 7,
        stats: {
          totalFiles: 10,
          doneFiles: 7,
          skippedDuplicates: 2,
          failedFiles: 1,
          totalBytes: 1000,
          doneBytes: 700,
          elapsedMs: 150_000,
          bytesPerSec: 5 * 1024 * 1024,
        },
      });
    });

    const modal = await screen.findByTestId("summary-modal");
    expect(modal).toBeInTheDocument();
    expect(screen.getByTestId("summary-done")).toHaveTextContent("7");
    expect(screen.getByTestId("summary-skipped")).toHaveTextContent("2");
    expect(screen.getByTestId("summary-failed")).toHaveTextContent("1");
    expect(modal).toHaveTextContent("2分30秒");
    expect(modal).toHaveTextContent("5.0 MB/s");
  });

  it("失败清单与重试：retry 成功显示新任务号，关闭出队弹窗", async () => {
    retryMock.mockResolvedValueOnce(99);
    const user = userEvent.setup();

    renderCenter();
    act(() => {
      useImportStore.setState({
        summary: {
          jobId: 7,
          mode: "copy",
          stats: {
            totalFiles: 3,
            doneFiles: 1,
            skippedDuplicates: 0,
            failedFiles: 2,
            totalBytes: 30,
            doneBytes: 10,
            elapsedMs: 1000,
            bytesPerSec: 10,
          },
          failures: [
            { src: "E:/A.CR3", dst: "", state: "failed" },
            { src: "E:/B.CR3", dst: "", state: "error" },
          ],
        },
      });
    });

    expect(await screen.findByTestId("summary-failures")).toHaveTextContent("E:/A.CR3");
    await user.click(screen.getByRole("button", { name: "重试失败项" }));
    expect(retryMock).toHaveBeenCalledWith(7);
    expect(await screen.findByText("已创建重试任务 #99")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "关闭" }));
    await waitFor(() => expect(screen.queryByTestId("summary-modal")).not.toBeInTheDocument());
    expect(useImportStore.getState().summary).toBeNull();
  });

  it("无失败清单但有失败计数时提示查看日志", async () => {
    renderCenter();
    act(() => {
      useImportStore.setState({
        summary: {
          jobId: 8,
          mode: "copy",
          stats: {
            totalFiles: 3,
            doneFiles: 2,
            skippedDuplicates: 0,
            failedFiles: 1,
            totalBytes: 30,
            doneBytes: 20,
            elapsedMs: 1000,
            bytesPerSec: 10,
          },
          failures: [],
        },
      });
    });

    expect(await screen.findByText("失败详情请展开该任务查看日志。")).toBeInTheDocument();
  });

  it("移动模式：标题移动完成，成功卡换为已移动 N，源删除失败警示", async () => {
    renderCenter();
    act(() => {
      useImportStore.setState({
        summary: {
          jobId: 9,
          mode: "move",
          stats: {
            totalFiles: 10,
            doneFiles: 8,
            skippedDuplicates: 1,
            failedFiles: 1,
            totalBytes: 100,
            doneBytes: 80,
            elapsedMs: 1000,
            bytesPerSec: 10,
            moved: 8,
            sourceDeleteFailed: 2,
          },
          failures: [],
        },
      });
    });

    const modal = await screen.findByTestId("summary-modal");
    expect(modal).toHaveTextContent("移动完成");
    const doneCard = screen.getByTestId("summary-done");
    expect(doneCard).toHaveTextContent("已移动");
    expect(doneCard).toHaveTextContent("8");
    expect(screen.getByTestId("summary-source-delete-failed")).toHaveTextContent("2");
  });

  it("移动模式的当前任务卡标题为移动任务", () => {
    useImportStore.setState({
      activeJobs: { 7: runningJob() },
      currentJobId: 7,
      jobModes: { 7: "move" },
    });

    renderCenter();

    expect(screen.getByText("移动任务 #7")).toBeInTheDocument();
  });
});

describe("安全清卡入口（M2）", () => {
  it("总结弹窗：volume 源任务显示「清理源文件…」并打开对话框", async () => {
    cleanCandidatesMock.mockResolvedValue([
      { src: "E:/A.CR3", relPath: "A.CR3", size: 1024, assetId: null },
    ]);
    const user = userEvent.setup();
    renderCenter();
    act(() => {
      useImportStore.setState({
        jobSources: { 7: "volume" },
        summary: {
          jobId: 7,
          mode: "copy",
          stats: {
            totalFiles: 3,
            doneFiles: 3,
            skippedDuplicates: 0,
            failedFiles: 0,
            totalBytes: 30,
            doneBytes: 30,
            elapsedMs: 1000,
            bytesPerSec: 10,
          },
          failures: [],
        },
      });
    });

    await user.click(await screen.findByTestId("summary-clean"));
    expect(await screen.findByTestId("clean-dialog")).toBeInTheDocument();
    expect(cleanCandidatesMock).toHaveBeenCalledWith(7);
  });

  it("总结弹窗：folder 源任务不显示清卡入口（本地纳管不可清）", async () => {
    renderCenter();
    act(() => {
      useImportStore.setState({
        jobSources: { 8: "folder" },
        summary: {
          jobId: 8,
          mode: "copy",
          stats: {
            totalFiles: 3,
            doneFiles: 3,
            skippedDuplicates: 0,
            failedFiles: 0,
            totalBytes: 30,
            doneBytes: 30,
            elapsedMs: 1000,
            bytesPerSec: 10,
          },
          failures: [],
        },
      });
    });

    expect(await screen.findByTestId("summary-modal")).toBeInTheDocument();
    expect(screen.queryByTestId("summary-clean")).not.toBeInTheDocument();
  });

  it("历史行：已完成 volume 源行显示清卡动作并打开对话框；folder 源/未完成行不显示", async () => {
    jobsPageMock.mockResolvedValue([
      jobRow(3, "done"),
      { ...jobRow(2, "done"), deviceId: "FOLDER:D:\\老照片" },
      jobRow(1, "cancelled"),
    ]);
    const user = userEvent.setup();

    renderCenter();

    // volume 源已完成行：有动作；folder 源已完成行与未完成行：无
    await screen.findByTestId("history-row-3");
    expect(screen.getByTestId("history-clean-3")).toBeInTheDocument();
    expect(screen.queryByTestId("history-clean-2")).not.toBeInTheDocument();
    expect(screen.queryByTestId("history-clean-1")).not.toBeInTheDocument();

    // 点击动作打开对话框（不触发行展开日志）
    await user.click(screen.getByTestId("history-clean-3"));
    expect(await screen.findByTestId("clean-dialog")).toBeInTheDocument();
    expect(screen.queryByTestId("log-viewer-3")).not.toBeInTheDocument();
  });
});

// --- 索引任务卡（M4 二轮升级：三类计数 + 立即开始） -----------------------------------

describe("索引任务卡（indexStatus 计数 + 立即开始）", () => {
  const STATUS: IndexStatus = {
    thumb: { pending: 3, running: 0, done: 117, failed: 1, total: 120 },
    exif: { pending: 0, running: 0, done: 120, failed: 0, total: 120 },
    ai: { pending: 0, running: 0, done: 45, failed: 0, total: 120 },
  };

  it("无恢复事件（indexPending=null）→ 不渲染索引卡", () => {
    renderCenter();
    expect(screen.queryByTestId("task-index")).not.toBeInTheDocument();
  });

  it("渲染三类计数；仅对有待办者给「立即开始」；点击负载 index_kick_now(kind)", async () => {
    indexStatusMock.mockResolvedValue(STATUS);
    useImportStore.setState({ indexPending: 12 });
    useAiStore.setState({ indexStatus: STATUS });
    renderCenter();

    const card = await screen.findByTestId("task-index");
    const statusLine = within(card).getByTestId("task-index-status");
    expect(statusLine).toHaveTextContent("缩略图：待处理 3 · 已完成 117 · 失败 1");
    expect(statusLine).toHaveTextContent("详细信息：待处理 0 · 已完成 120");
    expect(statusLine).toHaveTextContent("语义：已索引 45 / 120");

    // thumb（待处理 3）与 ai（45/120 未完成）可立即开始；exif 无待办 → 无按钮
    expect(within(card).getByTestId("task-index-kick-thumb")).toBeInTheDocument();
    expect(within(card).getByTestId("task-index-kick-ai")).toBeInTheDocument();
    expect(within(card).queryByTestId("task-index-kick-exif")).not.toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(within(card).getByTestId("task-index-kick-ai"));
    await waitFor(() => expect(indexKickNowMock).toHaveBeenCalledWith("ai"));
  });

  it("index_status 不可用（null）→ 仅有待处理行 + 暂停（无立即开始）", async () => {
    useImportStore.setState({ indexPending: 5 });
    renderCenter();

    const card = await screen.findByTestId("task-index");
    expect(within(card).getByTestId("task-index-pause")).toBeInTheDocument();
    expect(within(card).queryByTestId("task-index-kick-thumb")).not.toBeInTheDocument();
  });
});
