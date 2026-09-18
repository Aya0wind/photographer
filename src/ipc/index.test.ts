import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import { ipc } from "./index";

// setup.ts 已全局 mock @tauri-apps/api/core，这里取回 mock 引用并逐用例覆写
const invokeMock = vi.mocked(invoke);

describe("ipc()", () => {
  beforeEach(() => {
    invokeMock.mockReset();
  });

  it("无 payload 时按原样转发命令名", async () => {
    invokeMock.mockResolvedValue({ schemaVersion: 1 });

    const result = await ipc("settings_get");

    expect(result).toEqual({ schemaVersion: 1 });
    expect(invokeMock).toHaveBeenCalledTimes(1);
    expect(invokeMock).toHaveBeenCalledWith("settings_get", undefined);
  });

  it("命令名与 payload 参数对象原样透传", async () => {
    invokeMock.mockResolvedValue([{ id: "p1" }]);

    const result = await ipc("photos_list", { limit: 10, offset: 0 });

    expect(result).toEqual([{ id: "p1" }]);
    expect(invokeMock).toHaveBeenCalledWith("photos_list", { limit: 10, offset: 0 });
  });

  it("返回值不做任何加工，直接交给调用方", async () => {
    const sentinel = { nested: { deep: [1, 2, 3] } };
    invokeMock.mockResolvedValue(sentinel);

    await expect(ipc("echo")).resolves.toBe(sentinel);
  });

  it("invoke reject 时异常向上抛（不吞错）", async () => {
    invokeMock.mockRejectedValue(new Error("command not found"));

    await expect(ipc("missing_command")).rejects.toThrow("command not found");
  });

  it("拒绝原因为非 Error 值时同样原样向上抛", async () => {
    invokeMock.mockRejectedValue("raw failure");

    await expect(ipc("boom")).rejects.toBe("raw failure");
  });
});
