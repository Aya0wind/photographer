import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { listen } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";

import {
  deviceList,
  importCancel,
  importJobsPage,
  importPause,
  importResume,
  importRetryFailed,
  type AppEvent,
  type DeviceSnapshot,
  type JobRow,
} from "@/ipc/api";
import { useImportStore, resetImportStoreForTests, initImportStore, seedDevicesFromBackend } from "./importStore";
import { useSettingsStore } from "@/stores/settingsStore";

vi.mock("@/ipc/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/api")>();
  return {
    ...actual,
    deviceList: vi.fn(),
    importJobsPage: vi.fn(),
    importPause: vi.fn(),
    importResume: vi.fn(),
    importCancel: vi.fn(),
    importRetryFailed: vi.fn(),
  };
});

const deviceListMock = vi.mocked(deviceList);
const jobsPageMock = vi.mocked(importJobsPage);
const pauseMock = vi.mocked(importPause);
const resumeMock = vi.mocked(importResume);
const cancelMock = vi.mocked(importCancel);
const retryMock = vi.mocked(importRetryFailed);
const listenMock = vi.mocked(listen);

function snapshot(id: string, name = "SD 卡", newFiles = 12): DeviceSnapshot {
  return {
    id,
    name,
    kind: "volume",
    filesByKind: { photo: 100, raw: 40, other: 1 },
    bytesTotal: 8_000_000_000,
    newFiles,
  };
}

function emit(event: AppEvent): void {
  useImportStore.getState().handleAppEvent(event);
}

beforeEach(() => {
  vi.useFakeTimers();
  resetImportStoreForTests();
  deviceListMock.mockReset().mockImplementation(async () => useImportStore.getState().devices);
  jobsPageMock.mockReset().mockResolvedValue([]);
  pauseMock.mockReset().mockResolvedValue(undefined);
  resumeMock.mockReset().mockResolvedValue(undefined);
  cancelMock.mockReset().mockResolvedValue(undefined);
  retryMock.mockReset().mockResolvedValue(null);
  useSettingsStore.setState((s) => ({
    settings: { ...s.settings, import: { ...s.settings.import, promptOnDevice: true } },
  }));
});

afterEach(() => {
  vi.useRealTimers();
});

describe("设备事件流", () => {
  it("deviceScanned 更新设备列表并进入弹窗队列", () => {
    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:") });

    const s = useImportStore.getState();
    expect(s.devices).toHaveLength(1);
    expect(s.devices[0]).toMatchObject({ id: "E:", newFiles: 12 });
    expect(s.promptQueue).toEqual(["E:"]);
  });

  it("重复扫描同一设备不重复入队，但快照更新", () => {
    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:", "SD 卡", 5) });
    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:", "SD 卡", 3) });

    const s = useImportStore.getState();
    expect(s.promptQueue).toEqual(["E:"]);
    expect(s.devices).toHaveLength(1);
    expect(s.devices[0].newFiles).toBe(3);
  });

  it("多设备排队逐个弹出（ignoreDevice 后露出下一个）", () => {
    emit({ type: "deviceScanned", id: "E:", name: "SD", kind: "volume", snapshot: snapshot("E:") });
    emit({ type: "deviceScanned", id: "F:", name: "CF", kind: "volume", snapshot: snapshot("F:", "CF 卡") });

    expect(useImportStore.getState().promptQueue).toEqual(["E:", "F:"]);

    useImportStore.getState().ignoreDevice("E:");
    expect(useImportStore.getState().promptQueue).toEqual(["F:"]);
  });

  it("promptOnDevice=false 时不弹窗", () => {
    useSettingsStore.setState((s) => ({
      settings: { ...s.settings, import: { ...s.settings.import, promptOnDevice: false } },
    }));

    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:") });

    expect(useImportStore.getState().promptQueue).toEqual([]);
    expect(useImportStore.getState().devices).toHaveLength(1);
  });

  it("deviceArrived 记入 scanning；deviceRemoved 清设备并允许重插再弹", () => {
    emit({ type: "deviceArrived", id: "E:", kind: "volume", name: "SD 卡" });
    expect(useImportStore.getState().scanning).toEqual([{ id: "E:", name: "SD 卡", kind: "volume" }]);

    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:") });
    expect(useImportStore.getState().scanning).toEqual([]);

    useImportStore.getState().ignoreDevice("E:");
    emit({ type: "deviceRemoved", id: "E:" });
    expect(useImportStore.getState().devices).toEqual([]);

    // 重插：重新进入弹窗队列
    emit({ type: "deviceScanned", id: "E:", name: "SD 卡", kind: "volume", snapshot: snapshot("E:") });
    expect(useImportStore.getState().promptQueue).toEqual(["E:"]);
  });
});

describe("任务状态机", () => {
  it("sessionStarted：创建 running 任务并设为当前任务", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 100, totalBytes: 1000 });

    const job = useImportStore.getState().activeJobs[7];
    expect(job).toMatchObject({
      jobId: 7,
      status: "running",
      totalFiles: 100,
      totalBytes: 1000,
      doneFiles: 0,
    });
    expect(useImportStore.getState().currentJobId).toBe(7);
  });

  it("重复 sessionStarted 忽略（非法转移）", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 100, totalBytes: 1000 });
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 999, totalBytes: 9999 });

    expect(useImportStore.getState().activeJobs[7].totalFiles).toBe(100);
  });

  it("running→paused→running 合法；idle（未启动）时 paused 忽略", () => {
    emit({ type: "importPaused", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7]).toBeUndefined();

    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 1, totalBytes: 1 });
    // running 状态下的 resumed 属非法转移（自转移），忽略
    emit({ type: "importResumed", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7].status).toBe("running");

    emit({ type: "importPaused", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7].status).toBe("paused");

    emit({ type: "importResumed", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7].status).toBe("running");
  });

  it("paused→cancelled 合法；终态后 finished/paused 均忽略", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 1, totalBytes: 1 });
    emit({ type: "importPaused", jobId: 7 });
    emit({ type: "importCancelled", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7].status).toBe("cancelled");

    emit({
      type: "importSessionFinished",
      jobId: 7,
      stats: { totalFiles: 1, doneFiles: 1, skippedDuplicates: 0, failedFiles: 0, totalBytes: 1, doneBytes: 1, elapsedMs: 10, bytesPerSec: 100 },
    });
    emit({ type: "importPaused", jobId: 7 });
    expect(useImportStore.getState().activeJobs[7].status).toBe("cancelled");
    expect(useImportStore.getState().summary).toBeNull();
  });
});

describe("进度节流（150ms）", () => {
  it("节流窗口内多个事件只落一次、且取最后一条", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 10_000 });

    emit({ type: "importFileProgress", jobId: 7, doneFiles: 1, doneBytes: 100, settledBytes: 100, currentFile: "A.CR3", bytesPerSec: 1 });
    emit({ type: "importFileProgress", jobId: 7, doneFiles: 2, doneBytes: 200, settledBytes: 200, currentFile: "B.CR3", bytesPerSec: 2 });
    emit({ type: "importFileProgress", jobId: 7, doneFiles: 3, doneBytes: 300, settledBytes: 300, currentFile: "C.CR3", bytesPerSec: 3 });

    // 未到 150ms：渲染状态不更新
    expect(useImportStore.getState().activeJobs[7].doneFiles).toBe(0);
    expect(useImportStore.getState().currentJobId).toBe(7);

    vi.advanceTimersByTime(150);

    const job = useImportStore.getState().activeJobs[7];
    expect(job.doneFiles).toBe(3);
    expect(job.doneBytes).toBe(300);
    expect(job.currentFile).toBe("C.CR3");
    expect(job.bytesPerSec).toBe(3);
  });

  it("flush 后新事件重新开窗（第二个 150ms 周期）", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 10_000 });
    emit({ type: "importFileProgress", jobId: 7, doneFiles: 1, doneBytes: 100, settledBytes: 100, currentFile: "A", bytesPerSec: 1 });
    vi.advanceTimersByTime(150);
    expect(useImportStore.getState().activeJobs[7].doneFiles).toBe(1);

    emit({ type: "importFileProgress", jobId: 7, doneFiles: 5, doneBytes: 500, settledBytes: 500, currentFile: "E", bytesPerSec: 5 });
    expect(useImportStore.getState().activeJobs[7].doneFiles).toBe(1);
    vi.advanceTimersByTime(150);
    expect(useImportStore.getState().activeJobs[7].doneFiles).toBe(5);
  });

  it("sessionFinished 立即以 stats 定格终态并生成总结快照", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 10, totalBytes: 10_000 });
    emit({ type: "importFileProgress", jobId: 7, doneFiles: 9, doneBytes: 9_000, settledBytes: 9_000, currentFile: "X", bytesPerSec: 50 });
    // 尚未 flush 就 finished：pending 应被丢弃，以 stats 为准
    emit({
      type: "importSessionFinished",
      jobId: 7,
      stats: { totalFiles: 10, doneFiles: 10, skippedDuplicates: 2, failedFiles: 1, totalBytes: 10_000, doneBytes: 10_000, elapsedMs: 4000, bytesPerSec: 2500 },
    });

    const job = useImportStore.getState().activeJobs[7];
    expect(job.status).toBe("done");
    expect(job.doneFiles).toBe(10);
    expect(job.currentFile).toBe("");

    const summary = useImportStore.getState().summary;
    expect(summary?.jobId).toBe(7);
    expect(summary?.stats.skippedDuplicates).toBe(2);

    // 之后的定时器 flush 不应复活进度（pending 已被清除）
    vi.advanceTimersByTime(300);
    expect(useImportStore.getState().activeJobs[7].doneFiles).toBe(10);

    useImportStore.getState().dismissSummary();
    expect(useImportStore.getState().summary).toBeNull();
  });

  it("pendingJobMode 竞态：sessionStarted 先于 importStart 返回也能归位移动模式", () => {
    // 向导在调用 importStart 前预挂模式（真实场景：事件与 invoke 返回到达顺序不定）
    useImportStore.getState().setPendingJobMode("move");
    emit({ type: "importSessionStarted", jobId: 9, totalFiles: 2, totalBytes: 20 });
    emit({
      type: "importSessionFinished",
      jobId: 9,
      stats: { totalFiles: 2, doneFiles: 2, skippedDuplicates: 0, failedFiles: 0, totalBytes: 20, doneBytes: 20, elapsedMs: 1000, bytesPerSec: 20, moved: 2 },
    });

    const state = useImportStore.getState();
    expect(state.jobModes[9]).toBe("move");
    expect(state.pendingJobMode).toBeNull();
    expect(state.summary?.mode).toBe("move");
  });

  it("无 pendingJobMode 时 sessionStarted 回退 copy 并保留 pending 供后续任务", () => {
    emit({ type: "importSessionStarted", jobId: 10, totalFiles: 1, totalBytes: 1 });
    expect(useImportStore.getState().jobModes[10]).toBe("copy");
  });
});

describe("失败清单与总结", () => {
  it("importFileCompleted 的失败 state 收集进总结弹窗", () => {
    emit({ type: "importSessionStarted", jobId: 7, totalFiles: 3, totalBytes: 30 });
    emit({ type: "importFileCompleted", jobId: 7, src: "E:/A.CR3", dst: "Y:/A.CR3", state: "ok" });
    emit({ type: "importFileCompleted", jobId: 7, src: "E:/B.CR3", dst: "", state: "failed" });
    emit({
      type: "importSessionFinished",
      jobId: 7,
      stats: { totalFiles: 3, doneFiles: 2, skippedDuplicates: 0, failedFiles: 1, totalBytes: 30, doneBytes: 20, elapsedMs: 1000, bytesPerSec: 20 },
    });

    expect(useImportStore.getState().summary?.failures).toEqual([
      { src: "E:/B.CR3", dst: "", state: "failed" },
    ]);
  });

  it("appError 记录 lastError", () => {
    emit({ type: "appError", level: "warn", message: "目标盘写入偏慢", recoverable: true });
    expect(useImportStore.getState().lastError).toEqual({
      level: "warn",
      message: "目标盘写入偏慢",
      recoverable: true,
    });
  });
});

describe("历史任务分页", () => {
  function jobRow(id: number): JobRow {
    return {
      id,
      kind: "import",
      deviceId: "E:",
      deviceName: "SD 卡",
      status: "done",
      totalFiles: id,
      totalBytes: id * 100,
      statsJson: "{}",
      startedAt: 1_700_000_000_000 + id,
      finishedAt: 1_700_000_000_000 + id + 5000,
    };
  }

  it("首页加载并推进游标；追加第二页", async () => {
    // 首页给满一页（20 行）才视为未取尽
    const fullPage = Array.from({ length: 20 }, (_, i) => jobRow(20 - i));
    jobsPageMock.mockResolvedValueOnce(fullPage).mockResolvedValueOnce([jobRow(1)]);

    await useImportStore.getState().loadHistory(true);
    expect(jobsPageMock).toHaveBeenCalledWith(0, 20);
    expect(useImportStore.getState().history.rows).toHaveLength(20);
    expect(useImportStore.getState().history.cursor).toBe(1);
    expect(useImportStore.getState().history.exhausted).toBe(false);

    await useImportStore.getState().loadHistory();
    expect(jobsPageMock).toHaveBeenLastCalledWith(1, 20);
    expect(useImportStore.getState().history.rows).toHaveLength(21);
    expect(useImportStore.getState().history.rows[20]?.id).toBe(1);
  });

  it("返回不足一页视为取尽，后续不再请求", async () => {
    jobsPageMock.mockResolvedValueOnce([jobRow(5)]);

    await useImportStore.getState().loadHistory(true);
    expect(useImportStore.getState().history.exhausted).toBe(true);

    jobsPageMock.mockClear();
    await useImportStore.getState().loadHistory();
    expect(jobsPageMock).not.toHaveBeenCalled();
  });

  it("加载中防重入", async () => {
    let resolveFirst!: (rows: JobRow[]) => void;
    jobsPageMock.mockImplementationOnce(
      () => new Promise<JobRow[]>((resolve) => (resolveFirst = resolve)),
    );

    const first = useImportStore.getState().loadHistory(true);
    const second = useImportStore.getState().loadHistory();
    resolveFirst([]);
    await Promise.all([first, second]);

    expect(jobsPageMock).toHaveBeenCalledTimes(1);
  });
});

describe("任务动作（命令透传，状态由事件驱动）", () => {
  it("pause/resume/cancel/retry 调用对应 api", async () => {
    await useImportStore.getState().pauseJob(7);
    expect(pauseMock).toHaveBeenCalledWith(7);

    await useImportStore.getState().resumeJob(7);
    expect(resumeMock).toHaveBeenCalledWith(7);

    await useImportStore.getState().cancelJob(7);
    expect(cancelMock).toHaveBeenCalledWith(7);

    retryMock.mockResolvedValueOnce(99);
    await expect(useImportStore.getState().retryFailed(7)).resolves.toBe(99);
    expect(retryMock).toHaveBeenCalledWith(7);
  });
});

describe("initImportStore 事件订阅", () => {
  it("订阅 app://event 并把 payload 转发给 handleAppEvent", async () => {
    let captured: ((e: Event<unknown>) => void) | undefined;
    listenMock.mockReset();
    listenMock.mockImplementationOnce(async (_channel, cb) => {
      captured = cb;
      return () => {};
    });

    await initImportStore();

    expect(listenMock).toHaveBeenCalledWith("app://event", expect.any(Function));
    captured?.({ event: "app://event", id: 1, payload: { type: "deviceRemoved", id: "Z:" } });
    expect(useImportStore.getState().devices.find((d) => d.id === "Z:")).toBeUndefined();

    // subscribeAppEvents 依赖被 mock 的 listen：再次调用 initImportStore 不重复绑定
    listenMock.mockClear();
    await initImportStore();
    expect(listenMock).not.toHaveBeenCalled();
  });
});


describe("设备状态补偿与重连", () => {
  it("先显示已发现设备，扫描失败仍保持可见", () => {
    emit({ type: "deviceArrived", id: "cam", kind: "mtp", name: "相机" });
    expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
    emit({ type: "deviceScanFailed", id: "cam", message: "暂不可读" });
    expect(useImportStore.getState().devices[0]).toMatchObject({ id: "cam", scanStatus: "failed", scanError: "暂不可读" });
    expect(useImportStore.getState().scanning).toEqual([]);
  });

  it("补拉列表清理漏掉断开事件的设备与文件缓存", async () => {
    useImportStore.getState().addDevice(snapshot("E:"));
    useImportStore.getState().setSourceFiles("E:", []);
    deviceListMock.mockResolvedValueOnce([]);
    await seedDevicesFromBackend();
    expect(useImportStore.getState().devices).toEqual([]);
    expect(useImportStore.getState().sourceFiles).toEqual({});
  });

  it("请求失败保留现有设备", async () => {
    useImportStore.getState().addDevice(snapshot("E:"));
    deviceListMock.mockRejectedValueOnce(new Error("IPC failed"));
    await seedDevicesFromBackend();
    expect(useImportStore.getState().devices.map((d) => d.id)).toEqual(["E:"]);
  });

  it("迟到列表不能复活请求期间已断开的设备", async () => {
    let resolve!: (devices: DeviceSnapshot[]) => void;
    deviceListMock.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const request = seedDevicesFromBackend();
    emit({ type: "deviceRemoved", id: "E:" });
    resolve([snapshot("E:")]);
    await request;
    expect(useImportStore.getState().devices).toEqual([]);
  });

  it("迟到空列表不能删除请求期间新到达的设备", async () => {
    let resolve!: (devices: DeviceSnapshot[]) => void;
    deviceListMock.mockImplementationOnce(() => new Promise((done) => { resolve = done; }));
    const request = seedDevicesFromBackend();
    emit({ type: "deviceArrived", id: "E:", kind: "volume", name: "新连接" });
    resolve([]);
    await request;
    expect(useImportStore.getState().devices[0].name).toBe("新连接");
  });

  it("快速移除再扫描清除旧文件并恢复显示", () => {
    useImportStore.getState().addDevice(snapshot("E:"));
    useImportStore.getState().setSourceFiles("E:", []);
    emit({ type: "deviceRemoved", id: "E:" });
    emit({ type: "deviceArrived", id: "E:", kind: "volume", name: "新连接" });
    emit({ type: "deviceScanned", id: "E:", kind: "volume", name: "新连接", snapshot: snapshot("E:", "新连接") });
    expect(useImportStore.getState().devices[0]).toMatchObject({ name: "新连接", scanStatus: "ready" });
    expect(useImportStore.getState().sourceFiles["E:"]).toBeUndefined();
  });
});


it("扫描中的增量可立即读取，扫描完成保留文件避免二次枚举", () => {
  emit({ type: "deviceArrived", id: "cam", kind: "mtp", name: "相机" });
  const file = { id: "1", relPath: "DCIM/A.JPG", size: 10, mtime: "2026-09-19T00:00:00Z" };
  emit({ type: "deviceFilesProgress", id: "cam", files: [file] });
  expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
  expect(useImportStore.getState().sourceFiles.cam[0].name).toBe("A.JPG");
  emit({ type: "deviceFilesProgress", id: "cam", files: [file, { ...file, id: "2", relPath: "DCIM/B.JPG" }] });
  expect(useImportStore.getState().sourceFiles.cam).toHaveLength(2);
  emit({ type: "deviceScanned", id: "cam", kind: "mtp", name: "相机", snapshot: { ...snapshot("cam"), kind: "mtp" } });
  expect(useImportStore.getState().sourceFiles.cam).toHaveLength(2);
});


it("卡拔出后保留空槽位展示，清理旧卡清单；再插卡立即进入扫描状态", async () => {
  const original = snapshot("G:");
  useImportStore.setState({ devices: [original], sourceFiles: { "G:": [{ path: "A.ARW", name: "A.ARW", dir: "", kind: "raw", size: 10 }] }, promptQueue: ["G:"] });
  deviceListMock.mockResolvedValueOnce([{ ...original, mediaPresent: false }]);
  await seedDevicesFromBackend();
  expect(useImportStore.getState().devices[0].mediaPresent).toBe(false);
  expect(useImportStore.getState().sourceFiles["G:"]).toBeUndefined();
  expect(useImportStore.getState().promptQueue).toEqual([]);
  emit({ type: "deviceArrived", id: "G:", name: "SD 卡", kind: "volume" });
  expect(useImportStore.getState().devices[0].mediaPresent).toBe(true);
  expect(useImportStore.getState().devices[0].scanStatus).toBe("scanning");
});
