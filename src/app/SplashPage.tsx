/** 启动画面（splash 窗口加载，与主壳无关的最小组件树——毫秒级首帧）：
 *  主窗口 visible=false 启动，React 就绪后由 useWindowReveal 显示主窗并
 *  关闭本窗（2026-09-29：消除首启白屏数秒的「软件坏了」观感）。 */
import { useTranslation } from "react-i18next";

export default function SplashPage() {
  const { t } = useTranslation();
  return (
    <div className="flex h-screen flex-col items-center justify-center gap-4 bg-bg text-text-primary" data-testid="splash">
      <div className="flex h-14 w-14 animate-pulse items-center justify-center rounded-2xl border border-edge bg-surface">
        <svg viewBox="0 0 24 24" width="28" height="28" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" className="text-accent" aria-hidden="true">
          <rect x="3" y="5" width="18" height="14" rx="2" />
          <circle cx="9" cy="10" r="1.6" />
          <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
        </svg>
      </div>
      <div className="text-sm font-semibold tracking-wide">Photographer</div>
      <div className="animate-pulse text-[11px] text-text-muted">{t("splash.loading")}</div>
    </div>
  );
}
