import { describe, expect, it } from "vitest";

// 注意：zh.json 必须以 ?raw 读取 —— 直接 import 会经 JSON.parse，
// 重复 key 被静默覆盖，无法检测。
import zhRaw from "./zh.json?raw";
import zh from "./zh.json";
import i18n from "./index";

/** 关键 key 清单：侧栏导航 + 各占位页 + 设置页 + 向导/通用按钮（防漏文案） */
const CRITICAL_KEYS = [
  "nav.gallery",
  "nav.search",
  "nav.import",
  "nav.tasks",
  "nav.settings",
  "pages.gallery.title",
  "pages.gallery.desc",
  "pages.search.title",
  "pages.search.desc",
  "pages.import.title",
  "pages.import.desc",
  "pages.tasks.title",
  "pages.tasks.desc",
  "pages.settings.title",
  "pages.settings.desc",
  "pages.settings.currentLibrary",
  "pages.settings.libraryRoot",
  "pages.settings.dbDir",
  "pages.settings.libraryRootUnset",
  "onboarding.title",
  "common.back",
  "common.next",
  "common.done",
  "common.cancel",
] as const;

describe("i18n 初始化", () => {
  it("模块导入后完成初始化，语言为中文", () => {
    expect(i18n.isInitialized).toBe(true);
    expect(i18n.language).toBe("zh");
  });

  it("关键 key 返回中文文案而非 key 本身", () => {
    // 抽查一个具体译文的精确值
    expect(i18n.t("nav.gallery")).toBe("图库");
    expect(i18n.t("pages.settings.libraryRootUnset")).toBe("未设置");

    for (const key of CRITICAL_KEYS) {
      const text = i18n.t(key);
      // i18next 找不到词条时返回 key 本身——这里兜底拦截“漏文案”
      expect(text, `t("${key}") 不应返回 key 本身`).not.toBe(key);
      expect(text.length, `t("${key}") 不应为空串`).toBeGreaterThan(0);
    }
  });

  it("zh.json 中所有词条均可被 t() 解析（无解析失败的 key）", () => {
    const keys = Object.keys(zh);
    expect(keys.length).toBeGreaterThan(0);

    for (const key of keys) {
      expect(i18n.t(key), `t("${key}") 不应返回 key 本身`).not.toBe(key);
    }
  });

  it("确实缺失的 key 返回 key 本身（对照：证明上面的断言有效）", () => {
    expect(i18n.t("definitely.missing.key")).toBe("definitely.missing.key");
  });
});

describe("zh.json 文件质量", () => {
  it("不含重复 key（JSON.parse 会静默覆盖前者）", () => {
    // 匹配所有 JSON key（"xxx" 后跟冒号）；字符串值后面只会跟 , 或 }，不会误报
    const matches = [...zhRaw.matchAll(/"((?:[^"\\]|\\.)*)"\s*:/g)];
    expect(matches.length).toBeGreaterThan(0);

    const counts = new Map<string, number>();
    for (const match of matches) {
      const key = match[1];
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }

    const dupes = [...counts.entries()]
      .filter(([, count]) => count > 1)
      .map(([key, count]) => `${key} x${count}`);

    expect(dupes, `zh.json 存在重复 key：${dupes.join(", ")}`).toEqual([]);
  });
});
