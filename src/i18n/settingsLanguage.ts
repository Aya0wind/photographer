import { useSettingsStore } from "@/stores/settingsStore";
import { setAppLanguage } from "./index";

/** 语言是全局偏好：启动加载、设置修改及远端变更都更新当前界面。 */
export function initAppLanguage(): () => void {
  const apply = (language: string) => {
    void setAppLanguage(language).catch((error) => console.error("Language pack could not be loaded", error));
  };
  apply(useSettingsStore.getState().settings.system.language);
  return useSettingsStore.subscribe((next, previous) => {
    if (next.settings.system.language !== previous.settings.system.language) {
      apply(next.settings.system.language);
    }
  });
}
