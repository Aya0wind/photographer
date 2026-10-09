import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
import { clone, DEFAULT_SETTINGS, useSettingsStore } from "./settingsStore";

// 2026-10-09 单库多照片库定案：设置只剩应用级——库注册表/activeLibraryId/
// 库级 AI 档位投影（normalizeLibraryQuality）退役；AI 档位为全局设置。

describe("应用级设置", () => {
  beforeEach(() => {
    useSettingsStore.setState({ settings: clone(DEFAULT_SETTINGS), loaded: true });
    vi.mocked(ipc).mockClear();
  });

  it("默认设置为应用数据库目录缺省（跟随应用数据目录）", () => {
    expect(DEFAULT_SETTINGS.databaseDir).toBeNull();
    expect("libraries" in DEFAULT_SETTINGS).toBe(false);
    expect("activeLibraryId" in DEFAULT_SETTINGS).toBe(false);
  });

  it("update 局部合并 AI 档位并 save 落盘（全局单值，不再按库投影）", async () => {
    useSettingsStore.getState().update({ ai: { qualityTier: "accurate" } });
    const settings = useSettingsStore.getState().settings;
    expect(settings.ai.qualityTier).toBe("accurate");
    await useSettingsStore.getState().save(settings);
    const calls = vi.mocked(ipc).mock.calls;
    expect(calls[calls.length - 1]?.[1]).toMatchObject({ settings: { ai: { qualityTier: "accurate" } } });
  });

  it("load 兜底合并远端缺省字段（后端未实装 databaseDir 时补 null）", async () => {
    vi.mocked(ipc).mockResolvedValueOnce({
      schemaVersion: 1,
      onboardingCompleted: true,
      import: DEFAULT_SETTINGS.import,
    });
    await useSettingsStore.getState().load();
    const settings = useSettingsStore.getState().settings;
    expect(settings.databaseDir).toBeNull();
    expect(settings.onboardingCompleted).toBe(true);
    expect(settings.gallery.mergeRawJpg).toBe(true);
  });

  it("save 失败回滚到修改前快照", async () => {
    const before = useSettingsStore.getState().settings;
    vi.mocked(ipc).mockRejectedValueOnce(new Error("boom"));
    await expect(
      useSettingsStore.getState().save({ ...before, databaseDir: "D:\\SmartPhotoDB" }),
    ).rejects.toThrow("boom");
    expect(useSettingsStore.getState().settings).toBe(before);
  });
});
