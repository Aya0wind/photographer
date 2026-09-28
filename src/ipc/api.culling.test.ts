import { beforeEach, describe, expect, it, vi } from "vitest";

import { invoke } from "@tauri-apps/api/core";

import {
  cullDecisionApply,
  cullSessionCreate,
  cullSessionDiscard,
  cullSessionFinish,
  cullSessionList,
  cullSessionOpen,
  cullSessionRename,
  isIpcAvailable,
  resetIpcAvailable,
  type CullSessionDto,
} from "./api";

const invokeMock = vi.mocked(invoke);

/** 合法会话载荷（后端契约形状） */
function sessionDto(overrides: Partial<CullSessionDto> = {}): CullSessionDto {
  return {
    id: 7,
    name: "婚礼 · 初选",
    scope: { kind: "album", albumId: 3, subgroup: null },
    total: 120,
    accepted: 40,
    rejected: 30,
    undecided: 50,
    createdAt: "2026-09-28T10:00:00Z",
    updatedAt: "2026-09-28T11:00:00Z",
    finishedAt: null,
    ...overrides,
  };
}

beforeEach(() => {
  invokeMock.mockReset();
  resetIpcAvailable();
});

describe("选片 IPC 契约封装（Culling V1）", () => {
  it("cullSessionCreate 透传命令名与 scope 载荷，成功回传归一化会话", async () => {
    invokeMock.mockResolvedValue(sessionDto());
    const result = await cullSessionCreate({ kind: "query", assetIds: [1, 2, 3] });

    expect(invokeMock).toHaveBeenCalledWith("cull_session_create", {
      scope: { kind: "query", assetIds: [1, 2, 3] },
    });
    expect(result.ok).toBe(true);
    if (result.ok) expect(result.session.id).toBe(7);
  });

  it("cullSessionCreate 业务错误透传文案；invoke 不可用 error=null", async () => {
    invokeMock.mockRejectedValue("会话内没有照片");
    const business = await cullSessionCreate({ kind: "query", assetIds: [] });
    expect(business).toEqual({ ok: false, error: "会话内没有照片" });

    invokeMock.mockRejectedValue(new Error("command cull_session_create not found"));
    const unavailable = await cullSessionCreate({ kind: "query", assetIds: [1] });
    expect(unavailable).toEqual({ ok: false, error: null });
  });

  it("cullSessionList 透传命令；脏条目剔除、scope 归一", async () => {
    invokeMock.mockResolvedValue([
      sessionDto(),
      sessionDto({ id: 8, scope: { kind: "album", albumId: 2, subgroup: "成片" } }),
      { id: "x" },
      null,
    ]);
    const list = await cullSessionList();

    expect(invokeMock).toHaveBeenCalledWith("cull_session_list", undefined);
    expect(list.map((s) => s.id)).toEqual([7, 8]);
    expect(list[1].scope).toEqual({ kind: "album", albumId: 2, subgroup: "成片" });
  });

  it("cullSessionList 命令失败/非数组回退 []", async () => {
    invokeMock.mockRejectedValue(new Error("__TAURI_INTERNALS__"));
    await expect(cullSessionList()).resolves.toEqual([]);
    expect(isIpcAvailable()).toBe(false);
  });

  it("cullSessionOpen 归一 items（decision 脏值回 null，origin 只认 ai）", async () => {
    invokeMock.mockResolvedValue({
      session: sessionDto(),
      items: [
        { assetId: 1, decision: "accepted", origin: "manual" },
        { assetId: 2, decision: null, origin: "ai" },
        { assetId: 3, decision: "maybe", origin: "weird" },
        { assetId: "bad" },
        "junk",
      ],
    });
    const opened = await cullSessionOpen(7);

    expect(invokeMock).toHaveBeenCalledWith("cull_session_open", { sessionId: 7 });
    expect(opened).not.toBeNull();
    expect(opened?.items).toEqual([
      { assetId: 1, decision: "accepted", origin: "manual" },
      { assetId: 2, decision: null, origin: "ai" },
      { assetId: 3, decision: null, origin: "manual" },
    ]);
  });

  it("cullSessionOpen 失败/形状异常返回 null", async () => {
    invokeMock.mockResolvedValue(null);
    await expect(cullSessionOpen(7)).resolves.toBeNull();
    invokeMock.mockRejectedValue("boom");
    await expect(cullSessionOpen(7)).resolves.toBeNull();
  });

  it("cullDecisionApply 载荷含 sessionId+decisions（单条与批量同接口）；失败 null", async () => {
    invokeMock.mockResolvedValue(sessionDto({ accepted: 41, undecided: 49 }));
    const dto = await cullDecisionApply(7, [{ assetId: 1, decision: "accepted" }]);

    expect(invokeMock).toHaveBeenCalledWith("cull_decision_apply", {
      sessionId: 7,
      decisions: [{ assetId: 1, decision: "accepted" }],
    });
    expect(dto?.accepted).toBe(41);

    invokeMock.mockRejectedValue(new Error("channel closed"));
    await expect(cullDecisionApply(7, [{ assetId: 1, decision: null }])).resolves.toBeNull();
  });

  it("cullSessionRename / cullSessionDiscard：命令名与参数；失败 false", async () => {
    invokeMock.mockResolvedValue(undefined);
    await expect(cullSessionRename(7, "复选")).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("cull_session_rename", { sessionId: 7, name: "复选" });

    await expect(cullSessionDiscard(7)).resolves.toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("cull_session_discard", { sessionId: 7 });

    invokeMock.mockRejectedValue("boom");
    await expect(cullSessionRename(7, "x")).resolves.toBe(false);
    await expect(cullSessionDiscard(7)).resolves.toBe(false);
  });

  it("cullSessionFinish 载荷与结果形状；失败/脏数据 null", async () => {
    invokeMock.mockResolvedValue({ appliedFlag: 40, appliedRating: 0, rejected: 30 });
    const result = await cullSessionFinish(7, {
      acceptedFlag: true,
      acceptedRating: null,
      rejectRejected: true,
    });

    expect(invokeMock).toHaveBeenCalledWith("cull_session_finish", {
      sessionId: 7,
      apply: { acceptedFlag: true, acceptedRating: null, rejectRejected: true },
    });
    expect(result).toEqual({ appliedFlag: 40, appliedRating: 0, rejected: 30 });

    invokeMock.mockResolvedValue("junk");
    await expect(
      cullSessionFinish(7, { acceptedFlag: false, acceptedRating: 3, rejectRejected: false }),
    ).resolves.toBeNull();
    invokeMock.mockRejectedValue("boom");
    await expect(
      cullSessionFinish(7, { acceptedFlag: false, acceptedRating: null, rejectRejected: false }),
    ).resolves.toBeNull();
  });
});
