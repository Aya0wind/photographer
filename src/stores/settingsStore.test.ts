import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc", () => ({ ipc: vi.fn(async () => undefined) }));
import { ipc } from "@/ipc";
import { clone, DEFAULT_SETTINGS, normalizeLibraryQuality, useSettingsStore, type Library } from "./settingsStore";

const main: Library = { id: "main", name: "主库", dbDir: "I:/main", photoRoot: "Y:/main", streams: 4, configured: true, aiQualityTier: "normal" };
const fresh: Library = { ...main, id: "new", name: "新库", aiQualityTier: "fast" };

describe("库级 AI 方案", () => {
  beforeEach(() => {
    const settings = clone(DEFAULT_SETTINGS);
    settings.libraries = [main, fresh];
    settings.activeLibraryId = main.id;
    useSettingsStore.setState({ settings, loaded: true });
    vi.mocked(ipc).mockClear();
  });

  it("切库恢复各自档位，修改与保存只影响当前库", async () => {
    const store = useSettingsStore.getState();
    await store.save({ ...store.settings, activeLibraryId: "new" });
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("fast");
    useSettingsStore.getState().update({ ai: { qualityTier: "accurate" } });
    await useSettingsStore.getState().save(useSettingsStore.getState().settings);
    const edited = useSettingsStore.getState().settings;
    expect(edited.libraries.map((lib) => lib.aiQualityTier)).toEqual(["normal", "accurate"]);
    await useSettingsStore.getState().save({ ...edited, activeLibraryId: "main" });
    expect(useSettingsStore.getState().settings.ai.qualityTier).toBe("normal");
    const calls = vi.mocked(ipc).mock.calls;
    expect(calls[calls.length - 1]?.[1]).toMatchObject({ settings: { ai: { qualityTier: "normal" } } });
  });

  it("建新库选择快速不会把旧库的普通档改掉", () => {
    const previous = useSettingsStore.getState().settings;
    const result = normalizeLibraryQuality({ ...previous, activeLibraryId: "new", ai: { ...previous.ai, qualityTier: "fast" } }, previous);
    expect(result.libraries[0].aiQualityTier).toBe("normal");
    expect(result.ai.qualityTier).toBe("fast");
  });

  it("旧全局档位迁移后固定到库，不再随其他库变化", () => {
    const legacy = clone(DEFAULT_SETTINGS);
    legacy.libraries = [{ ...main, aiQualityTier: undefined }];
    legacy.activeLibraryId = main.id;
    legacy.ai.qualityTier = "accurate";
    const migrated = normalizeLibraryQuality(legacy);
    expect(migrated.libraries[0].aiQualityTier).toBe("accurate");
    const next = normalizeLibraryQuality({ ...migrated, libraries: [...migrated.libraries, fresh], activeLibraryId: fresh.id }, migrated);
    expect(next.libraries.map((lib) => lib.aiQualityTier)).toEqual(["accurate", "fast"]);
  });
});
