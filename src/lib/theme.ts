import { useSettingsStore } from "@/stores/settingsStore";

type Theme = "dark" | "light";
const THEME_CACHE_KEY = "photographer.appearance.theme";

function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
  document.documentElement.style.colorScheme = theme;
  try { localStorage.setItem(THEME_CACHE_KEY, theme); } catch { /* 配置文件仍是真值。 */ }
}

/** 缓存仅避免启动闪屏；加载后、设置修改及其他窗口变更都跟随配置。 */
export function initAppTheme(): () => void {
  let cached: Theme = "dark";
  try { if (localStorage.getItem(THEME_CACHE_KEY) === "light") cached = "light"; } catch { /* 默认深色。 */ }
  applyTheme(useSettingsStore.getState().loaded ? useSettingsStore.getState().settings.appearance?.theme ?? "dark" : cached);
  return useSettingsStore.subscribe((next, previous) => {
    if (next.loaded && (!previous.loaded || next.settings.appearance?.theme !== previous.settings.appearance?.theme)) {
      applyTheme(next.settings.appearance?.theme ?? "dark");
    }
  });
}
