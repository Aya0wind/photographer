import { useState, type ReactNode } from "react";
import { useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";
import { APP_LANGUAGES, normalizeLanguage } from "@/i18n";

import ThemePicker from "@/features/settings/components/ThemePicker";
import DatabaseSettings from "@/features/settings/components/DatabaseSettings";
import AiTab from "@/features/settings/AiTab";
import { indexNewTags, loadSmartTags, saveSmartTags, unindexedTags, smartTagLabel } from "@/features/albums/lib/smartTags";
import { useSettingsStore, type DeepPartial, type Settings } from "@/stores/settingsStore";

/**
 * 设置页（经典工业风选项卡）：常规 / 外观 / 画廊 / 导入 / AI 水平 tab
 * （下划线选中、紧凑行高）。每项行式布局——标签（左）+ 控件（右）+
 * 说明小字（下一行），分组用 uppercase 小节标题，不用卡片框。
 * 修改即存：settingsStore.update + save（IPC 失败本地仍生效）。
 * 旧「库」tab 已随 2026-10-09 单库多照片库定案退役：照片库管理（新建/
 * 从文件夹建立/重定位/移除登记）由左侧导航「存储」页承担（M3 已实装）；
 * 数据库注册表（多数据库修正：新建/切换/移除）卡片置于「常规」tab 顶部。
 */

type SettingsTab = "general" | "appearance" | "gallery" | "import" | "ai";

const TABS: { key: SettingsTab; labelKey: string }[] = [
  { key: "general", labelKey: "settings.tab.general" },
  { key: "appearance", labelKey: "settings.tab.appearance" },
  { key: "gallery", labelKey: "settings.tab.gallery" },
  { key: "import", labelKey: "settings.tab.import" },
  { key: "ai", labelKey: "settings.tab.ai" },
];

/** 修改即存：本地合并 + 持久化（失败静默，本地已更新） */
function commit(partial: DeepPartial<Settings>): void {
  const { update, save } = useSettingsStore.getState();
  update(partial);
  void save(useSettingsStore.getState().settings);
}


export function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <h3 className="mt-1 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
      {children}
    </h3>
  );
}

/** 行式设置项：标签（左）+ 控件（右）+ 说明小字（下一行），行高约 36px */
export function SettingRow({
  label,
  desc,
  children,
  testId,
}: {
  label: ReactNode;
  desc?: string;
  children: ReactNode;
  testId?: string;
}) {
  return (
    <div
      className="flex min-h-[36px] items-center justify-between gap-8 border-b border-edge/40 py-2"
      data-testid={testId}
    >
      <div className="min-w-0">
        <div className="text-xs text-text-primary">{label}</div>
        {desc && <div className="mt-0.5 text-[11px] leading-relaxed text-text-muted">{desc}</div>}
      </div>
      <div className="flex shrink-0 items-center">{children}</div>
    </div>
  );
}

export function Toggle({
  checked,
  disabled = false,
  onChange,
  label,
  testId,
}: {
  checked: boolean;
  disabled?: boolean;
  onChange: (next: boolean) => void;
  label: string;
  testId?: string;
}) {
  return (
    <input
      type="checkbox"
      role="switch"
      checked={checked}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
      aria-label={label}
      data-testid={testId}
      className="ui-switch disabled:opacity-40"
    />
  );
}

export const SELECT_CLASS =
  "rounded-md border border-edge bg-bg px-2 py-1 text-xs text-text-primary outline-none transition-colors focus:border-accent disabled:opacity-40";

/** 智能相册显示的标签（简化存储：localStorage，不走 settings） */
function AlbumTagsSetting() {
  const { t } = useTranslation();
  const [tags, setTags] = useState<string[]>(loadSmartTags);
  const [newTag, setNewTag] = useState("");
  const [pending, setPending] = useState(() => unindexedTags(loadSmartTags()).length);
  const [progress, setProgress] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const updateTags = (next: string[]) => {
    saveSmartTags(next);
    setTags(next);
    setPending(unindexedTags(next).length);
    setError(null);
  };
  const addTag = () => {
    const tag = newTag.trim();
    if (!tag) return;
    if (!tags.includes(tag)) updateTags([...tags, tag]);
    setNewTag("");
  };
  const build = async () => {
    setBusy(true);
    setError(null);
    try {
      await indexNewTags(tags, (done, total) => {
        setProgress(`${done}/${total}`);
        setPending(unindexedTags(tags).length);
      });
      setPending(unindexedTags(tags).length);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy(false);
      setProgress(null);
    }
  };
  return (
    <div className="border-b border-edge/40 py-2" data-testid="settings-row-album-tags">
      <div className="text-xs text-text-primary">{t("settings.albums.visibleTags")}</div>
      <div className="mt-0.5 text-[11px] leading-relaxed text-text-muted">{t("settings.albums.visibleTagsDesc")}</div>
      <div className="mt-3 flex w-full flex-col items-start gap-2" data-testid="settings-album-tags">
        <div className="flex flex-wrap gap-1.5">
          {tags.map((tag) => (
            <span key={tag} className="inline-flex items-center gap-1 rounded-md border border-edge bg-panel px-2 py-1 text-[11px] text-text-secondary" data-testid="settings-album-tag" data-tag={tag}>
              {smartTagLabel(tag)}
              <button type="button" onClick={() => updateTags(tags.filter((item) => item !== tag))} aria-label={`${t("settings.albums.removeTag")} ${smartTagLabel(tag)}`} className="ml-0.5 text-text-muted hover:text-red-400">×</button>
            </span>
          ))}
        </div>
        <div className="flex flex-wrap items-center gap-1.5">
          <input value={newTag} onChange={(event) => setNewTag(event.target.value)} onKeyDown={(event) => { if (event.key === "Enter") { event.preventDefault(); addTag(); } }} aria-label={t("settings.albums.addTag")} className="h-7 w-28 rounded-md border border-edge bg-bg px-2 text-xs text-text-primary outline-none focus:border-accent" />
          <button type="button" onClick={addTag} className="h-7 rounded-md border border-edge px-2 text-xs text-text-secondary hover:border-accent">{t("settings.albums.addTag")}</button>
          <button type="button" onClick={() => void build()} disabled={busy || pending === 0} className="h-7 rounded-md border border-accent px-2 text-xs text-accent disabled:border-edge disabled:text-text-muted" data-testid="settings-index-tags">
            {busy ? `${t("settings.albums.indexing")} ${progress ?? ""}` : pending > 0 ? `${t("settings.albums.indexTags")} (${pending})` : t("settings.albums.indexed")}
          </button>
        </div>
        {error && <span role="alert" className="text-[11px] text-red-400">{error}</span>}
      </div>
    </div>
  );
}

export default function SettingsPage() {
  const { t } = useTranslation();
  const settings = useSettingsStore((s) => s.settings);
  // 深链 ?tab=ai（语义门禁/未就绪引导卡的「去设置」直达；非法值回常规）
  const [searchParams] = useSearchParams();
  const urlTab = searchParams.get("tab");
  const [tab, setTab] = useState<SettingsTab>(
    TABS.some((item) => item.key === urlTab) ? (urlTab as SettingsTab) : "general",
  );

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 shrink-0 items-center border-b border-edge px-4">
        <h1 className="text-sm font-semibold text-text-primary">{t("pages.settings.title")}</h1>
      </header>

      {/* 与下方内容使用相同边距、最大宽度和滚动条预留空间。 */}
      <div className="sp-scroll shrink-0 overflow-y-hidden px-5 pt-3">
      <div
        role="tablist"
        aria-label={t("pages.settings.title")}
        className="ui-settings-content ui-glass flex min-h-10 w-full flex-wrap items-stretch gap-1 rounded-xl border p-1"
      >
        {TABS.map((item) => {
          const active = tab === item.key;
          return (
            <button
              key={item.key}
              type="button"
              role="tab"
              aria-selected={active}
              onClick={() => setTab(item.key)}
              className={`rounded-lg px-4 py-2 text-xs transition-colors ${
                active
                  ? "bg-panel text-text-primary shadow-sm"
                  : "text-text-secondary hover:bg-panel/50 hover:text-text-primary"
              }`}
              data-testid={`settings-tab-${item.key}`}
            >
              {t(item.labelKey)}
            </button>
          );
        })}
      </div>
      </div>

      <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-5 py-5">
        <div className="ui-settings-content ui-glass flex w-full flex-col gap-3 rounded-2xl border border-edge p-5">
          {tab === "general" && (
            <>
              <SectionTitle>{t("settings.section.database")}</SectionTitle>
              <DatabaseSettings />
              <SectionTitle>{t("settings.section.system")}</SectionTitle>
              <SettingRow
                label={t("settings.closeBehavior")}
                desc={t("settings.closeBehaviorDesc")}
                testId="settings-row-close-behavior"
              >
                <div
                  role="radiogroup"
                  aria-label={t("settings.closeBehavior")}
                  className="flex rounded-md border border-edge bg-bg p-0.5"
                  data-testid="settings-close-behavior"
                >
                  {(
                    [
                      { value: true, labelKey: "settings.closeToTray" },
                      { value: false, labelKey: "settings.quitOnClose" },
                    ] as const
                  ).map((option) => (
                    <button
                      key={option.labelKey}
                      type="button"
                      role="radio"
                      aria-checked={settings.system.closeToTray === option.value}
                      onClick={() => commit({ system: { closeToTray: option.value } })}
                      className={`rounded px-2.5 py-1 text-[11px] transition-colors ${
                        settings.system.closeToTray === option.value
                          ? "bg-accent text-black"
                          : "text-text-secondary hover:text-text-primary"
                      }`}
                    >
                      {t(option.labelKey)}
                    </button>
                  ))}
                </div>
              </SettingRow>
              <SettingRow label={t("settings.launchAtLogin")} desc={t("settings.launchAtLoginDesc")}>
                <Toggle
                  checked={settings.system.launchAtLogin}
                  label={t("settings.launchAtLogin")}
                  onChange={(next) => commit({ system: { launchAtLogin: next } })}
                />
              </SettingRow>
              <SettingRow label={t("settings.language")} desc={t("settings.languageDesc")}>
                <select
                  value={normalizeLanguage(settings.system.language)}
                  onChange={(event) => commit({ system: { language: event.target.value } })}
                  aria-label={t("settings.language")}
                  className={SELECT_CLASS}
                  data-testid="settings-language"
                >
                  {APP_LANGUAGES.map(({ code, label }) => <option key={code} value={code}>{label}</option>)}
                </select>
              </SettingRow>

            </>
          )}

          {tab === "appearance" && (
            <>
              <SectionTitle>{t("settings.section.appearance")}</SectionTitle>
              <SettingRow label={t("settings.appearance.theme")} desc={t("settings.appearance.themeDesc")} testId="settings-row-theme">
                <ThemePicker />
              </SettingRow>
              <SettingRow
                label={t("settings.appearance.animations")}
                desc={t("settings.appearance.animationsDesc")}
                testId="settings-row-animations"
              >
                <Toggle
                  checked={settings.appearance?.animations ?? true}
                  label={t("settings.appearance.animations")}
                  testId="settings-animations"
                  onChange={(next) => commit({ appearance: { animations: next } })}
                />
              </SettingRow>
            </>
          )}

          {tab === "gallery" && (
            <>
              <SectionTitle>{t("settings.section.gallery")}</SectionTitle>
              <SettingRow
                label={t("settings.gallery.mergeRawJpg")}
                desc={t("settings.gallery.mergeRawJpgDesc")}
                testId="settings-row-merge-raw-jpg"
              >
                <Toggle
                  checked={settings.gallery.mergeRawJpg}
                  label={t("settings.gallery.mergeRawJpg")}
                  onChange={(next) => commit({ gallery: { mergeRawJpg: next } })}
                />
              </SettingRow>
              <SectionTitle>{t("settings.section.albums")}</SectionTitle>
              <AlbumTagsSetting />
            </>
          )}

          {tab === "import" && (
            <>
              <SectionTitle>{t("settings.section.import")}</SectionTitle>
              <SettingRow label={t("settings.promptOnDevice")} desc={t("settings.promptOnDeviceDesc")}>
                <Toggle
                  checked={settings.import.promptOnDevice}
                  label={t("settings.promptOnDevice")}
                  onChange={(next) => commit({ import: { promptOnDevice: next } })}
                />
              </SettingRow>
              <SettingRow label={t("wizard.skipImported")}>
                <Toggle
                  checked={settings.import.skipImported}
                  label={t("wizard.skipImported")}
                  onChange={(next) => commit({ import: { skipImported: next } })}
                />
              </SettingRow>
              <SettingRow label={t("wizard.duplicatePolicy")}>
                <select
                  value={settings.import.duplicatePolicy}
                  onChange={(e) =>
                    commit({
                      import: {
                        duplicatePolicy: e.target.value as Settings["import"]["duplicatePolicy"],
                      },
                    })
                  }
                  aria-label={t("wizard.duplicatePolicy")}
                  className={SELECT_CLASS}
                  data-testid="settings-duplicate-policy"
                >
                  <option value="skip">{t("onboarding.scheme.dup.skip")}</option>
                  <option value="rename">{t("onboarding.scheme.dup.rename")}</option>
                  <option value="ask">{t("onboarding.scheme.dup.ask")}</option>
                </select>
              </SettingRow>
              <SettingRow label={t("settings.dualDest")} desc={t("settings.dualDestDesc")}>
                <span className="rounded bg-panel px-1.5 py-0.5 text-[10px] text-text-muted">
                  {t("settings.comingSoon")}
                </span>
              </SettingRow>
            </>
          )}

          {tab === "ai" && <AiTab />}
        </div>
      </div>
    </div>
  );
}
