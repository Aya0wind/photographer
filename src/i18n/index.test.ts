import { afterEach, describe, expect, it } from "vitest";

// 注意：zh.json 必须以 ?raw 读取 —— 直接 import 会经 JSON.parse，
// 重复 key 被静默覆盖，无法检测。
import zhRaw from "./zh.json?raw";
import zh from "./zh.json";
import i18n, { normalizeLanguage, setAppLanguage } from "./index";
import enRaw from "./en.json?raw";
import jaRaw from "./ja.json?raw";
import esRaw from "./es.json?raw";
import traditionalRaw from "./zh-TW.json?raw";
import { formatDateLabel } from "@/features/gallery/lib/assetGroups";
import { smartTagLabel } from "@/features/albums/lib/smartTags";

afterEach(async () => { await setAppLanguage("zh"); });

describe("语言包完整性", () => {
  it.each([['en', enRaw], ['ja', jaRaw], ['es', esRaw], ['zh-TW', traditionalRaw]])("%s 包含全部文案并保留插值变量", (_language, raw) => {
    const pack: Record<string, string> = JSON.parse(raw);
    const keys = [...raw.matchAll(/"((?:[^"\\]|\\.)*)"\s*:/g)].map((match) => match[1]);
    expect(new Set(keys).size).toBe(keys.length);
    for (const [key, source] of Object.entries(zh)) {
      expect(pack[key], key).toBeTruthy();
      expect([...pack[key].matchAll(/\{\{(.*?)\}\}/g)].map((m) => m[1]).sort(), key)
        .toEqual([...source.matchAll(/\{\{(.*?)\}\}/g)].map((m) => m[1]).sort());
    }
  });

  it.each([['zh-Hant-HK', 'zh-TW'], ['zh_CN', 'zh'], ['en-GB', 'en'], ['ja-JP', 'ja'], ['es-MX', 'es'], ['unknown', 'zh']])("兼容语言代码 %s", (code, expected) => {
    expect(normalizeLanguage(code)).toBe(expected);
  });

  it.each([['en', 'Gallery'], ['ja', 'ギャラリー'], ['es', 'Galería'], ['zh-TW', '圖庫']])("%s 可离线加载并切换", async (language, label) => {
    await setAppLanguage(language);
    expect(i18n.t('nav.gallery')).toBe(label);
    expect(document.documentElement.lang).not.toBe('zh-CN');
    for (const key of Object.keys(zh)) expect(i18n.exists(key, { lng: language, fallbackLng: false }), key).toBe(true);
  });

  it("快速切换只应用最后的选择", async () => {
    await Promise.all([setAppLanguage('es'), setAppLanguage('ja'), setAppLanguage('en')]);
    expect(i18n.language).toBe('en');
  });

  it("日期、计数和内置标签随语言切换，自定义标签保持原样", async () => {
    await setAppLanguage('en');
    expect(formatDateLabel('2026-09-29')).toBe('September 29, 2026');
    expect(i18n.t('gallery.groupCount', { count: 1 })).toBe('1 photo');
    expect(i18n.t('gallery.groupCount', { count: 2 })).toBe('2 photos');
    expect(smartTagLabel('风景')).toBe('Landscape');
    expect(smartTagLabel('我的旅行')).toBe('我的旅行');
    await setAppLanguage('es');
    expect(i18n.t('gallery.groupCount', { count: 1 })).toBe('1 foto');
    expect(i18n.t('gallery.groupCount', { count: 2 })).toBe('2 fotos');
  });
});

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
