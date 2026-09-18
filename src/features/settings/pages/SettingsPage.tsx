import { useTranslation } from "react-i18next";

import PagePlaceholder from "@/app/components/PagePlaceholder";
import { useSettingsStore } from "@/stores/settingsStore";

function InfoRow({ label, value, mono = true }: { label: string; value: string | null; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-panel pt-4 text-left">
      <span className="shrink-0 text-sm text-text-secondary">{label}</span>
      <span
        className={`truncate text-xs ${mono ? "font-mono" : ""} ${
          value ? "text-text-primary" : "text-text-muted"
        }`}
        title={value ?? ""}
      >
        {value}
      </span>
    </div>
  );
}

export default function SettingsPage() {
  const { t } = useTranslation();
  const library = useSettingsStore((s) => s.settings.activeLibraryId
    ? s.settings.libraries.find((lib) => lib.id === s.settings.activeLibraryId) ?? null
    : null);

  return (
    <PagePlaceholder titleKey="pages.settings.title" descKey="pages.settings.desc">
      {/* 当前库信息：向导写入后生效（完整设置 UI 为 M4） */}
      <div className="mt-6 flex flex-col gap-4">
        <InfoRow
          label={t("pages.settings.currentLibrary")}
          value={library?.name ?? null}
          mono={false}
        />
        <InfoRow
          label={t("pages.settings.libraryRoot")}
          value={library?.photoRoot ?? null}
        />
        <InfoRow
          label={t("pages.settings.dbDir")}
          value={library?.dbDir ?? null}
        />
      </div>
    </PagePlaceholder>
  );
}
