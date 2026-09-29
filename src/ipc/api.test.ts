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
  thumbGet,
  deviceThumbGet,
  assetsByIds,
  assetsPage,
  assetViewMark,
  burstStats,
  gearStats,
  onThisDay,
  duplicateDelete,
  duplicatesList,
  assetFlagSet,
  assetRatingSet,
  importJobDelete,
  indexRebuild,
  cameraList,
  formatList,
  lensList,
  recentAssets,
  recentViewed,
  searchSemantic,
  deviceList,
  deviceScan,
  folderScan,
  fsListDirs,
  platformCapabilities,
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

it("导入预览调用已注册的路径命令，相机预览传持久对象 ID", async () => {
  invokeMock.mockResolvedValueOnce("C:\\cache\\preview.jpg");
  expect(await thumbGet("E:\\照片\\A.NEF", 256)).toBe("C:\\cache\\preview.jpg");
  expect(invokeMock).toHaveBeenCalledWith("thumb_get_by_path", { path: "E:\\照片\\A.NEF", size: 256 });
  invokeMock.mockResolvedValueOnce(null);
  await deviceThumbGet("camera", "persistent-id", "mtime:123", 256);
  expect(invokeMock).toHaveBeenCalledWith("device_thumb_get", {
    deviceId: "camera", objectId: "persistent-id", version: "mtime:123", size: 256,
  });
});

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
    const devices = [{ id: "E:", name: "SD 卡", kind: "volume", filesByKind: { photo: 3, raw: 0, other: 0 }, bytesTotal: 100, newFiles: 2 }];
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
    const snapshot = { id: "FOLDER:D:\\老照片", name: "老照片", kind: "folder", filesByKind: { photo: 3, raw: 0, other: 0 }, bytesTotal: 100, newFiles: 3 };
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

  it("原生能力保留 false，不把不支持当作空设备列表", async () => {
    const capabilities = {
      filesystemRoots: false, volumeDevices: false, portableDevices: false, hotplug: false,
      systemOpen: false, fileClipboard: false, fileReveal: false, documentUris: false,
    };
    invokeMock.mockResolvedValue(capabilities);
    await expect(platformCapabilities()).resolves.toEqual(capabilities);
    expect(invokeMock).toHaveBeenCalledWith("platform_capabilities", undefined);
  });

  it("能力与严格目录查询透传平台错误", async () => {
    invokeMock.mockRejectedValue(new Error("当前平台尚未实现根枚举"));
    await expect(platformCapabilities()).rejects.toThrow("尚未实现");
    await expect(fsListDirs(undefined, true)).rejects.toThrow("尚未实现");
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
      filesByKind: { photo: 0, raw: 0, other: 0 }, bytesTotal: 0, newFiles: 0,
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
        cameras: ["Canon EOS R5", "Sony A7R5"],
      }),
    ).resolves.toEqual([ASSET]);
    expect(invokeMock).toHaveBeenCalledWith("assets_page", {
      afterId: 7,
      limit: 100,
      filters: {
        kinds: ["photo", "raw"],
        capturedAfter: "2026-01-01T00:00:00.000Z",
        capturedBefore: "2026-02-01T23:59:59.999Z",
        cameras: ["Canon EOS R5", "Sony A7R5"],
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
      megapixels: null,
      aspect: null,
      orientation: null,
      iso: null,
      aperture: null,
      shutter: null,
      focalLength: null,
      flash: null,
      meteringMode: null,
      whiteBalance: null,
      exposureProgram: null,
      software: null,
      artist: null,
      gpsLat: null,
      gpsLon: null,
      format: null,
      rating: null,
      flagged: null,
      aiAnalysis: null,
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
      focalLength: "35",
    });
    await expect(assetDetail(4)).resolves.toMatchObject({
      kind: "raw",
      dupCount: 1,
      width: 8192,
      height: 5464,
      iso: 400,
      aperture: "2.8",
      shutter: "1/250",
      focalLength: "35",
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
      megapixels: null,
      aspect: null,
      orientation: null,
      iso: null,
      aperture: null,
      shutter: null,
      focalLength: null,
      flash: null,
      meteringMode: null,
      whiteBalance: null,
      exposureProgram: null,
      software: null,
      artist: null,
      gpsLat: null,
      gpsLon: null,
      format: null,
      rating: null,
      flagged: null,
      aiAnalysis: null,
    });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetDetail(3)).resolves.toBeNull();
  });

  it("assetThumbGet 传 assetId/size；三态 ready/pending/unavailable（含失败与非对象回退）", async () => {
    invokeMock.mockResolvedValueOnce({
      status: "ready",
      path: "I:\\SmartPhoto\\主库\\thumbs\\256\\a1-1234.jpg",
    });
    await expect(assetThumbGet(3, 240)).resolves.toEqual({
      status: "ready",
      path: "I:\\SmartPhoto\\主库\\thumbs\\256\\a1-1234.jpg",
    });
    expect(invokeMock).toHaveBeenCalledWith("asset_thumb_get", { assetId: 3, size: 240 });

    invokeMock.mockResolvedValueOnce({ status: "pending" });
    await expect(assetThumbGet(3, 1280)).resolves.toEqual({ status: "pending" });

    invokeMock.mockResolvedValueOnce(null);
    await expect(assetThumbGet(3, 1280)).resolves.toEqual({ status: "unavailable" });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetThumbGet(3, 1280)).resolves.toEqual({ status: "unavailable" });
  });

  it("cameraList 返回相机计数清单（camera_list）；失败/非数组回退空数组", async () => {
    const list = [{ camera: "Canon EOS R5", count: 12 }];
    invokeMock.mockResolvedValueOnce(list);
    await expect(cameraList()).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("camera_list", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(cameraList()).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(cameraList()).resolves.toEqual([]);
  });

  it("indexRebuild：index_rebuild 负载（thumb/exif/semantic/face）；失败透传", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await expect(indexRebuild("semantic")).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("index_rebuild", { kind: "semantic" });

    invokeMock.mockRejectedValueOnce("请先在设置中下载模型");
    await expect(indexRebuild("face")).rejects.toBe("请先在设置中下载模型");
  });

  it("assetRatingSet / assetFlagSet：负载 + 失败静默", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await expect(assetRatingSet(3, 5)).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("asset_rating_set", { assetId: 3, rating: 5 });
    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetRatingSet(3, 0)).resolves.toBeUndefined();

    invokeMock.mockResolvedValueOnce(undefined);
    await expect(assetFlagSet(3, true)).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("asset_flag_set", { assetId: 3, flagged: true });
    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetFlagSet(3, false)).resolves.toBeUndefined();
  });

  it("importJobDelete：import_job_delete 负载；失败透传", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await expect(importJobDelete(7)).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("import_job_delete", { jobId: 7 });

    invokeMock.mockRejectedValueOnce(new Error("locked"));
    await expect(importJobDelete(7)).rejects.toThrow("locked");
  });

  it("recentAssets：recent_assets keyset 负载；失败/非数组回退空数组", async () => {
    const list = [{ id: 3, path: "P", name: "A.JPG", kind: "photo", capturedAt: null, camera: null, sizeBytes: 1 }];
    invokeMock.mockResolvedValueOnce(list);
    await expect(recentAssets(101, 100)).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("recent_assets", { afterId: 101, limit: 100 });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(recentAssets(0, 100)).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(recentAssets(0, 100)).resolves.toEqual([]);
  });

  it("recentViewed：recent_viewed(limit) 负载（最后浏览时间 DESC）；失败/非数组回退空数组", async () => {
    const list = [{ id: 3, path: "P", name: "A.JPG", kind: "photo", capturedAt: null, camera: null, sizeBytes: 1 }];
    invokeMock.mockResolvedValueOnce(list);
    await expect(recentViewed(200)).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("recent_viewed", { limit: 200 });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(recentViewed(200)).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(recentViewed(200)).resolves.toEqual([]);
  });

  it("assetViewMark：asset_view_mark 负载；命令失败静默（打点不阻塞查看）", async () => {
    invokeMock.mockResolvedValueOnce(undefined);
    await expect(assetViewMark(7)).resolves.toBeUndefined();
    expect(invokeMock).toHaveBeenCalledWith("asset_view_mark", { assetId: 7 });

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(assetViewMark(7)).resolves.toBeUndefined();
  });

  it("lensList 返回镜头计数清单（lens_list）；失败/非数组回退空数组", async () => {
    const list = [{ lens: "RF24-70mm F2.8 L", count: 8 }];
    invokeMock.mockResolvedValueOnce(list);
    await expect(lensList()).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("lens_list", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(lensList()).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(lensList()).resolves.toEqual([]);
  });

  it("formatList 返回格式计数清单（format_list）；失败/非数组回退空数组", async () => {
    const list = [
      { format: "NEF", count: 5 },
      { format: "JPG", count: 3 },
    ];
    invokeMock.mockResolvedValueOnce(list);
    await expect(formatList()).resolves.toEqual(list);
    expect(invokeMock).toHaveBeenCalledWith("format_list", undefined);

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(formatList()).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(formatList()).resolves.toEqual([]);
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

  it("burstStats 返回分组统计；null/失败/字段缺失静默回 null", async () => {
    invokeMock.mockResolvedValueOnce({ groups: 12, photosInBursts: 47 });
    await expect(burstStats()).resolves.toEqual({ groups: 12, photosInBursts: 47 });
    expect(invokeMock).toHaveBeenCalledWith("burst_stats", undefined);

    invokeMock.mockResolvedValueOnce(null);
    await expect(burstStats()).resolves.toBeNull();

    invokeMock.mockRejectedValueOnce(new Error("command not found"));
    await expect(burstStats()).resolves.toBeNull();

    invokeMock.mockResolvedValueOnce({ groups: 12 }); // photosInBursts 缺失
    await expect(burstStats()).resolves.toBeNull();
  });

  it("onThisDay 返回历年同月日资产；失败/非数组回退空数组", async () => {
    const assets = [{ id: 3, path: "P", name: "A.JPG", kind: "photo", capturedAt: "2023-09-19T10:00:00", camera: null, sizeBytes: 1 }];
    invokeMock.mockResolvedValueOnce(assets);
    await expect(onThisDay()).resolves.toEqual(assets);
    expect(invokeMock).toHaveBeenCalledWith("on_this_day", undefined);

    invokeMock.mockRejectedValueOnce(new Error("command not found"));
    await expect(onThisDay()).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(onThisDay()).resolves.toEqual([]);
  });

  it("gearStats 返回器材分布；null/失败/字段形状异常静默回 null", async () => {
    const stats = {
      cameras: [{ name: "Canon EOS R5", count: 10 }],
      lenses: [{ name: "RF 50mm F1.8", count: 4 }],
      focalBuckets: [
        { label: "24mm", min: 20, max: 35, count: 3 },
        { label: "200+mm", min: 200, max: null, count: 1 },
      ],
      isoBuckets: [{ label: "ISO 100", count: 5 }],
      apertureBuckets: [{ label: "f/2.8", count: 2 }],
      shutterBuckets: [{ label: "1/250s", count: 6 }],
    };
    invokeMock.mockResolvedValueOnce(stats);
    await expect(gearStats()).resolves.toEqual(stats);
    expect(invokeMock).toHaveBeenCalledWith("gear_stats", undefined);

    invokeMock.mockResolvedValueOnce(null);
    await expect(gearStats()).resolves.toBeNull();

    invokeMock.mockRejectedValueOnce(new Error("nope"));
    await expect(gearStats()).resolves.toBeNull();

    // 字段缺失/类型不对：形状校验拦下（不把半截数据交给 UI）
    invokeMock.mockResolvedValueOnce({ cameras: stats.cameras }); // 其余字段缺失
    await expect(gearStats()).resolves.toBeNull();
    invokeMock.mockResolvedValueOnce({ ...stats, isoBuckets: "nope" });
    await expect(gearStats()).resolves.toBeNull();
  });

  it("duplicatesList 传 kind/after/limit（0 基组偏移游标）；失败/非数组回退 []，坏组项剔除", async () => {
    const groups = [
      { kind: "exact", assets: [{ id: 1, path: "P", name: "A.JPG", kind: "photo", capturedAt: null, camera: null, sizeBytes: 1 }] },
      { kind: "similar", assets: [{ id: 2, path: "P", name: "B.JPG", kind: "photo", capturedAt: null, camera: null, sizeBytes: 1 }] },
    ];
    invokeMock.mockResolvedValueOnce(groups);
    await expect(duplicatesList("exact", 20, 20)).resolves.toEqual(groups);
    expect(invokeMock).toHaveBeenCalledWith("duplicates_list", { kind: "exact", after: 20, limit: 20 });

    // 默认参数：after=0 / limit=20
    invokeMock.mockResolvedValueOnce([]);
    await duplicatesList("similar");
    expect(invokeMock).toHaveBeenLastCalledWith("duplicates_list", { kind: "similar", after: 0, limit: 20 });

    invokeMock.mockRejectedValueOnce(new Error("command not found"));
    await expect(duplicatesList("exact")).resolves.toEqual([]);
    invokeMock.mockResolvedValueOnce(null);
    await expect(duplicatesList("exact")).resolves.toEqual([]);

    // 组项形状异常（未知 kind / assets 非数组）剔除，保留合法组
    invokeMock.mockResolvedValueOnce([
      { kind: "nope", assets: [] },
      { kind: 7 },
      groups[0],
    ]);
    await expect(duplicatesList("exact")).resolves.toEqual([groups[0]]);
  });

  it("duplicateDelete 传 assetIds 返回实际删除数；业务错误原样抛出", async () => {
    invokeMock.mockResolvedValueOnce(2);
    await expect(duplicateDelete([4, 5])).resolves.toBe(2);
    expect(invokeMock).toHaveBeenCalledWith("duplicate_delete", { assetIds: [4, 5] });

    // 后端 Err（如未选库）透传给调用方展示
    invokeMock.mockRejectedValueOnce("未选择活动库");
    await expect(duplicateDelete([4])).rejects.toBe("未选择活动库");
  });
});
