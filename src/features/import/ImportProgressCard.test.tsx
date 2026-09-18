import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter, Route, Routes } from "react-router";

import i18n from "@/i18n";
import ImportProgressCard, { FINISHED_LINGER_MS } from "./ImportProgressCard";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import {
  importCancel,
  importPause,
  importResume,
  type AppEvent,
  type ImportStats,
} from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importPause: vi.fn(),
    importResume: vi.fn(),
    importCancel: vi.fn(),
  };
});

const pauseMock = vi.mocked(importPause);
const resumeMock = vi.mocked(importResume);
const cancelMock = vi.mocked(importCancel);

function GalleryProbe() {
  return <div data-testid="gallery-probe" />;
}

function TasksProbe() {
  return <div data-testid="tasks-probe" />;
}

/** 卡片与 Routes 平级常驻（与 AppShell 挂载语义一致：导航不卸载） */
function renderCard() {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter initialEntries={["/gallery"]}>
        <ImportProgressCard />
        <Routes>
          <Route path="/gallery" element={<GalleryProbe />} />
          <Route path="/tasks" element={<TasksProbe />} />
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

/** 直接置终态（不经事件流）：fake timers 用例避免活跃→终态切换产生退场克隆节点 */
function seedFinishedJob(): void {
  act(() => {
    useImportStore.setState({
      activeJobs: {
        7: {
          jobId: 7,
          status: "done",
          totalFiles: 10,
          totalBytes: 1000,
          doneFiles: 7,
          doneBytes: 700,
          bytesPerSec: 5 * 1024 * 1024,
          currentFile: "",
        },
      },
      currentJobId: 7,
      summary: { jobId: 7, stats: finishedStats(), failures: [], mode: "copy" },
    });
  });
}

beforeEach(() => {
  resetImportStoreForTests();
  pauseMock.mockReset().mockResolvedValue(undefined);
  resumeMock.mockReset().mockResolvedValue(undefined);
  cancelMock.mockReset().mockResolvedValue(undefined);
});

afterEach(() => {
  vi.useRealTimers();
});

describe("渲染与空态", () => {
  it("无任务、无新错误时不渲染任何卡片", () => {
    renderCard();

    expect(screen.queryByTestId("import-card")).not.toBeInTheDocument();
    expect(screen.queryByTestId("import-card-error")).not.toBeInTheDocument();
  });

  it("挂载前已存在的旧错误不再弹出（只响应新到达的 appError）", () => {
    act(() => {
      useImportStore.setState({
        lastError: { level: "error", message: "旧错误", recoverable: true },
      });
    });

    renderCard();

    expect(screen.queryByTestId("import-card-error")).not.toBeInTheDocument();
  });
});

describe("进度随事件渲染", () => {
  it("sessionStarted + 进度事件：标题/进度条宽度/速度/当前文件", async () => {
    renderCard();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 100, totalBytes: 10_000_000_000 });

    const card = await screen.findByTestId("import-card");
    expect(card).toHaveAttribute("data-phase", "running");
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
      expect(screen.getByTestId("import-card-progress")).toHaveAttribute("aria-valuenow", "50");
    });
    const fill = screen.getByTestId("import-card-progress").firstElementChild as HTMLElement;
    expect(fill.style.width).toBe("50%");
    expect(screen.getByText("50.0 MB/s")).toBeInTheDocument();
    expect(screen.getByText("4.7 GB / 9.3 GB")).toBeInTheDocument();
    expect(screen.getByTestId("import-card-file")).toHaveTextContent("IMG_0042.CR3");
  });

  it("移动任务（jobModes=move）标题为移动任务；暂停事件切换状态样式与按钮", async () => {
    act(() => {
      useImportStore.getState().setPendingJobMode("move");
    });
    renderCard();
    emit({ type: "importSessionStarted", jobId: 9, totalFiles: 4, totalBytes: 400 });

    expect(await screen.findByText("移动任务 #9")).toBeInTheDocument();

    emit({ type: "importPaused", jobId: 9 });
    await waitFor(() => {
      expect(screen.getByTestId("import-card")).toHaveAttribute("data-phase", "paused");
    });
    expect(screen.getByText("已暂停")).toBeInTheDocument();
    expect(screen.getByTestId("import-card-toggle")).toHaveTextContent("恢复");
  });
});

describe("快捷按钮与导航", () => {
  it("暂停/继续/取消按钮调用 IPC 命令", async () => {
    renderCard();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 100 });
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("import-card-toggle"));
    expect(pauseMock).toHaveBeenCalledWith(7);

    emit({ type: "importPaused", jobId: 7 });
    await user.click(await screen.findByRole("button", { name: "恢复" }));
    expect(resumeMock).toHaveBeenCalledWith(7);

    emit({ type: "importResumed", jobId: 7 });
    await user.click(screen.getByRole("button", { name: "取消" }));
    expect(cancelMock).toHaveBeenCalledWith(7);
  });

  it("点击卡片主体跳转任务中心", async () => {
    renderCard();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 100 });
    const user = userEvent.setup();

    await user.click(await screen.findByTestId("import-card-body"));

    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
  });
});

describe("完成/取消终态", () => {
  it("sessionFinished：完成样式 + 成功/跳过/失败计数行；点击跳任务中心（summary 保留）", async () => {
    renderCard();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importSessionFinished", jobId: 7, stats: finishedStats() });

    const card = await screen.findByTestId("import-card");
    expect(card).toHaveAttribute("data-phase", "done");
    expect(card).toHaveTextContent("导入完成");
    expect(card).toHaveTextContent("#7");
    expect(screen.getByTestId("import-card-counts")).toHaveTextContent("成功 7 · 跳过 2 · 失败 1");
    // 总结仍在 store：任务中心打开时复用弹出总结弹窗
    expect(useImportStore.getState().summary?.jobId).toBe(7);

    const user = userEvent.setup();
    await user.click(screen.getByTestId("import-card-body"));
    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
  });

  it("移动任务完成：标题移动完成，计数行为已移动", async () => {
    act(() => {
      useImportStore.getState().setPendingJobMode("move");
    });
    renderCard();
    emit({ type: "importSessionStarted", jobId: 11, totalFiles: 10, totalBytes: 1000 });
    emit({
      type: "importSessionFinished",
      jobId: 11,
      stats: finishedStats({ doneFiles: 8, skippedDuplicates: 1, moved: 8 }),
    });

    const card = await screen.findByTestId("import-card");
    expect(card).toHaveTextContent("移动完成");
    expect(screen.getByTestId("import-card-counts")).toHaveTextContent("已移动 8 · 跳过 1 · 失败 1");
  });

  it("软取消：转已取消样式", async () => {
    renderCard();
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 1000 });
    emit({ type: "importCancelled", jobId: 7 });

    const card = await screen.findByTestId("import-card");
    expect(card).toHaveAttribute("data-phase", "cancelled");
    expect(card).toHaveTextContent("已取消");
    expect(screen.queryByTestId("import-card-counts")).not.toBeInTheDocument();
  });

  it("完成卡停留 5s 后自动收起（fake timers）", async () => {
    seedFinishedJob();
    vi.useFakeTimers();
    renderCard();

    expect(screen.getByTestId("import-card")).toHaveAttribute("data-phase", "done");

    act(() => {
      vi.advanceTimersByTime(FINISHED_LINGER_MS - 1);
    });
    expect(screen.getByTestId("import-card")).toBeInTheDocument();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    // 退场动画（150ms）切回真实时钟后完成，DOM 才移除
    vi.useRealTimers();
    await waitFor(() => expect(screen.queryByTestId("import-card")).not.toBeInTheDocument());
  });

  it("已收起的完成卡不因后续 store 变化（如关闭总结弹窗）重复弹出", async () => {
    seedFinishedJob();
    vi.useFakeTimers();
    renderCard();
    act(() => {
      vi.advanceTimersByTime(FINISHED_LINGER_MS);
    });
    vi.useRealTimers();
    await waitFor(() => expect(screen.queryByTestId("import-card")).not.toBeInTheDocument());

    act(() => {
      useImportStore.getState().dismissSummary();
    });
    expect(screen.queryByTestId("import-card")).not.toBeInTheDocument();
  });
});

describe("错误卡（appError）", () => {
  it("appError：错误样式展示消息；查看任务跳任务中心", async () => {
    renderCard();
    emit({ type: "appError", level: "error", message: "磁盘写入失败", recoverable: true });

    const err = await screen.findByTestId("import-card-error");
    expect(err).toHaveAttribute("data-phase", "error");
    expect(err).toHaveTextContent("磁盘写入失败");

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "查看任务" }));
    expect(await screen.findByTestId("tasks-probe")).toBeInTheDocument();
  });

  it("收起按钮关闭错误卡且同一错误不再弹出", async () => {
    renderCard();
    emit({ type: "appError", level: "error", message: "磁盘写入失败", recoverable: true });
    const user = userEvent.setup();

    await user.click(await screen.findByRole("button", { name: "收起" }));
    await waitFor(() => expect(screen.queryByTestId("import-card-error")).not.toBeInTheDocument());
    // store 中 lastError 未清，但已被本地收起
    expect(useImportStore.getState().lastError).not.toBeNull();
    expect(screen.queryByTestId("import-card-error")).not.toBeInTheDocument();
  });
});
