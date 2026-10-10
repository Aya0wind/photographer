import { beforeEach, describe, expect, it, vi } from "vitest";

import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";

import i18n from "@/i18n";
import StoragePage from "./StoragePage";
import {
  openWithSystem,
  photoLibraryCreate,
  photoLibraryList,
  photoLibraryRemove,
  photoLibraryScanCancel,
  photoLibraryScanStatus,
  subscribeAppEvents,
  type AppEvent,
  type LibraryScanStatus,
  type PhotoLibrary,
} from "@/ipc/api";

// IPC 全按 P0 契约 mock（后端实现并行进行中，汇合后联调）
vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    photoLibraryList: vi.fn(),
    photoLibraryCreate: vi.fn(),
    photoLibraryRemove: vi.fn(),
    photoLibraryScanStatus: vi.fn(),
    photoLibraryScanCancel: vi.fn(),
    openWithSystem: vi.fn(),
    subscribeAppEvents: vi.fn(),
  };
});
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const listMock = vi.mocked(photoLibraryList);
const createMock = vi.mocked(photoLibraryCreate);
const removeMock = vi.mocked(photoLibraryRemove);
const scanStatusMock = vi.mocked(photoLibraryScanStatus);
const scanCancelMock = vi.mocked(photoLibraryScanCancel);
const openMock = vi.mocked(openWithSystem);
const subscribeMock = vi.mocked(subscribeAppEvents);

function lib(overrides: Partial<PhotoLibrary> = {}): PhotoLibrary {
  return {
    id: "lib-1",
    name: "主照片库",
    rootPath: "I:\\Photos",
    createdAt: "2026-10-09T00:00:00Z",
    status: "online",
    assetCount: 120,
    sizeBytes: 1073741824, // 1.0 GiB
    ...overrides,
  };
}

function scanRow(overrides: Partial<LibraryScanStatus> = {}): LibraryScanStatus {
  return {
    libraryId: "lib-1",
    status: "running",
    total: 100,
    registered: 40,
    skipped: 0,
    error: null,
    ...overrides,
  };
}

/** 捕获的事件 handler（photoLibrariesChanged 重拉 / 扫描进度收尾用例驱动） */
let eventHandlers: Array<(event: AppEvent) => void> = [];

/** 广播一条 app://event（存储页自身 + useLibraryScan 两处订阅都会收到） */
function emit(event: AppEvent): void {
  for (const handler of eventHandlers) handler(event);
}

function renderPage() {
  return render(
    <I18nextProvider i18n={i18n}>
      <StoragePage />
    </I18nextProvider>,
  );
}

beforeEach(() => {
  listMock.mockReset().mockResolvedValue([]);
  createMock.mockReset();
  removeMock.mockReset();
  scanStatusMock.mockReset().mockResolvedValue([]);
  scanCancelMock.mockReset().mockResolvedValue(undefined);
  openMock.mockReset().mockResolvedValue(undefined);
  eventHandlers = [];
  subscribeMock.mockReset().mockImplementation(async (handler) => {
    eventHandlers.push(handler);
    return () => {};
  });
});

describe("StoragePage 渲染（M3 存储页）", () => {
  it("照片库卡片列表：名称/路径/在线状态/照片数/容量", async () => {
    listMock.mockResolvedValue([
      lib(),
      lib({ id: "lib-2", name: "移动硬盘库", rootPath: "E:\\备份照片", status: "offline", assetCount: 0, sizeBytes: 0 }),
    ]);
    renderPage();

    const cards = await screen.findAllByTestId("storage-library-card");
    expect(cards).toHaveLength(2);
    expect(cards[0]).toHaveAttribute("data-status", "online");
    expect(cards[1]).toHaveAttribute("data-status", "offline");

    const first = within(cards[0]);
    expect(first.getByText("主照片库")).toBeInTheDocument();
    expect(first.getByTestId("storage-library-path")).toHaveTextContent("I:\\Photos");
    expect(first.getByTestId("storage-library-status")).toHaveTextContent("正常");
    expect(first.getByTestId("storage-library-stats")).toHaveTextContent("120 张照片");
    expect(first.getByTestId("storage-library-stats")).toHaveTextContent("1.0 GB");

    expect(within(cards[1]).getByTestId("storage-library-status")).toHaveTextContent("异常");
  });

  it("无照片库 → 空态引导（新建 / 从文件夹建立入口）", async () => {
    renderPage();

    expect(await screen.findByTestId("storage-empty")).toBeInTheDocument();
    expect(screen.queryByTestId("storage-library-card")).not.toBeInTheDocument();
    expect(screen.getByTestId("storage-empty-create")).toBeInTheDocument();
    expect(screen.queryByTestId("storage-empty-from-folder")).not.toBeInTheDocument();
  });
});

describe("StoragePage 登记入口（photo_library_create，单一按钮 + 对话框内模式二选一）", () => {
  it("新建照片库：默认新建模式 → photoLibraryCreate(name, root, false) → 刷新列表", async () => {
    const created = lib();
    listMock
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce([created]);
    createMock.mockResolvedValue({ ok: true, library: created });
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("storage-empty");

    await user.click(screen.getByTestId("storage-create-library"));
    const dialog = screen.getByTestId("storage-create-dialog");
    expect(dialog).toHaveAttribute("data-mode", "new");
    expect(screen.getByTestId("storage-create-mode-new")).toHaveAttribute("data-selected", "true");

    await user.type(screen.getByTestId("storage-create-name"), "主照片库");
    await user.type(screen.getByTestId("storage-create-root"), "I:\\Photos");
    await user.click(screen.getByTestId("storage-create-confirm"));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith("主照片库", "I:\\Photos", false));
    // 登记成功：对话框关闭 + 本地补拉出新卡 + toast 提示
    await screen.findByText("主照片库");
    expect(screen.queryByTestId("storage-create-dialog")).not.toBeInTheDocument();
    expect(screen.getByTestId("storage-toast")).toHaveTextContent("主照片库");
  });

  it("对话框内切「从已有文件夹建立」：reference=true 登记（只登记不搬文件）", async () => {
    listMock.mockResolvedValue([]);
    createMock.mockResolvedValue({ ok: true, library: lib() });
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("storage-empty");

    // 单一「新建照片库」入口；模式在对话框内切换
    await user.click(screen.getByTestId("storage-create-library"));
    await user.click(screen.getByTestId("storage-create-mode-reference"));
    expect(screen.getByTestId("storage-create-dialog")).toHaveAttribute("data-mode", "reference");

    await user.type(screen.getByTestId("storage-create-name"), "LR 目录");
    await user.type(screen.getByTestId("storage-create-root"), "D:\\Lightroom");
    await user.click(screen.getByTestId("storage-create-confirm"));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith("LR 目录", "D:\\Lightroom", true));
  });

  it("登记业务错误：后端 Err 文案透传内联展示，对话框保持打开", async () => {
    listMock.mockResolvedValue([]);
    createMock.mockResolvedValue({ ok: false, error: "路径与其他照片库重叠" });
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("storage-empty");

    await user.click(screen.getByTestId("storage-create-library"));
    await user.type(screen.getByTestId("storage-create-name"), "库");
    await user.type(screen.getByTestId("storage-create-root"), "I:\\Photos");
    await user.click(screen.getByTestId("storage-create-confirm"));

    expect(await screen.findByTestId("storage-create-error")).toHaveTextContent("路径与其他照片库重叠");
    expect(screen.getByTestId("storage-create-dialog")).toBeInTheDocument();
  });
});

describe("StoragePage 卡片操作", () => {
  it("在系统中打开：open_with_system 打开库根目录", async () => {
    listMock.mockResolvedValue([lib()]);
    const user = userEvent.setup();
    renderPage();
    await screen.findAllByTestId("storage-library-card");

    await user.click(screen.getAllByTestId("storage-open-in-system")[0]);
    await waitFor(() => expect(openMock).toHaveBeenCalledWith("I:\\Photos"));
  });


  it("移除登记：两档确认——仅摘登记 deleteRecords=false / 连记录删 true；永不删照片文件提示在场", async () => {
    listMock.mockResolvedValue([lib({ assetCount: 35 })]);
    removeMock.mockResolvedValue({ recordsDeleted: 0 });
    const user = userEvent.setup();
    renderPage();
    await screen.findAllByTestId("storage-library-card");

    await user.click(screen.getAllByTestId("storage-remove")[0]);
    const dialog = screen.getByTestId("storage-remove-dialog");
    // 用户红线常驻提示
    expect(within(dialog).getByTestId("storage-remove-never-delete")).toHaveTextContent(
      "移除登记永远不会删除磁盘上的照片文件",
    );
    expect(within(dialog).getByTestId("storage-remove-delete-records")).toHaveTextContent("35 张");

    await user.click(within(dialog).getByTestId("storage-remove-keep-records"));
    await waitFor(() => expect(removeMock).toHaveBeenCalledWith("lib-1", false));
    expect(screen.getByTestId("storage-toast")).toHaveTextContent("照片记录保留");
  });

  it("移除登记并删除记录：两段式——需输入「我确认删除」短语才能执行 deleteRecords=true", async () => {
    listMock.mockResolvedValue([lib({ assetCount: 35 })]);
    removeMock.mockResolvedValue({ recordsDeleted: 35 });
    const user = userEvent.setup();
    renderPage();
    await screen.findAllByTestId("storage-library-card");

    await user.click(screen.getAllByTestId("storage-remove")[0]);
    // 第一段：点「连记录删」只进入待确认态，不执行
    await user.click(screen.getByTestId("storage-remove-delete-records"));
    expect(removeMock).not.toHaveBeenCalled();
    expect(screen.getByTestId("storage-remove-confirm-delete")).toBeDisabled();

    // 短语不匹配不可执行
    await user.type(screen.getByTestId("storage-remove-phrase"), "确认删除");
    expect(screen.getByTestId("storage-remove-confirm-delete")).toBeDisabled();

    // 第二段：输入完整短语 → 执行
    await user.clear(screen.getByTestId("storage-remove-phrase"));
    await user.type(screen.getByTestId("storage-remove-phrase"), "我确认删除");
    expect(screen.getByTestId("storage-remove-confirm-delete")).toBeEnabled();
    await user.click(screen.getByTestId("storage-remove-confirm-delete"));

    await waitFor(() => expect(removeMock).toHaveBeenCalledWith("lib-1", true));
    expect(screen.getByTestId("storage-toast")).toHaveTextContent("35");
  });
});

describe("StoragePage 列表刷新", () => {
  it("photoLibrariesChanged 事件 → 重拉列表", async () => {
    listMock.mockResolvedValue([lib()]);
    renderPage();
    await screen.findAllByTestId("storage-library-card");
    expect(listMock).toHaveBeenCalledTimes(1);

    listMock.mockResolvedValue([lib(), lib({ id: "lib-2", name: "第二库", rootPath: "E:\\X" })]);
    for (const handler of eventHandlers) handler({ type: "photoLibrariesChanged" });

    await waitFor(() => expect(listMock).toHaveBeenCalledTimes(2));
    await screen.findByText("第二库");
  });
});

describe("StoragePage 扫描闭环（M4f 从文件夹建立）", () => {
  it("向导基本流程：登记 → 卡片扫描进度（事件推进）→ 取消 → photo_library_scan_cancel", async () => {
    // 空态 → 从文件夹建立（reference=true）→ 登记成功出新卡
    const created = lib({ assetCount: 0, sizeBytes: 0 });
    listMock.mockResolvedValueOnce([]).mockResolvedValue([created]);
    createMock.mockResolvedValue({ ok: true, library: created });
    const user = userEvent.setup();
    renderPage();
    await screen.findByTestId("storage-empty");

    await user.click(screen.getByTestId("storage-create-library"));
    await user.click(screen.getByTestId("storage-create-mode-reference"));
    await user.type(screen.getByTestId("storage-create-name"), "主照片库");
    await user.type(screen.getByTestId("storage-create-root"), "I:\\Photos");
    await user.click(screen.getByTestId("storage-create-confirm"));

    await waitFor(() => expect(createMock).toHaveBeenCalledWith("主照片库", "I:\\Photos", true));
    // 登记即触发后台扫描：toast 说明扫描已开始（区别于新建空库的登记提示）
    expect(screen.getByTestId("storage-toast")).toHaveTextContent("正在后台扫描");
    await screen.findByTestId("storage-library-card");
    // 尚无扫描事件/状态 → 最近同步未知显示「—」（会话内记录，定案不持久化）
    expect(screen.getByTestId("storage-scan-last-sync")).toHaveTextContent("最近同步 —");

    // 递归扫描进度事件：进度行 + 进度条推进
    emit({ type: "libraryScanProgress", libraryId: "lib-1", registered: 40, total: 100 });
    const progress = await screen.findByTestId("storage-scan-progress");
    expect(progress).toHaveAttribute("data-scan-status", "running");
    expect(progress).toHaveTextContent("扫描中 40/100 张");

    emit({ type: "libraryScanProgress", libraryId: "lib-1", registered: 80, total: 100 });
    expect(await screen.findByTestId("storage-scan-progress")).toHaveTextContent("扫描中 80/100 张");

    // 可取消：软信号 photo_library_scan_cancel(libraryId)
    await user.click(screen.getByTestId("storage-scan-cancel"));
    await waitFor(() => expect(scanCancelMock).toHaveBeenCalledWith("lib-1"));
  });

  it("登记完成通知：新增/跳过计数 + 跨库重复提示；卡片记录最近同步时间", async () => {
    listMock.mockResolvedValue([lib()]);
    renderPage();
    await screen.findAllByTestId("storage-library-card");
    emit({ type: "libraryScanProgress", libraryId: "lib-1", registered: 118, total: 128 });
    await screen.findByTestId("storage-scan-progress");

    // 契约扩展字段（M4b 后端补发；前端防御性读取）——registered 的子集计数
    const finished = {
      type: "libraryScanFinished",
      libraryId: "lib-1",
      registered: 120,
      skipped: 8,
      crossLibraryDuplicates: 5,
    } as AppEvent;
    emit(finished);

    const toast = await screen.findByTestId("storage-toast");
    expect(toast).toHaveTextContent("「主照片库」扫描完成");
    expect(toast).toHaveTextContent("新增 120 张");
    expect(toast).toHaveTextContent("跳过 8 张");
    // 「其中 N 张与其他照片库内容相同」——照常登记，只是提示（计划 §四）
    expect(screen.getByTestId("storage-toast-hint")).toHaveTextContent("其中 5 张与其他照片库内容相同");

    // 收尾后：进度行消失，最近同步时间已知（不再是「—」）
    expect(screen.queryByTestId("storage-scan-progress")).not.toBeInTheDocument();
    const lastSync = screen.getByTestId("storage-scan-last-sync");
    expect(lastSync).toHaveTextContent("最近同步");
    expect(lastSync).not.toHaveTextContent("—");
    // 完成通知同时补拉列表（照片数/容量缓存跟上）
    await waitFor(() => expect(listMock.mock.calls.length).toBeGreaterThanOrEqual(2));
  });

  it("后端未发 crossLibraryDuplicates（契约扩展未落地）：通知不显示跨库提示、不报错", async () => {
    listMock.mockResolvedValue([lib()]);
    renderPage();
    await screen.findAllByTestId("storage-library-card");

    emit({ type: "libraryScanFinished", libraryId: "lib-1", registered: 12, skipped: 0 });

    const toast = await screen.findByTestId("storage-toast");
    expect(toast).toHaveTextContent("新增 12 张");
    expect(screen.queryByTestId("storage-toast-hint")).not.toBeInTheDocument();
  });

  it("挂载即有在跑扫描（photo_library_scan_status）→ 直接呈现进度；paused/failed 态文案", async () => {
    listMock.mockResolvedValue([lib(), lib({ id: "lib-2", name: "第二库" }), lib({ id: "lib-3", name: "第三库" })]);
    scanStatusMock.mockResolvedValue([
      scanRow({ libraryId: "lib-1", status: "running", total: 200, registered: 66 }),
      scanRow({ libraryId: "lib-2", status: "paused" }),
      scanRow({ libraryId: "lib-3", status: "failed", registered: 0, total: 0, error: "磁盘不可读" }),
    ]);
    renderPage();
    const cards = await screen.findAllByTestId("storage-library-card");

    const first = within(cards[0]);
    expect(first.getByTestId("storage-scan-progress")).toHaveAttribute("data-scan-status", "running");
    expect(first.getByTestId("storage-scan-progress")).toHaveTextContent("扫描中 66/200 张");

    const second = within(cards[1]);
    expect(second.getByTestId("storage-scan-progress")).toHaveAttribute("data-scan-status", "paused");
    expect(second.getByTestId("storage-scan-progress")).toHaveTextContent("扫描暂停");
    // 让路暂停态同样可取消
    expect(second.getByTestId("storage-scan-cancel")).toBeInTheDocument();

    const third = within(cards[2]);
    expect(third.getByTestId("storage-scan-progress")).toHaveAttribute("data-scan-status", "failed");
    expect(third.getByTestId("storage-scan-progress")).toHaveTextContent("扫描失败：磁盘不可读");
    expect(third.queryByTestId("storage-scan-cancel")).not.toBeInTheDocument();
  });
});
