import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";

import type { OnboardingDraft } from "../types";

interface Props {
  draft: OnboardingDraft;
  onChange: (patch: Partial<OnboardingDraft>) => void;
}

async function pickDirectory(current: string, onPicked: (dir: string) => void) {
  try {
    const dir = await openDialog({ directory: true, defaultPath: current || undefined });
    if (typeof dir === "string" && dir.length > 0) onPicked(dir);
  } catch {
    // 非 Tauri 环境（vite 预览）或用户取消：保持现状
  }
}

function Label({ htmlFor, labelKey }: { htmlFor: string; labelKey: string }) {
  const { t } = useTranslation();
  return (
    <label htmlFor={htmlFor} className="text-sm font-medium text-text-primary">
      {t(labelKey)}
    </label>
  );
}

function Desc({ descKey }: { descKey: string }) {
  const { t } = useTranslation();
  return <p className="text-xs leading-relaxed text-text-muted">{t(descKey)}</p>;
}

const inputClass =
  "w-full rounded-md border border-edge bg-bg px-3 py-2 font-mono text-xs text-text-primary outline-none transition-colors focus:border-accent";

/** 步骤 1：库设置——库名 + 数据库目录（自包含）+ 照片存储目录（达芬奇式库模型） */
export default function LibraryStep({ draft, onChange }: Props) {
  const { t } = useTranslation();

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-col gap-1.5">
        <Label htmlFor="onboarding.library.name" labelKey="onboarding.library.name" />
        <input
          id="onboarding.library.name"
          type="text"
          value={draft.libraryName}
          onChange={(e) => onChange({ libraryName: e.target.value })}
          className={inputClass}
        />
        <Desc descKey="onboarding.library.nameDesc" />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="onboarding.library.dbDir" labelKey="onboarding.library.dbDir" />
        <div className="flex gap-2">
          <input
            id="onboarding.library.dbDir"
            type="text"
            value={draft.dbDir}
            onChange={(e) => onChange({ dbDir: e.target.value })}
            className={inputClass}
          />
          <button
            type="button"
            onClick={() => pickDirectory(draft.dbDir, (dir) => onChange({ dbDir: dir }))}
            className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          >
            {t("onboarding.library.browse")}
          </button>
        </div>
        <Desc descKey="onboarding.library.dbDirDesc" />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="onboarding.library.photoRoot" labelKey="onboarding.library.photoRoot" />
        <div className="flex gap-2">
          <input
            id="onboarding.library.photoRoot"
            type="text"
            value={draft.photoRoot}
            onChange={(e) => onChange({ photoRoot: e.target.value })}
            className={inputClass}
          />
          <button
            type="button"
            onClick={() => pickDirectory(draft.photoRoot, (dir) => onChange({ photoRoot: dir }))}
            className="shrink-0 rounded-md border border-edge px-3 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          >
            {t("onboarding.library.browse")}
          </button>
        </div>
        <Desc descKey="onboarding.library.photoRootDesc" />
      </div>

    </div>
  );
}
