import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";

import { motionInitial, useMotionOn } from "@/lib/motion";

/**
 * 快捷键速查弹窗（M4.5）：全局「?」键开关，Esc 关闭。分组列出既有绑定：
 * 全局 / 画廊 / 查看器（静态清单——与各组件实际绑定保持同步）。
 */

interface ShortcutGroup {
  titleKey: string;
  items: ReadonlyArray<{ keys: string; labelKey: string }>;
}

const GROUPS: ReadonlyArray<ShortcutGroup> = [
  {
    titleKey: "shortcuts.group.global",
    items: [
      { keys: "?", labelKey: "shortcuts.global.help" },
      { keys: "Esc", labelKey: "shortcuts.global.esc" },
    ],
  },
  {
    titleKey: "shortcuts.group.gallery",
    items: [
      { keys: "← → ↑ ↓", labelKey: "shortcuts.gallery.move" },
      { keys: "Enter", labelKey: "shortcuts.gallery.open" },
      { keys: "shortcuts.selectKeys", labelKey: "shortcuts.gallery.select" },
      { keys: "Esc", labelKey: "shortcuts.gallery.exitSelect" },
    ],
  },
  {
    titleKey: "shortcuts.group.viewer",
    items: [
      { keys: "← →", labelKey: "shortcuts.viewer.nav" },
      { keys: "shortcuts.zoomKeys", labelKey: "shortcuts.viewer.zoom" },
      { keys: "[ ] / , . / R", labelKey: "shortcuts.viewer.rotate" },
      { keys: "1–5 / 0", labelKey: "shortcuts.viewer.rating" },
      { keys: "P / U", labelKey: "shortcuts.viewer.flag" },
      { keys: "I", labelKey: "shortcuts.viewer.info" },
      { keys: "Home / End", labelKey: "shortcuts.viewer.homeEnd" },
      { keys: "Esc", labelKey: "shortcuts.viewer.close" },
    ],
  },
];

export default function ShortcutsModal({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const motionOn = useMotionOn();

  // Esc 关闭（弹窗自身；不吞其他层——仅在 open 时监听）
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("keydown", onKey, true); // 捕获阶段先行，避免查看器同时响应
    return () => window.removeEventListener("keydown", onKey, true);
  }, [open, onClose]);

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          key="backdrop"
          initial={motionInitial(motionOn, { opacity: 0 })}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: 0.15 }}
          onClick={onClose}
          className="fixed inset-0 z-[60] flex items-center justify-center bg-black/50"
          data-testid="shortcuts-modal-backdrop"
        >
          <motion.div
            key="panel"
            initial={motionInitial(motionOn, { opacity: 0, y: 12 })}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ duration: 0.15, ease: "easeOut" }}
            className="max-h-[76vh] w-[420px] overflow-y-auto rounded-xl border border-edge bg-surface p-5 shadow-2xl"
            role="dialog"
            aria-modal="true"
            aria-label={t("shortcuts.title")}
            data-testid="shortcuts-modal"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center justify-between">
              <h2 className="text-sm font-semibold text-text-primary">{t("shortcuts.title")}</h2>
              <button
                type="button"
                onClick={onClose}
                aria-label={t("common.close")}
                data-testid="shortcuts-close"
                className="rounded-md p-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
              >
                <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                  <path d="M4 4l8 8M12 4l-8 8" />
                </svg>
              </button>
            </div>

            <div className="mt-4 space-y-4">
              {GROUPS.map((group) => (
                <section key={group.titleKey} data-testid="shortcuts-group">
                  <h3 className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                    {t(group.titleKey)}
                  </h3>
                  <dl className="space-y-1">
                    {group.items.map((item) => (
                      <div
                        key={item.keys}
                        className="flex items-center justify-between gap-4 text-xs"
                      >
                        <dt className="shrink-0 rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[10px] text-text-secondary">
                          {item.keys.startsWith("shortcuts.") ? t(item.keys) : item.keys}
                        </dt>
                        <dd className="min-w-0 text-right text-text-secondary">
                          {t(item.labelKey)}
                        </dd>
                      </div>
                    ))}
                  </dl>
                </section>
              ))}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
