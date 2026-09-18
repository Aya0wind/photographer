import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import PagePlaceholder from "@/app/components/PagePlaceholder";
import { importRootOf } from "@/features/onboarding/onboardingConfig";
import { useSettingsStore } from "@/stores/settingsStore";

function InfoRow({ label, value, mono = true }: { label: string; value: string | null; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 border-t border-edge pt-4 text-left">
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

/**
 * 设置页·库管理区（达芬奇式）：当前库信息只读（整理规则是库属性，在新建链/选择器修改）
 * + 库列表展示（激活高亮；切换统一走库选择器，不在原地切换）+「前往库选择器」入口。
 */
export default function SettingsPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const settings = useSettingsStore((s) => s.settings);
  const library = settings.activeLibraryId
    ? settings.libraries.find((lib) => lib.id === settings.activeLibraryId) ?? null
    : null;

  return (
    <PagePlaceholder titleKey="pages.settings.title" descKey="pages.settings.desc">
      <div className="mt-6 flex w-full max-w-xl flex-col gap-5 text-center">
        {/* 标题行 + 前往库选择器 */}
        <div className="flex items-center justify-between">
          <h2 className="text-sm font-semibold text-text-secondary">
            {t("settings.libraries.title")}
          </h2>
          <button
            type="button"
            onClick={() => navigate("/library-picker")}
            className="rounded-md border border-edge px-3 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="settings-goto-picker"
          >
            {t("settings.libraries.goPicker")}
          </button>
        </div>

        {/* 当前库信息（只读） */}
        <div className="flex flex-col gap-4" data-testid="settings-current-library">
          <InfoRow
            label={t("pages.settings.currentLibrary")}
            value={library?.name ?? null}
            mono={false}
          />
          <InfoRow label={t("pages.settings.libraryRoot")} value={library?.photoRoot ?? null} />
          <InfoRow label={t("pages.settings.dbDir")} value={library?.dbDir ?? null} />
          <InfoRow
            label={t("settings.libraries.importRoot")}
            value={
              library ? importRootOf(library.photoRoot, library.importSubdir || "") || library.photoRoot : null
            }
          />
          <InfoRow
            label={t("settings.libraries.template")}
            value={library?.dirTemplate ?? null}
          />
        </div>

        {/* 库列表：仅展示 + 激活高亮（切换走选择器） */}
        {settings.libraries.length > 0 && (
          <div className="flex flex-col gap-1.5" data-testid="settings-library-list">
            {settings.libraries.map((lib) => {
              const isActive = lib.id === settings.activeLibraryId;
              return (
                <div
                  key={lib.id}
                  className={`flex items-center justify-between gap-3 rounded-lg border px-3 py-2 text-left ${
                    isActive ? "border-accent bg-accent/10" : "border-edge"
                  }`}
                  data-testid="settings-library-item"
                  data-active={isActive}
                >
                  <span className="truncate text-sm text-text-primary" title={lib.name}>
                    {lib.name}
                  </span>
                  {isActive ? (
                    <span className="shrink-0 rounded bg-accent/15 px-1.5 py-0.5 text-[11px] text-accent">
                      {t("settings.libraries.active")}
                    </span>
                  ) : (
                    <span
                      className="shrink-0 truncate font-mono text-[11px] text-text-muted"
                      title={lib.dbDir}
                    >
                      {lib.dbDir}
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </PagePlaceholder>
  );
}
