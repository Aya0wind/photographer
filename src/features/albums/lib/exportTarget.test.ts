import { beforeEach, describe, expect, it, vi } from "vitest";

import {
  LAST_EXPORT_DIR_KEY,
  findContainingLibrary,
  isPathInside,
  loadLastExportDir,
  saveLastExportDir,
} from "./exportTarget";

/** localStorage 桩（避免测试间真实存储串扰；异常注入用） */
function makeStorage(): Storage & { map: Map<string, string> } {
  const map = new Map<string, string>();
  return {
    map,
    getItem: (key: string) => (map.has(key) ? map.get(key)! : null),
    setItem: (key: string, value: string) => {
      map.set(key, value);
    },
    removeItem: (key: string) => {
      map.delete(key);
    },
  } as Storage & { map: Map<string, string> };
}

const LIBS = [
  { id: "lib-1", name: "主照片库", rootPath: "I:\\Photos" },
  { id: "lib-2", name: "外置库", rootPath: "E:\\照片库" },
];

describe("isPathInside（库内判定，Windows 主战场）", () => {
  it("子路径/大小写/正反斜杠/尾分隔符都算库内", () => {
    expect(isPathInside("I:\\Photos\\Export", "I:\\Photos")).toBe(true);
    expect(isPathInside("i:\\photos\\export", "I:\\Photos")).toBe(true);
    expect(isPathInside("I:/Photos/Export", "I:\\Photos")).toBe(true);
    expect(isPathInside("I:\\Photos\\", "I:\\Photos")).toBe(true);
    // 等于 root 本身也算「在库内」（整库根做导出目标同样触发提示）
    expect(isPathInside("I:\\Photos", "I:\\Photos")).toBe(true);
  });

  it("同级前缀名不算库内（Photos2 ≠ Photos\\…）", () => {
    expect(isPathInside("I:\\Photos2020", "I:\\Photos")).toBe(false);
    expect(isPathInside("I:\\Other", "I:\\Photos")).toBe(false);
    expect(isPathInside("", "I:\\Photos")).toBe(false);
  });
});

describe("findContainingLibrary", () => {
  it("命中包含该路径的库；库外/空路径返回 null", () => {
    expect(findContainingLibrary("E:\\照片库\\2026\\LR", LIBS)?.id).toBe("lib-2");
    expect(findContainingLibrary("D:\\Lightroom\\Export", LIBS)).toBeNull();
    expect(findContainingLibrary("   ", LIBS)).toBeNull();
    expect(findContainingLibrary("D:\\X", [])).toBeNull();
  });
});

describe("上次导出位置（localStorage 记忆）", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.restoreAllMocks();
  });

  it("保存后可读取（trim 后落盘）；未存/损坏/空串回退 null", () => {
    expect(loadLastExportDir()).toBeNull();

    saveLastExportDir("  D:\\LR Export  ");
    expect(loadLastExportDir()).toBe("D:\\LR Export");
    expect(JSON.parse(localStorage.getItem(LAST_EXPORT_DIR_KEY)!)).toBe("D:\\LR Export");

    localStorage.setItem(LAST_EXPORT_DIR_KEY, "{oops");
    expect(loadLastExportDir()).toBeNull();

    localStorage.setItem(LAST_EXPORT_DIR_KEY, JSON.stringify("  "));
    expect(loadLastExportDir()).toBeNull();
  });

  it("空串不落盘；存储异常静默（隐私模式）", () => {
    const storage = makeStorage();
    saveLastExportDir("   ", storage);
    expect(storage.map.size).toBe(0);

    const throwing = {
      getItem: () => null,
      setItem: () => {
        throw new Error("quota");
      },
      removeItem: () => {},
    };
    expect(() => saveLastExportDir("D:\\X", throwing)).not.toThrow();
  });
});
