import { useTranslation } from "react-i18next";

import PagePlaceholder from "@/app/components/PagePlaceholder";
import { useSettingsStore } from "@/stores/settingsStore";

export default function SettingsPage() {
  const { t } = useTranslation();
  const libraryRoot = useSettingsStore((s) => s.settings.libraryRoot);

  return (
    <PagePlaceholder
      titleKey="pages.settings.title"
      descKey="pages.settings.desc"
    >
      {/* 库根目录：M0 预留展示位，向导（Task 5）写入后生效 */}
      <div className="mt-6 flex items-center justify-between gap-4 border-t border-panel pt-4 text-left">
        <span className="shrink-0 text-sm text-text-secondary">
          {t("pages.settings.libraryRoot")}
        </span>
        <span
          className={`truncate font-mono text-xs ${
            libraryRoot ? "text-text-primary" : "text-text-muted"
          }`}
          title={libraryRoot ?? t("pages.settings.libraryRootUnset")}
        >
          {libraryRoot ?? t("pages.settings.libraryRootUnset")}
        </span>
      </div>
    </PagePlaceholder>
  );
}
