import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";

import {
  deviceList,
  deviceScan,
  folderScan,
  fsListDirs,
  importCancel,
  importJobsPage,
  importLogsPage,
  importPause,
  importResume,
  importRetryFailed,
  importStart,
  isIpcAvailable,
  resetIpcAvailable,
  subscribeAppEvents,
  type ImportPlan,
} from "./api";

const invokeMock = vi.mocked(invoke);
const listenMock = vi.mocked(listen);

const PLAN: ImportPlan = {
  sourceId: "E:",
  targetRoot: "Y:\\照片",
  dirTemplate: "{YYYY}/{MM-DD}",
  nameTemplate: "{原文件名}",
  duplicatePolicy: "skip",
  skipImported: true,
  streams: 4,
  mode: "copy",
};

beforeEach(() => {
  invokeMock.mockReset();
  resetIpcAvailable();
});

describe("IPC 契约封装", () => {
  it("成功时透传命令名与 camelCase 参数", async () => {
    invokeMock.mockResolvedValue([]);

    await importJobsPage(10, 20);

    expect(invokeMock).toHaveBeenCalledWith("import_jobs_page", { afterId: 10, limit: 20 });
  });

  it("deviceList 成功返回数据", async () => {
    const devices = [{ id: "E:", name: "SD 卡", kind: "volume", filesByKind: { photo: 3, raw: 0, video: 0, other: 0 }, bytesTotal: 100, newFiles: 2 }];
    invokeMock.mockResolvedValue(devices);

    await expect(deviceList()).resolves.toEqual(devices);
    expect(isIpcAvailable()).toBe(true);
  });

  it("deviceScan 传 id 参数", async () => {
    invokeMock.mockResolvedValue(null);

    await deviceScan("E:");

    expect(invokeMock).toHaveBeenCalledWith("device_scan", { id: "E:" });
  });

  it("folderScan 传 path 并返回快照", async () => {
    const snapshot = { id: "FOLDER:D:\\老照片", name: "老照片", kind: "folder", filesByKind: { photo: 3, raw: 0, video: 0, other: 0 }, bytesTotal: 100, newFiles: 3 };
    invokeMock.mockResolvedValue(snapshot);

    await expect(folderScan("D:\\老照片")).resolves.toEqual(snapshot);
    expect(invokeMock).toHaveBeenCalledWith("folder_scan", { path: "D:\\老照片" });
  });

  it("folderScan 失败返回 null 并标记 IPC 不可用", async () => {
    invokeMock.mockRejectedValue(new Error("no backend"));

    await expect(folderScan("D:\\老照片")).resolves.toBeNull();
    expect(isIpcAvailable()).toBe(false);
  });

  it("fsListDirs 无参=盘符根；带 parent 传参", async () => {
    const roots = [{ name: "D:", path: "D:\\", hasSubdirs: true }];
    invokeMock.mockResolvedValue(roots);

    await expect(fsListDirs()).resolves.toEqual(roots);
    expect(invokeMock).toHaveBeenCalledWith("fs_list_dirs", undefined);

    await fsListDirs("D:\\");
    expect(invokeMock).toHaveBeenLastCalledWith("fs_list_dirs", { parent: "D:\\" });
  });

  it("fsListDirs 失败静默返回空数组", async () => {
    invokeMock.mockRejectedValue(new Error("no backend"));

    await expect(fsListDirs("D:\\")).resolves.toEqual([]);
  });

  it("importStart 传 plan 参数并返回 jobId", async () => {
    invokeMock.mockResolvedValue(42);

    await expect(importStart(PLAN)).resolves.toBe(42);
    expect(invokeMock).toHaveBeenCalledWith("import_start", { plan: PLAN });
  });

  it("importPause/Resume/Cancel 传 jobId", async () => {
    invokeMock.mockResolvedValue(undefined);

    await importPause(1);
    await importResume(1);
    await importCancel(1);

    expect(invokeMock).toHaveBeenCalledWith("import_pause", { jobId: 1 });
    expect(invokeMock).toHaveBeenCalledWith("import_resume", { jobId: 1 });
    expect(invokeMock).toHaveBeenCalledWith("import_cancel", { jobId: 1 });
  });

  it("importLogsPage 传 jobId/afterId/limit", async () => {
    invokeMock.mockResolvedValue([]);

    await importLogsPage(7, 100, 50);

    expect(invokeMock).toHaveBeenCalledWith("import_logs_page", { jobId: 7, afterId: 100, limit: 50 });
  });

  it("importRetryFailed 返回新 jobId", async () => {
    invokeMock.mockResolvedValue(99);

    await expect(importRetryFailed(7)).resolves.toBe(99);
    expect(invokeMock).toHaveBeenCalledWith("import_retry_failed", { jobId: 7 });
  });
});

describe("IPC 失败兜底", () => {
  it("列表/分页类失败返回空数组并置 ipcAvailable=false", async () => {
    invokeMock.mockRejectedValue(new Error("command not found"));

    await expect(deviceList()).resolves.toEqual([]);
    await expect(importJobsPage(0, 20)).resolves.toEqual([]);
    await expect(importLogsPage(1, 0, 50)).resolves.toEqual([]);
    expect(isIpcAvailable()).toBe(false);
  });

  it("标量类失败返回 null 并置 ipcAvailable=false", async () => {
    invokeMock.mockRejectedValue("raw failure");

    await expect(deviceScan("E:")).resolves.toBe(null);
    await expect(importStart(PLAN)).resolves.toBe(null);
    await expect(importRetryFailed(7)).resolves.toBe(null);
    expect(isIpcAvailable()).toBe(false);
  });

  it("void 命令失败不抛错（静默）但同样置标志", async () => {
    invokeMock.mockRejectedValue(new Error("nope"));

    await expect(importPause(1)).resolves.toBeUndefined();
    await expect(importResume(1)).resolves.toBeUndefined();
    await expect(importCancel(1)).resolves.toBeUndefined();
    expect(isIpcAvailable()).toBe(false);
  });

  it("resetIpcAvailable 恢复可用状态（测试辅助）", async () => {
    invokeMock.mockRejectedValue(new Error("x"));
    await deviceList();
    expect(isIpcAvailable()).toBe(false);

    resetIpcAvailable();
    expect(isIpcAvailable()).toBe(true);
  });
});

describe("subscribeAppEvents", () => {
  it("订阅唯一通道 app://event 并以 payload 回调", async () => {
    let captured: ((e: Event<unknown>) => void) | undefined;
    listenMock.mockImplementationOnce(async (_channel, cb) => {
      captured = cb;
      return () => {};
    });

    const handler = vi.fn();
    const unsubscribe = await subscribeAppEvents(handler);

    expect(listenMock).toHaveBeenCalledWith("app://event", expect.any(Function));
    expect(typeof unsubscribe).toBe("function");

    captured?.({ event: "app://event", id: 1, payload: { type: "deviceRemoved", id: "E:" } });
    expect(handler).toHaveBeenCalledWith({ type: "deviceRemoved", id: "E:" });
  });

  it("listen 失败时返回 noop 取消函数（dev 预览不崩）", async () => {
    listenMock.mockRejectedValueOnce(new Error("no window.__TAURI__"));

    const unsubscribe = await subscribeAppEvents(vi.fn());

    expect(typeof unsubscribe).toBe("function");
    expect(() => unsubscribe()).not.toThrow();
  });
});
