import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

/**
 * 快捷键一次性提示条（M4.5）：首次进画廊右下角显示「按 ? 查看快捷键」，
 * localStorage 标记（一个库会话生命周期只提示一次）；× 立即关闭。
 */

export const SHORTCUTS_HINT_KEY = "photographer.shortcuts.hintSeen";

export function loadHintSeen(storage: Pick<Storage, "getItem"> = localStorage): boolean {
  try {
    return storage.getItem(SHORTCUTS_HINT_KEY) === "1";
  } catch {
    return true; // 存储不可用不骚扰
  }
}

export default function ShortcutsHint() {
  const { t } = useTranslation();
  const [visible, setVisible] = useState(() => !loadHintSeen());

  // 展示即标记（一次性：刷新/下次进入不再出现）
  useEffect(() => {
    if (!visible) return;
    try {
      localStorage.setItem(SHORTCUTS_HINT_KEY, "1");
    } catch {
      // 静默
    }
  }, [visible]);

  if (!visible) return null;

  return (
    <div
      className="fixed bottom-4 right-4 z-30 flex items-center gap-2 rounded-lg border border-edge bg-surface/95 px-3 py-2 shadow-lg backdrop-blur-sm"
      data-testid="shortcuts-hint"
      role="status"
    >
      <span className="rounded border border-edge bg-bg px-1.5 py-0.5 font-mono text-[10px] text-text-secondary">
        ?
      </span>
      <p className="text-[11px] text-text-secondary">{t("shortcuts.hint")}</p>
      <button
        type="button"
        onClick={() => setVisible(false)}
        aria-label={t("common.close")}
        className="rounded p-0.5 text-text-muted transition-colors hover:text-text-primary"
        data-testid="shortcuts-hint-close"
      >
        <svg viewBox="0 0 16 16" width="10" height="10" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
          <path d="M4 4l8 8M12 4l-8 8" />
        </svg>
      </button>
    </div>
  );
}
