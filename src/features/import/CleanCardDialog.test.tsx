import { beforeEach, describe, expect, it, vi } from "vitest";

import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { MemoryRouter } from "react-router";

import i18n from "@/i18n";
import CleanCardDialogLayer from "./CleanCardDialog";
import { TaskDrawerPanel } from "@/features/tasks/TaskDrawer";
import { resetImportStoreForTests, useImportStore } from "@/stores/importStore";
import { cleanApply, cleanCandidates, type AppEvent, type CleanCandidateDto } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    cleanCandidates: vi.fn(),
    cleanApply: vi.fn(),
  };
});

const candidatesMock = vi.mocked(cleanCandidates);
const applyMock = vi.mocked(cleanApply);

function candidate(src: string, size: number): CleanCandidateDto {
  return { src, relPath: src.replace("E:/", ""), size, assetId: null };
}

function renderDialog(jobId = 7) {
  return render(
    <I18nextProvider i18n={i18n}>
      <MemoryRouter>
        <CleanCardDialogLayer open={true} jobId={jobId} onClose={() => {}} />
      </MemoryRouter>
    </I18nextProvider>,
  );
}

function emit(event: AppEvent): void {
  act(() => {
    useImportStore.getState().handleAppEvent(event);
  });
}

beforeEach(() => {
  resetImportStoreForTests();
  candidatesMock.mockReset().mockResolvedValue([]);
  applyMock.mockReset().mockResolvedValue(null);
});

describe("候选加载", () => {
  it("打开先调 clean_candidates：文件数/总容量大数字 + 明细 + 指纹复验说明", async () => {
    candidatesMock.mockResolvedValue([
      candidate("E:/DCIM/IMG_0001.CR3", 45 * 1024 * 1024),
      candidate("E:/DCIM/IMG_0002.CR3", 44 * 1024 * 1024),
    ]);

    renderDialog();

    expect(candidatesMock).toHaveBeenCalledWith(7);
    const dialog = await screen.findByTestId("clean-dialog");
    const summary = within(dialog).getByTestId("clean-summary");
    expect(summary).toHaveTextContent("2");
    expect(summary).toHaveTextContent("89.0 MB"); // 45+44 MB
    const list = within(dialog).getByTestId("clean-list");
    expect(within(list).getByText("E:/DCIM/IMG_0001.CR3")).toBeInTheDocument();
    expect(within(list).getByText("E:/DCIM/IMG_0002.CR3")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("逐文件校验指纹");
    // 明细超限截断提示
    expect(within(list).queryByText(/还有/)).not.toBeInTheDocument();
  });

  it("明细超 8 条截断：显示「还有 N 个…」", async () => {
    candidatesMock.mockResolvedValue(
      Array.from({ length: 10 }, (_, i) => candidate(`E:/IMG_${i}.JPG`, 1024)),
    );

    renderDialog();

    const list = await screen.findByTestId("clean-list");
    expect(within(list).getAllByTitle(/^E:\//)).toHaveLength(8);
    expect(within(list).getByText("还有 2 个…")).toBeInTheDocument();
  });

  it("空态：无候选时显示无可清理说明", async () => {
    candidatesMock.mockResolvedValue([]);

    renderDialog();

    const empty = await screen.findByTestId("clean-empty");
    expect(empty).toHaveTextContent("没有可清理的源文件");
    expect(screen.queryByTestId("clean-apply")).not.toBeInTheDocument();
  });
});

describe("强确认门控", () => {
  it("未勾选确认时删除按钮禁用；勾选后可用且文案带 N/容量", async () => {
    candidatesMock.mockResolvedValue([
      candidate("E:/A.CR3", 2 * 1024 * 1024),
      candidate("E:/B.CR3", 3 * 1024 * 1024),
    ]);
    const user = userEvent.setup();
    renderDialog();

    const apply = await screen.findByTestId("clean-apply");
    expect(apply).toBeDisabled();
    expect(apply).toHaveTextContent("删除 2 个文件（释放 5.0 MB）");

    await user.click(screen.getByTestId("clean-confirm"));
    expect(screen.getByTestId("clean-apply")).toBeEnabled();
  });
});

describe("执行与事件驱动状态", () => {
  it("确认 → cleanApply 调用；cleanStarted/cleanFinished 驱动进行中→完成态；errors 可展开", async () => {
    candidatesMock.mockResolvedValue([candidate("E:/A.CR3", 1024), candidate("E:/B.CR3", 1024)]);
    applyMock.mockResolvedValue({ deleted: 1, failed: 1, freedBytes: 1024, errors: ["E:/B.CR3: 指纹不一致"] });
    const user = userEvent.setup();
    renderDialog();

    await user.click(await screen.findByTestId("clean-confirm"));
    await user.click(screen.getByTestId("clean-apply"));

    expect(applyMock).toHaveBeenCalledWith(7);

    // cleanStarted → 进行中
    emit({ type: "cleanStarted", jobId: 7, count: 2, bytes: 2048 });
    expect(await screen.findByTestId("clean-running")).toHaveTextContent("2");
    expect(screen.getByTestId("clean-running")).toHaveTextContent("2.0 KB");

    // cleanFinished → 完成态：成功/失败/释放
    emit({
      type: "cleanFinished",
      jobId: 7,
      stats: { deleted: 1, failed: 1, freedBytes: 1024, errors: ["E:/B.CR3: 指纹不一致"] },
    });
    const finished = await screen.findByTestId("clean-finished");
    expect(finished).toHaveTextContent("1"); // 成功
    expect(finished).toHaveTextContent("1.0 KB"); // 释放
    expect(screen.queryByTestId("clean-running")).not.toBeInTheDocument();

    // 失败清单展开
    await user.click(screen.getByTestId("clean-errors-toggle"));
    const errors = await screen.findByTestId("clean-errors");
    expect(within(errors).getByText("E:/B.CR3: 指纹不一致")).toBeInTheDocument();
  });

  it("其他任务的清卡事件不影响本对话框", async () => {
    candidatesMock.mockResolvedValue([candidate("E:/A.CR3", 1024)]);
    const user = userEvent.setup();
    renderDialog();

    await user.click(await screen.findByTestId("clean-confirm"));
    await user.click(screen.getByTestId("clean-apply"));

    emit({ type: "cleanStarted", jobId: 99, count: 5, bytes: 5000 });
    // 仍是候选清单态（本 jobId=7 未开始）
    expect(await screen.findByTestId("clean-summary")).toBeInTheDocument();
    expect(screen.queryByTestId("clean-running")).not.toBeInTheDocument();
  });

  it("无失败时完成态不显示失败清单", async () => {
    candidatesMock.mockResolvedValue([candidate("E:/A.CR3", 1024)]);
    const user = userEvent.setup();
    renderDialog();

    await user.click(await screen.findByTestId("clean-confirm"));
    await user.click(screen.getByTestId("clean-apply"));
    emit({ type: "cleanStarted", jobId: 7, count: 1, bytes: 1024 });
    emit({
      type: "cleanFinished",
      jobId: 7,
      stats: { deleted: 1, failed: 0, freedBytes: 1024, errors: [] },
    });

    const finished = await screen.findByTestId("clean-finished");
    expect(finished).toBeInTheDocument();
    expect(screen.queryByTestId("clean-errors-toggle")).not.toBeInTheDocument();
  });
});

describe("进度卡完成态入口（volume/MTP 才显示）", () => {
  it("volume 源完成卡显示「清理源文件…」并可打开对话框", async () => {
    act(() => {
      useImportStore.setState({
        activeJobs: {
          7: {
            jobId: 7,
            status: "done",
            totalFiles: 10,
            totalBytes: 1000,
            doneFiles: 10,
            doneBytes: 1000,
            bytesPerSec: 0,
            currentFile: "",
          },
        },
        currentJobId: 7,
        jobSources: { 7: "volume" },
        summary: {
          jobId: 7,
          mode: "copy",
          stats: {
            totalFiles: 10,
            doneFiles: 10,
            skippedDuplicates: 0,
            failedFiles: 0,
            totalBytes: 1000,
            doneBytes: 1000,
            elapsedMs: 1000,
            bytesPerSec: 0,
          },
          failures: [],
        },
      });
    });
    candidatesMock.mockResolvedValue([candidate("E:/A.CR3", 1024)]);
    const user = userEvent.setup();

    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerPanel open onClose={() => {}} />
        </MemoryRouter>
      </I18nextProvider>,
    );

    await user.click(await screen.findByTestId("taskdrawer-import-clean"));
    expect(await screen.findByTestId("clean-dialog")).toBeInTheDocument();
    expect(candidatesMock).toHaveBeenCalledWith(7);
  });

  it("folder 源完成卡不显示清卡入口（本地纳管不可清）", async () => {
    act(() => {
      useImportStore.setState({
        activeJobs: {
          7: {
            jobId: 7,
            status: "done",
            totalFiles: 10,
            totalBytes: 1000,
            doneFiles: 10,
            doneBytes: 1000,
            bytesPerSec: 0,
            currentFile: "",
          },
        },
        currentJobId: 7,
        jobSources: { 7: "folder" },
        summary: null,
      });
    });

    render(
      <I18nextProvider i18n={i18n}>
        <MemoryRouter>
          <TaskDrawerPanel open onClose={() => {}} />
        </MemoryRouter>
      </I18nextProvider>,
    );

    await screen.findByTestId("taskdrawer-import");
    await waitFor(() => expect(screen.queryByTestId("taskdrawer-import-clean")).not.toBeInTheDocument());
  });
});
