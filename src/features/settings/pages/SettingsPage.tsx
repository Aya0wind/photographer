import { useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { useTranslation } from "react-i18next";

import { importRootOf } from "@/features/onboarding/onboardingConfig";
import NewLibraryDialog from "@/features/library/NewLibraryDialog";
import AiTab from "@/features/settings/AiTab";
import { indexNewTags, loadSmartTags, saveSmartTags, unindexedTags } from "@/features/albums/lib/smartTags";
import { useSettingsStore, type DeepPartial, type Settings } from "@/stores/settingsStore";

/**
 * 设置页（经典工业风选项卡）：常规 / 导入 / 库 / AI 四个水平 tab
 * （下划线选中、紧凑行高）。每项行式布局——标签（左）+ 控件（右）+
 * 说明小字（下一行），分组用 uppercase 小节标题，不用卡片框。
 * 修改即存：settingsStore.update + save（IPC 失败本地仍生效）。
 * 「库」tab 保留库管理能力（只读信息 + 列表 + 激活标记 + 新建/打开入口），
 * 新建库走可复用 NewLibraryDialog（顶部菜单共用）。
 */

type SettingsTab = "general" | "appearance" | "gallery" | "import" | "libraries" | "ai";

const TABS: { key: SettingsTab; labelKey: string }[] = [
  { key: "general", labelKey: "settings.tab.general" },
  { key: "appearance", labelKey: "settings.tab.appearance" },
  { key: "gallery", labelKey: "settings.tab.gallery" },
  { key: "import", labelKey: "settings.tab.import" },
  { key: "libraries", labelKey: "settings.tab.libraries" },
  { key: "ai", labelKey: "settings.tab.ai" },
];

/** 修改即存：本地合并 + 持久化（失败静默，本地已更新） */
function commit(partial: DeepPartial<Settings>): void {
  const { update, save } = useSettingsStore.getState();
  update(partial);
  void save(useSettingsStore.getState().settings);
}

/** LR 交接指引行（B2 静态文案）：小图标 + 标题 + 说明 */
function LrGuideRow({
  icon,
  titleKey,
  textKey,
}: {
  icon: string;
  titleKey: string;
  textKey: string;
}) {
  const { t } = useTranslation();
  return (
    <div className="flex items-start gap-2.5" data-testid="lr-guide-row" data-guide={titleKey}>
      <svg
        viewBox="0 0 16 16"
        width="14"
        height="14"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.3"
        strokeLinecap="round"
        strokeLinejoin="round"
        className="mt-0.5 shrink-0 text-accent"
        aria-hidden="true"
      >
        <path d={icon} />
      </svg>
      <p className="min-w-0 text-[11px] leading-relaxed text-text-secondary">
        <span className="font-medium text-text-primary">{t(titleKey)}</span>
        <span className="mx-1.5 text-text-muted">·</span>
        {t(textKey)}
      </p>
    </div>
  );
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
      checked={checked}
      disabled={disabled}
      onChange={(e) => onChange(e.target.checked)}
      aria-label={label}
      data-testid={testId}
      className="h-3.5 w-3.5 accent-[#F0A83C] disabled:opacity-40"
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
              {tag}
              <button type="button" onClick={() => updateTags(tags.filter((item) => item !== tag))} aria-label={`${t("settings.albums.removeTag")} ${tag}`} className="ml-0.5 text-text-muted hover:text-red-400">×</button>
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

function InfoRow({ label, value, mono = true }: { label: string; value: string | null; mono?: boolean }) {
  return (
    <div className="flex items-center justify-between gap-4 py-1.5 text-left">
      <span className="shrink-0 text-xs text-text-secondary">{label}</span>
      <span
        className={`truncate text-xs ${mono ? "font-mono" : ""} ${
          value ? "text-text-primary" : "text-text-muted"
        }`}
        title={value ?? ""}
      >
        {value ?? "—"}
      </span>
    </div>
  );
}

export default function SettingsPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const settings = useSettingsStore((s) => s.settings);
  // 深链 ?tab=ai（语义门禁/未就绪引导卡的「去设置」直达；非法值回常规）
  const [searchParams] = useSearchParams();
  const urlTab = searchParams.get("tab");
  const [tab, setTab] = useState<SettingsTab>(
    TABS.some((item) => item.key === urlTab) ? (urlTab as SettingsTab) : "general",
  );
  const [newLibOpen, setNewLibOpen] = useState(false);

  const library = settings.activeLibraryId
    ? settings.libraries.find((lib) => lib.id === settings.activeLibraryId) ?? null
    : null;

  return (
    <div className="flex h-full flex-col">
      <header className="flex h-12 shrink-0 items-center border-b border-edge px-4">
        <h1 className="text-sm font-semibold text-text-primary">{t("pages.settings.title")}</h1>
      </header>

      {/* 水平选项卡行（下划线选中） */}
      <div
        role="tablist"
        aria-label={t("pages.settings.title")}
        className="flex h-9 shrink-0 items-stretch gap-1 border-b border-edge px-4"
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
              className={`border-b-2 px-3 text-xs transition-colors ${
                active
                  ? "border-accent text-accent"
                  : "border-transparent text-text-secondary hover:text-text-primary"
              }`}
              data-testid={`settings-tab-${item.key}`}
            >
              {t(item.labelKey)}
            </button>
          );
        })}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        <div className="flex w-full flex-col gap-2.5">
          {tab === "general" && (
            <>
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
                  disabled
                  value={settings.system.language}
                  aria-label={t("settings.language")}
                  className={SELECT_CLASS}
                  data-testid="settings-language"
                >
                  <option value="zh">{t("settings.languageZh")}</option>
                </select>
              </SettingRow>

              {/* LR 交接（B2）：静态指引——导出建议 / XMP 冲突 / 星级互通边界 */}
              <SectionTitle>{t("settings.section.lr")}</SectionTitle>
              <div
                className="flex flex-col gap-2 rounded-lg border border-edge/70 bg-surface/60 p-3"
                data-testid="settings-lr-guide"
              >
                <LrGuideRow icon="M2 3.5h12M2 3.5v9h12v-9M5 6.5h6M5 9h4" titleKey="lr.guide.flowTitle" textKey="lr.guide.flow" />
                <LrGuideRow icon="M8 2v8M5 4.5L8 2l3 2.5M3 8v3.5h10V8" titleKey="lr.guide.exportTitle" textKey="lr.guide.export" />
                <LrGuideRow icon="M3 4h10v8H3zM5.5 8l2 2 3.5-4" titleKey="lr.guide.xmpTitle" textKey="lr.guide.xmp" />
                <LrGuideRow icon="M8 2l1.8 3.7 4 .6-2.9 2.8.7 4L8 11.4 4.4 13.1l.7-4L2.2 6.3l4-.6z" titleKey="lr.guide.ratingTitle" textKey="lr.guide.rating" />
              </div>
            </>
          )}

          {tab === "appearance" && (
            <>
              <SectionTitle>{t("settings.section.appearance")}</SectionTitle>
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
              <SettingRow label={t("onboarding.scheme.notify")}>
                <Toggle
                  checked={settings.import.notifyMilestones}
                  label={t("onboarding.scheme.notify")}
                  onChange={(next) => commit({ import: { notifyMilestones: next } })}
                />
              </SettingRow>
              <SettingRow label={t("settings.dualDest")} desc={t("settings.dualDestDesc")}>
                <span className="rounded bg-panel px-1.5 py-0.5 text-[10px] text-text-muted">
                  {t("settings.comingSoon")}
                </span>
              </SettingRow>
            </>
          )}

          {tab === "libraries" && (
            <>
              <SectionTitle>{t("settings.section.library")}</SectionTitle>

              {/* 当前库信息（只读）：整理规则是库属性，在新建链/选择器修改 */}
              <div className="flex flex-col" data-testid="settings-current-library">
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
                    library
                      ? importRootOf(library.photoRoot, library.importSubdir || "") ||
                        library.photoRoot
                      : null
                  }
                />
                <InfoRow label={t("settings.libraries.template")} value={library?.dirTemplate ?? null} />
              </div>

              {/* 并发流数（库属性）：1-4 分段，改即存；无激活库时占位 */}
              <div className="flex items-center justify-between gap-8 py-2">
                <span className="shrink-0 text-xs text-text-secondary">{t("wizard.streams")}</span>
                {library ? (
                  <div
                    role="radiogroup"
                    aria-label={t("wizard.streams")}
                    className="flex rounded-md border border-edge bg-bg p-0.5"
                    data-testid="settings-library-streams"
                  >
                    {[1, 2, 3, 4].map((value) => {
                      const active = (library.streams ?? 4) === value;
                      return (
                        <button
                          key={value}
                          type="button"
                          role="radio"
                          aria-checked={active}
                          onClick={() => {
                            if (active) return;
                            commit({
                              libraries: settings.libraries.map((lib) =>
                                lib.id === library.id ? { ...lib, streams: value } : lib,
                              ),
                            });
                          }}
                          className={`w-8 rounded px-1 py-1 text-center font-mono text-[11px] transition-colors ${
                            active
                              ? "bg-accent text-black"
                              : "text-text-secondary hover:text-text-primary"
                          }`}
                        >
                          {value}
                        </button>
                      );
                    })}
                  </div>
                ) : (
                  <span className="text-xs text-text-muted">—</span>
                )}
              </div>

              {/* 新建库（复用对话框）+ 打开其他库（选择器） */}
              <div className="mt-3 flex gap-2">
                <button
                  type="button"
                  onClick={() => setNewLibOpen(true)}
                  className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                  data-testid="settings-new-library"
                >
                  {t("picker.newLibrary")}
                </button>
                <button
                  type="button"
                  onClick={() => navigate("/library-picker")}
                  className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                  data-testid="settings-goto-picker"
                >
                  {t("settings.libraries.goPicker")}
                </button>
              </div>

              {/* 库列表：仅展示 + 激活高亮（切换走选择器，不在原地切换） */}
              {settings.libraries.length > 0 && (
                <div className="mt-3 flex flex-col gap-1.5" data-testid="settings-library-list">
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
            </>
          )}

          {tab === "ai" && <AiTab />}
        </div>
      </div>

      <NewLibraryDialog open={newLibOpen} onClose={() => setNewLibOpen(false)} />
    </div>
  );
}
