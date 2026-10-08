import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSettingsStore } from "@/stores/settingsStore";

export default function ThemePicker() {
  const { t } = useTranslation();
  const theme = useSettingsStore((state) => state.settings.appearance?.theme ?? "dark");
  const [error, setError] = useState(false);
  const [saving, setSaving] = useState(false);
  async function choose(next: "light" | "dark"): Promise<void> {
    setError(false);
    setSaving(true);
    const { settings, save } = useSettingsStore.getState();
    try {
      await save({ ...settings, appearance: { ...settings.appearance, theme: next } });
    } catch {
      // save 会回退主题，界面给出失败提示。
      setError(true);
    } finally {
      setSaving(false);
    }
  }
  return (
    <div className="flex flex-col items-end gap-2">
      <div className="flex rounded-xl border border-edge bg-bg p-1" role="radiogroup" aria-label={t("settings.appearance.theme")} aria-busy={saving}>
        {(["light", "dark"] as const).map((option) => (
          <button key={option} type="button" role="radio" disabled={saving} aria-checked={theme === option} onClick={() => void choose(option)}
            className={`flex items-center gap-2 rounded-lg px-3 py-2 text-xs transition-colors ${theme === option ? "bg-panel text-text-primary shadow-sm" : "text-text-secondary hover:text-text-primary"}`}
            data-testid={`settings-theme-${option}`}>
            <span aria-hidden="true">{option === "light" ? "☀" : "☾"}</span>{t(`settings.appearance.${option}`)}
          </button>
        ))}
      </div>
      {error && <span role="alert" className="text-xs text-red-500">{t("settings.appearance.saveFailed")}</span>}
    </div>
  );
}
