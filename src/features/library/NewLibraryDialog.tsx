import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { suggestedLibraryName } from "@/features/onboarding/onboardingConfig";
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

/** sessionStorage 标记：本次会话由对话框新建、尚未配置完成的库 id。
 *  向导（/onboarding）取消时据此判定「可删除的空库」；配置完成或取消后清除。 */
export const NEW_LIBRARY_DRAFT_KEY = "smartphoto.import.newLibDraft";

export function readDraftLibraryId(): string | null {
  try {
    return sessionStorage.getItem(NEW_LIBRARY_DRAFT_KEY);
  } catch {
    return null;
  }
}

export function clearDraftLibraryId(): void {
  try {
    sessionStorage.removeItem(NEW_LIBRARY_DRAFT_KEY);
  } catch {
    // 存储不可用时静默
  }
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
  streams: number;
}

function defaultDraft(): Draft {
  return {
    name: suggestedLibraryName(),
    dbDir: "",
    photoRoot: "",
    streams: 4,
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

  // 库名全局唯一（用户定案）：与现有库重名时禁建并提示
  // selector 只取 libraries（引用稳定——.map 进 selector 会每渲染新数组，
  // zustand 快照失稳导致订阅组件死循环，真机设置页 40 测试崩）
  const libraries = useSettingsStore((s) => s.settings.libraries);
  const nameTaken = libraries.some(
    (l) => l.name.trim() === draft.name.trim() && draft.name.trim() !== "",
  );

  const canCreate =
    !creating &&
    draft.name.trim().length > 0 &&
    !nameTaken &&
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
      streams: draft.streams,
      configured: false,
    };
    const current = useSettingsStore.getState().settings;
    await useSettingsStore.getState().save({
      ...current,
      libraries: [...current.libraries, library],
      activeLibraryId: library.id,
    });
    useSettingsStore.getState().setLibraryChosen(true);
    // 记「本次会话新建」标记：向导取消时可安全删除这个未配置空库
    try {
      sessionStorage.setItem(NEW_LIBRARY_DRAFT_KEY, library.id);
    } catch {
      // 存储不可用时静默（取消退化为不删库，仅退出）
    }
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
                {nameTaken && (
                  <span className="text-[11px] text-red-400" role="alert">
                    {t("newLib.nameTaken")}
                  </span>
                )}
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
                {t("onboarding.library.dbDir")}
                <input
                  type="text"
                  value={draft.dbDir}
                  onChange={(e) => patch({ dbDir: e.target.value })}
                  className={FIELD_CLASS}
                />
              </label>
              <div className="flex flex-col gap-1 text-xs text-text-secondary">
                <span className="flex items-center gap-2">
                  {t("wizard.streams")}
                  <span className="text-[11px] text-text-muted">{t("newLib.streamsDesc")}</span>
                </span>
                <div
                  role="radiogroup"
                  aria-label={t("wizard.streams")}
                  className="flex w-fit rounded-md border border-edge bg-bg p-0.5"
                  data-testid="new-library-streams"
                >
                  {[1, 2, 3, 4].map((value) => (
                    <button
                      key={value}
                      type="button"
                      role="radio"
                      aria-checked={draft.streams === value}
                      onClick={() => patch({ streams: value })}
                      className={`w-10 rounded px-1 py-1.5 text-center font-mono text-[11px] transition-colors ${
                        draft.streams === value
                          ? "bg-accent text-black"
                          : "text-text-secondary hover:text-text-primary"
                      }`}
                    >
                      {value}
                    </button>
                  ))}
                </div>
              </div>
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
