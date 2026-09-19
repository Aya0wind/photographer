import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { Event } from "@tauri-apps/api/event";

import {
  aiFaceDataClear,
  aiModelCancel,
  aiModelDelete,
  aiModelDownload,
  aiModelsStatus,
  assetDetail,
  assetGroupDates,
  assetThumbGet,
  assetsByIds,
  assetsPage,
  cameraList,
  searchSemantic,
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
  peopleList,
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

  it("peopleList 将后端 PersonRow.id 归一为 clusterId，过滤损坏条目", async () => {
    invokeMock.mockResolvedValueOnce([
      { id: 7, name: null, faceCount: 12, coverAssetId: 101 },
      { id: "bad", name: "损坏", faceCount: 1, coverAssetId: 102 },
    ]);

    await expect(peopleList()).resolves.toEqual([
      { clusterId: 7, name: null, faceCount: 12, coverAssetId: 101 },
    ]);
    expect(invokeMock).toHaveBeenCalledWith("people_list", undefined);
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

  it("importStart 传 plan 参数并返回 ok/jobId", async () => {
    invokeMock.mockResolvedValue(42);

    await expect(importStart(PLAN)).resolves.toEqual({ ok: true, jobId: 42 });
    expect(invokeMock).toHaveBeenCalledWith("import_start", { plan: PLAN });
  });

  it("importStart 后端逻辑错误透出 Err 文案（嵌套守卫等）", async () => {
    invokeMock.mockRejectedValue("目标目录不能位于源目录内");

    await expect(importStart(PLAN)).resolves.toEqual({
      ok: false,
      error: "目标目录不能位于源目录内",
    });
  });

  it("importStart invoke 不可用（非 Tauri/命令未注册）返回 error=null", async () => {
    invokeMock.mockRejectedValue(new Error("__TAURI_INTERNALS__ is undefined"));

    await expect(importStart(PLAN)).resolves.toEqual({ ok: false, error: null });
  });

  it("importStart 返回非数字 jobId 视为不可用（error=null）", async () => {
    invokeMock.mockResolvedValue(undefined);

    await expect(importStart(PLAN)).resolves.toEqual({ ok: false, error: null });
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

  it("后端业务错误返回 null，但后端仍可达", async () => {
    invokeMock.mockRejectedValue("raw failure");

    await expect(deviceScan("E:")).resolves.toBe(null);
    await expect(importRetryFailed(7)).resolves.toBe(null);
    expect(isIpcAvailable()).toBe(true);
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

describe("IPC 可用性自愈", () => {
  it("失败置不可用后，任一成功调用即恢复可用（dev 重启竞态不再永久预览模式）", async () => {
    invokeMock.mockRejectedValueOnce(new Error("transient"));
    await expect(deviceScan("E:")).resolves.toBeNull();
    expect(isIpcAvailable()).toBe(false);

    invokeMock.mockResolvedValueOnce({
      id: "E:", name: "SD", kind: "volume",
      filesByKind: { photo: 0, raw: 0, video: 0, other: 0 }, bytesTotal: 0, newFiles: 0,
    });
    await deviceScan("E:");
    expect(isIpcAvailable()).toBe(true);
  });
});


it("文件夹失败透出原始原因，不伪装成后端离线", async () => {
  invokeMock.mockRejectedValue("打开库失败: database is locked");
  await expect(folderScan("D:\\photos", true)).rejects.toBe("打开库失败: database is locked");
  expect(isIpcAvailable()).toBe(true);
});

describe("M3 画廊命令", () => {
  const ASSET = {
    id: 3,
    path: "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_0001.JPG",
    name: "IMG_0001.JPG",
    kind: "photo" as const,
    capturedAt: "2026-09-18T10:20:30",
    camera: "Canon EOS R5",
    sizeBytes: 5242880,
  };

  it("assetsPage 透传 keyset 参数并以 filters 容器键携带过滤条件", async () => {
    invokeMock.mockResolvedValue([ASSET]);

    await expect(
      assetsPage(7, 100, {
        kinds: ["photo", "raw"],
        capturedAfter: "2026-01-01T00:00:00.000Z",
        capturedBefore: "2026-02-01T23:59:59.999Z",
        camera: "Canon",
      }),
    ).resolves.toEqual([ASSET]);
    expect(invokeMock).toHaveBeenCalledWith("assets_page", {
      afterId: 7,
      limit: 100,
      filters: {
        kinds: ["photo", "raw"],
        capturedAfter: "2026-01-01T00:00:00.000Z",
        capturedBefore: "2026-02-01T23:59:59.999Z",
        camera: "Canon",
      },
    });
  });

  it("assetsPage 无 filters 时只带分页参数；失败/非数组回退空数组", async () => {
    invokeMock.mockResolvedValue([]);
    await assetsPage(0, 50);
    expect(invokeMock).toHaveBeenLastCalledWith("assets_page", { afterId: 0, limit: 50 });

    invokeMock.mockRejectedValueOnce(new Error("command not found"));
    await expect(assetsPage(0, 100)).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(assetsPage(0, 100)).resolves.toEqual([]);
  });

  it("assetGroupDates 返回分组统计（含未知日期组 date=null）；失败回退空数组", async () => {
    const groups = [
      { date: null, count: 5, coverAssetId: 9 },
      { date: "2026-09-18", count: 12, coverAssetId: 3 },
    ];
    invokeMock.mockResolvedValueOnce(groups);
    await expect(assetGroupDates()).resolves.toEqual(groups);
    expect(invokeMock).toHaveBeenCalledWith("asset_group_dates", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetGroupDates()).resolves.toEqual([]);
  });

  it("assetDetail 传 id 并按后端实测负载归一（filename/size/duplicate_count→dupCount）", async () => {
    // 后端真实负载：AssetDetailDto = {id, flatten(AssetRow camelCase), duplicate_count（顶层 snake_case）}
    invokeMock.mockResolvedValueOnce({
      id: 3,
      path: "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_0001.JPG",
      filename: "IMG_0001.JPG",
      size: 5242880,
      mtime: "2026-09-18T09:00:00Z",
      xxhash: 123,
      sha256: [1, 2, 3],
      kind: "photo",
      capturedAt: "2026-09-18T10:20:30Z",
      camera: "Canon EOS R5",
      source: "E:",
      createdAt: "2026-09-19T08:00:00Z",
      origin: "imported",
      duplicate_count: 2,
    });
    await expect(assetDetail(3)).resolves.toEqual({
      id: 3,
      path: "Y:\\照片\\SmartPhoto\\2026\\09-18\\IMG_0001.JPG",
      filename: "IMG_0001.JPG",
      size: 5242880,
      kind: "photo",
      capturedAt: "2026-09-18T10:20:30Z",
      camera: "Canon EOS R5",
      lens: null,
      createdAt: "2026-09-19T08:00:00Z",
      dupCount: 2,
      width: null,
      height: null,
      iso: null,
      aperture: null,
      shutter: null,
      focalLength: null,
    });
    expect(invokeMock).toHaveBeenCalledWith("asset_detail", { id: 3 });
  });

  it("assetDetail：EXIF 扩展字段存在即透出（fNumber/exposure 别名、duplicateCount 驼峰兼容）", async () => {
    invokeMock.mockResolvedValueOnce({
      id: 4,
      path: "P",
      filename: "A.NEF",
      size: 1,
      kind: "raw",
      capturedAt: null,
      camera: null,
      createdAt: "2026-09-19T08:00:00Z",
      duplicateCount: 1, // 未来后端修正命名时仍可读
      width: 8192,
      height: 5464,
      iso: 400,
      fNumber: 2.8,
      exposure: "1/250",
      focalLength: 35,
    });
    await expect(assetDetail(4)).resolves.toMatchObject({
      kind: "raw",
      dupCount: 1,
      width: 8192,
      height: 5464,
      iso: 400,
      aperture: 2.8,
      shutter: "1/250",
      focalLength: 35,
      capturedAt: null,
      camera: null,
    });
  });

  it("assetDetail：负载异常（null/非对象/字段类型异常）容错；命令失败返回 null", async () => {
    invokeMock.mockResolvedValueOnce(null);
    await expect(assetDetail(3)).resolves.toBeNull();

    invokeMock.mockResolvedValueOnce("oops");
    await expect(assetDetail(3)).resolves.toBeNull();

    // 字段类型异常 → 归一兜底值而非 undefined 透传
    invokeMock.mockResolvedValueOnce({ id: "bad", size: "bad", camera: 42 });
    await expect(assetDetail(3)).resolves.toEqual({
      id: 0,
      path: "",
      filename: "",
      size: 0,
      kind: "photo",
      capturedAt: null,
      camera: null,
      lens: null,
      createdAt: null,
      dupCount: 0,
      width: null,
      height: null,
      iso: null,
      aperture: null,
      shutter: null,
      focalLength: null,
    });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetDetail(3)).resolves.toBeNull();
  });

  it("assetThumbGet 传 assetId/size；失败返回 null", async () => {
    invokeMock.mockResolvedValueOnce("I:\\SmartPhoto\\主库\\thumbs\\256\\a1-1234.jpg");
    await expect(assetThumbGet(3, 240)).resolves.toBe(
      "I:\\SmartPhoto\\主库\\thumbs\\256\\a1-1234.jpg",
    );
    expect(invokeMock).toHaveBeenCalledWith("asset_thumb_get", { assetId: 3, size: 240 });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetThumbGet(3, 1280)).resolves.toBeNull();
  });

  it("cameraList 返回相机计数清单（cameras_list）；失败/非数组回退空数组", async () => {
    const list = [{ camera: "Canon EOS R5", count: 12 }];
    invokeMock.mockResolvedValueOnce(list);
    await expect(cameraList()).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("cameras_list", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(cameraList()).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(cameraList()).resolves.toEqual([]);
  });
});


describe("M4 AI 命令", () => {
  const MODELS = [
    { id: "siglip2-visual", installed: true, bytesTotal: 160_000_000, downloadedBytes: 160_000_000, version: "v1.0", feature: "semantic", state: "done" },
    { id: "scrfd", installed: false, bytesTotal: 2_500_000, downloadedBytes: 0, version: null, feature: "face", state: "idle" },
  ];

  it("aiModelsStatus 返回模型清单；失败/非数组回退空数组", async () => {
    invokeMock.mockResolvedValueOnce(MODELS);
    await expect(aiModelsStatus()).resolves.toEqual(MODELS);
    expect(invokeMock).toHaveBeenCalledWith("ai_models_status", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(aiModelsStatus()).resolves.toEqual([]);
  });

  it("aiModelDownload/Cancel/Delete 传 id；失败静默", async () => {
    invokeMock.mockResolvedValue(undefined);

    await aiModelDownload("siglip2-visual");
    await aiModelCancel("scrfd");
    await aiModelDelete("arcface");

    expect(invokeMock).toHaveBeenCalledWith("ai_model_download", { id: "siglip2-visual" });
    expect(invokeMock).toHaveBeenCalledWith("ai_model_cancel", { id: "scrfd" });
    expect(invokeMock).toHaveBeenCalledWith("ai_model_delete", { id: "arcface" });

    invokeMock.mockRejectedValue(new Error("nope"));
    await expect(aiModelDownload("x")).resolves.toBeUndefined();
    await expect(aiModelDelete("x")).resolves.toBeUndefined();
  });

  it("aiFaceDataClear 返回布尔；失败回退 false", async () => {
    invokeMock.mockResolvedValueOnce(true);
    await expect(aiFaceDataClear()).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("ai_face_data_clear", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(aiFaceDataClear()).resolves.toBe(false);
  });

  it("searchSemantic 传 camelCase 负载（minScore 可选）；命令错误原样抛出（模型未就绪语义）", async () => {
    const hits = [{ assetId: 7, score: 0.93 }];
    invokeMock.mockResolvedValueOnce(hits);

    await expect(searchSemantic("海边日落", 100)).resolves.toEqual(hits);
    expect(invokeMock).toHaveBeenCalledWith("search_semantic", { query: "海边日落", limit: 100 });

    invokeMock.mockResolvedValueOnce(hits);
    await searchSemantic("猫", 50, 0.2);
    expect(invokeMock).toHaveBeenLastCalledWith("search_semantic", { query: "猫", limit: 50, minScore: 0.2 });

    // 模型未就绪：后端返回明确错误字符串 → 抛给调用方（区别于回退空）
    invokeMock.mockRejectedValueOnce("语义模型未就绪");
    await expect(searchSemantic("海边日落", 100)).rejects.toBe("语义模型未就绪");
  });

  it("assetsByIds 传 ids 数组；失败/非数组回退空数组", async () => {
    const asset = { id: 3, path: "P", name: "A.JPG", kind: "photo", capturedAt: null, camera: null, sizeBytes: 1 };
    invokeMock.mockResolvedValueOnce([asset]);
    await expect(assetsByIds([3])).resolves.toEqual([asset]);
    expect(invokeMock).toHaveBeenCalledWith("assets_by_ids", { ids: [3] });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetsByIds([3])).resolves.toEqual([]);
  });
});
