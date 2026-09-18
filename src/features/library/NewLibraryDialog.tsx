import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import {
  DEFAULT_IMPORT_SUBDIR,
  SUGGESTED_DB_DIR,
  SUGGESTED_LIBRARY_NAME,
  SUGGESTED_PHOTO_ROOT,
} from "@/features/onboarding/onboardingConfig";
import { useSettingsStore, type Library } from "@/stores/settingsStore";

/**
 * 新建库对话框（可复用）：顶部菜单「文件 → 新建库…」与设置页「库」选项卡共用。
 * 快速录入库位置四要素（名称/数据库目录/照片存储目录/导入子目录，本机默认值预填），
 * 创建 configured=false 的库并激活，随即进入 /onboarding?library=<id> 补完
 * 整理规则与 AI 设置（与库选择器「打开未配置库」同一条补完链）。
 */

interface NewLibraryDialogProps {
  open: boolean;
  onClose: () => void;
}

function makeLibraryId(): string {
  return typeof crypto !== "undefined" && "randomUUID" in crypto
    ? crypto.randomUUID()
    : `lib-${Date.now().toString(36)}`;
}

interface Draft {
  name: string;
  dbDir: string;
  photoRoot: string;
  importSubdir: string;
}

function defaultDraft(): Draft {
  return {
    name: SUGGESTED_LIBRARY_NAME,
    dbDir: SUGGESTED_DB_DIR,
    photoRoot: SUGGESTED_PHOTO_ROOT,
    importSubdir: DEFAULT_IMPORT_SUBDIR,
  };
}

const FIELD_CLASS =
  "w-full rounded-md border border-edge bg-bg px-2.5 py-1.5 text-xs text-text-primary outline-none transition-colors focus:border-accent";

export default function NewLibraryDialog({ open, onClose }: NewLibraryDialogProps) {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [draft, setDraft] = useState<Draft>(defaultDraft);
  const [creating, setCreating] = useState(false);

  // 每次打开重置为本机默认值草稿
  useEffect(() => {
    if (open) {
      setDraft(defaultDraft());
      setCreating(false);
    }
  }, [open]);

  const canCreate =
    !creating &&
    draft.name.trim().length > 0 &&
    draft.dbDir.trim().length > 0 &&
    draft.photoRoot.trim().length > 0;

  function patch(p: Partial<Draft>): void {
    setDraft((d) => ({ ...d, ...p }));
  }

  async function create(): Promise<void> {
    if (!canCreate) return;
    setCreating(true);
    const library: Library = {
      id: makeLibraryId(),
      name: draft.name.trim(),
      dbDir: draft.dbDir.trim(),
      photoRoot: draft.photoRoot.trim(),
      dirTemplate: "{YYYY}/{MM-DD}/{原文件名}",
      importSubdir: draft.importSubdir.trim(),
      configured: false,
    };
    const current = useSettingsStore.getState().settings;
    await useSettingsStore.getState().save({
      ...current,
      libraries: [...current.libraries, library],
      activeLibraryId: library.id,
    });
    useSettingsStore.getState().setLibraryChosen(true);
    onClose();
    navigate(`/onboarding?library=${encodeURIComponent(library.id)}`);
  }

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15, ease: "easeOut" }}
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
          role="dialog"
          aria-modal="true"
          aria-label={t("newLib.title")}
        >
          <motion.div
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="w-[440px] rounded-xl border border-edge bg-surface p-5 shadow-2xl"
            data-testid="new-library-dialog"
          >
            <h2 className="text-sm font-semibold text-text-primary">{t("newLib.title")}</h2>

            <div className="mt-4 flex flex-col gap-3">
              <label className="flex flex-col gap-1 text-xs text-text-secondary">
                {t("onboarding.library.name")}
                <input
                  type="text"
                  value={draft.name}
                  onChange={(e) => patch({ name: e.target.value })}
                  className={FIELD_CLASS}
                  autoFocus
                />
              </label>
              <label className="flex flex-col gap-1 text-xs text-text-secondary">
                {t("onboarding.library.photoRoot")}
                <input
                  type="text"
                  value={draft.photoRoot}
                  onChange={(e) => patch({ photoRoot: e.target.value })}
                  className={FIELD_CLASS}
                />
              </label>
              <label className="flex flex-col gap-1 text-xs text-text-secondary">
                {t("onboarding.library.importSubdir")}
                <input
                  type="text"
                  value={draft.importSubdir}
                  onChange={(e) => patch({ importSubdir: e.target.value })}
                  className={FIELD_CLASS}
                />
              </label>
              <label className="flex flex-col gap-1 text-xs text-text-secondary">
                {t("onboarding.library.dbDir")}
                <input
                  type="text"
                  value={draft.dbDir}
                  onChange={(e) => patch({ dbDir: e.target.value })}
                  className={FIELD_CLASS}
                />
              </label>
            </div>

            <p className="mt-3 text-[11px] leading-relaxed text-text-muted">{t("newLib.hint")}</p>

            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
                data-testid="new-library-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                disabled={!canCreate}
                onClick={() => void create()}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="new-library-submit"
              >
                {t("newLib.create")}
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
