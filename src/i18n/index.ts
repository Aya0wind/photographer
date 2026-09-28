import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import zh from "./zh.json";

export const APP_LANGUAGES = [
  { code: "zh", label: "中文（简体）", locale: "zh-CN" },
  { code: "zh-TW", label: "中文（繁體）", locale: "zh-TW" },
  { code: "en", label: "English", locale: "en-US" },
  { code: "ja", label: "日本語", locale: "ja-JP" },
  { code: "es", label: "Español", locale: "es-ES" },
] as const;
export type AppLanguage = typeof APP_LANGUAGES[number]["code"];

export function normalizeLanguage(value: string): AppLanguage {
  const code = value.toLowerCase().replace(/_/g, "-");
  if (/^zh-(tw|hk|mo|hant)(-|$)/.test(code)) return "zh-TW";
  if (/^zh(-|$)/.test(code)) return "zh";
  const base = code.split("-")[0];
  return base === "en" || base === "ja" || base === "es" ? base : "zh";
}

export function getIntlLocale(language = i18n.resolvedLanguage ?? i18n.language): string {
  return APP_LANGUAGES.find((item) => item.code === normalizeLanguage(language))!.locale;
}

const loaders = {
  "zh-TW": () => import("./zh-TW.json"),
  en: () => import("./en.json"),
  ja: () => import("./ja.json"),
  es: () => import("./es.json"),
};
const loading = new Map<AppLanguage, Promise<void>>();
let languageRequest = 0;

/** 语言包随应用提供，按需加载；快速切换时只应用最后一次选择。 */
export async function setAppLanguage(value: string): Promise<void> {
  const request = ++languageRequest;
  const language = normalizeLanguage(value);
  if (language !== "zh" && !i18n.hasResourceBundle(language, "translation")) {
    let pending = loading.get(language);
    if (!pending) {
      pending = loaders[language]().then(({ default: resources }) => {
        i18n.addResourceBundle(language, "translation", resources);
      });
      loading.set(language, pending);
      void pending.catch(() => { loading.delete(language); });
    }
    await pending;
  }
  if (request !== languageRequest) return;
  await i18n.changeLanguage(language);
  if (typeof document !== "undefined") document.documentElement.lang = getIntlLocale(language);
}

void i18n.use(initReactI18next).init({
  resources: {
    zh: {
      translation: zh,
    },
  },
  lng: "zh",
  fallbackLng: "zh",
  supportedLngs: APP_LANGUAGES.map((item) => item.code),
  load: "currentOnly",
  interpolation: {
    // React 已经对渲染内容做了转义，无需 i18next 再转义一次
    escapeValue: false,
  },
});

export default i18n;
