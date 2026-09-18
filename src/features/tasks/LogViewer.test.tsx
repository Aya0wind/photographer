import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import LogViewer from "./LogViewer";
import { importLogsPage, type LogRow } from "@/ipc/api";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    importLogsPage: vi.fn(),
  };
});

const logsPageMock = vi.mocked(importLogsPage);

function row(id: number, level: LogRow["level"], message: string): LogRow {
  // 本地时区构造，避免 CI/本地时区差异影响时间显示断言
  return { id, ts: new Date(2026, 8, 18, 12, 0, id).getTime(), level, jobId: 3, message };
}

function renderViewer(jobId = 3) {
  return render(
    <I18nextProvider i18n={i18n}>
      <LogViewer jobId={jobId} />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  logsPageMock.mockReset().mockResolvedValue([]);
});

describe("LogViewer", () => {
  it("首屏加载并渲染等宽日志行（时间/级别/消息）", async () => {
    logsPageMock.mockResolvedValue([
      row(1, "info", "session started"),
      row(2, "error", "copy failed: E:/A.CR3"),
    ]);

    renderViewer();

    expect(logsPageMock).toHaveBeenCalledWith(3, 0, 50);
    expect(await screen.findByText("session started")).toBeInTheDocument();
    // 级别列与消息同行渲染
    const message = screen.getByText("copy failed: E:/A.CR3");
    expect(message.closest("div")).toHaveTextContent("error");
  });

  it("数据不可用时显示空态", async () => {
    renderViewer();

    expect(await screen.findByText("暂无日志")).toBeInTheDocument();
  });

  it("级别筛选：只显示所选级别", async () => {
    logsPageMock.mockResolvedValue([row(1, "info", "info line"), row(2, "warn", "warn line"), row(3, "error", "error line")]);

    renderViewer();
    await screen.findByText("info line");
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "error" }));

    expect(screen.getByText("error line")).toBeInTheDocument();
    expect(screen.queryByText("info line")).not.toBeInTheDocument();
    expect(screen.queryByText("warn line")).not.toBeInTheDocument();
    expect(screen.getByText("1 条")).toBeInTheDocument();
  });

  it("游标分页：加载更多以最后一条 id 为游标", async () => {
    // 首页满一页（50 行）才未取尽；游标 = 最后一行 id 50
    const fullPage = Array.from({ length: 50 }, (_, i) => row(i + 1, "info", `line ${i + 1}`));
    logsPageMock.mockResolvedValueOnce(fullPage).mockResolvedValueOnce([row(51, "info", "tail")]);
    const user = userEvent.setup();

    renderViewer();
    await screen.findByText("line 1");
    await user.click(screen.getByRole("button", { name: "加载更多" }));

    expect(await screen.findByText("tail")).toBeInTheDocument();
    expect(logsPageMock).toHaveBeenLastCalledWith(3, 50, 50);
  });

  it("不足一页视为取尽，显示已全部加载", async () => {
    logsPageMock.mockResolvedValue([row(1, "info", "only one")]);

    renderViewer();

    await screen.findByText("only one");
    expect(screen.getByText("已全部加载")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "加载更多" })).not.toBeInTheDocument();
  });
});
