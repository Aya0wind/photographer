import { useEffect, useState, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * 自绘标题栏（无边框窗口，tauri.conf decorations=false）：整窗最顶部 40px 条
 * （surface 底 + 底边 edge），左侧应用标识，右侧动作位（全局搜索框/任务抽屉
 * 开关）与窗口控制三钮（最小化 / 最大化↔还原 / 关闭）。顶部菜单栏已按用户
 * 要求移除——原菜单功能入口全部有替代：导航走侧栏、新建/打开库走 设置→库
 * 与库选择器、退出走窗口关闭钮（Ctrl+1..5 导航快捷键保留在 AppShell）。
 *
 * - 拖拽：背景层带 data-tauri-drag-region；Tauri 注入脚本（window/scripts/drag.js）
 *   处理拖动与双击最大化（internal_toggle_maximize）——前端不再绑 onDoubleClick，
 *   否则会双次 toggle 相互抵消。前景容器 pointer-events-none、按钮 pointer-events-auto，
 *   空白区域点击自然落到拖拽背景层；脚本对 BUTTON 等可点击元素自动豁免拖拽。
 * - 最大化状态：mount 时 + tauri://resize（onResized）后查询 isMaximized 切图标。
 * - close() 走后端 CloseRequested → 按 system.closeToTray 决定托盘/退出，前端不处理。
 * - dev 预览（无 Tauri）下所有窗口 API 调用 catch 静默。
 */

/** 16 viewBox 线条图标（stroke 风格与侧栏一致），按钮内 10px 渲染 */
function GlyphMinimize() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="10"
      height="10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M3 8h10" />
    </svg>
  );
}

function GlyphMaximize() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="10"
      height="10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <rect x="3.5" y="3.5" width="9" height="9" rx="1" />
    </svg>
  );
}

function GlyphRestore() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="10"
      height="10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M5.5 3.5h6a1 1 0 0 1 1 1v6" />
      <rect x="3.5" y="5.5" width="7" height="7" rx="1" />
    </svg>
  );
}

function GlyphClose() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="10"
      height="10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.4"
      strokeLinecap="round"
      aria-hidden="true"
    >
      <path d="M4 4l8 8M12 4l-8 8" />
    </svg>
  );
}

interface TitleBarButtonProps {
  label: string;
  onClick: () => void;
  testId: string;
  /** 关闭钮 hover 红底白字（Windows 惯例） */
  danger?: boolean;
  children: ReactNode;
}

function TitleBarButton({ label, onClick, testId, danger = false, children }: TitleBarButtonProps) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      data-testid={testId}
      className={`pointer-events-auto flex h-full w-[44px] items-center justify-center transition-colors ${
        danger
          ? "text-text-secondary hover:bg-[#C42B1C] hover:text-white"
          : "text-text-secondary hover:bg-panel hover:text-text-primary"
      }`}
    >
      {children}
    </button>
  );
}

export default function TitleBar({
  actions,
}: {
  /** 右侧动作位（主壳传全局搜索框/任务抽屉开关等；渲染在弹性空区与窗口控制钮之间） */
  actions?: ReactNode;
}) {
  const { t } = useTranslation();
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    let disposed = false;
    let unlisten: (() => void) | null = null;
    const appWindow = getCurrentWindow();
    void appWindow
      .isMaximized()
      .then((value) => {
        if (!disposed) setMaximized(value);
      })
      .catch(() => {
        // dev 预览/测试环境无窗口：静默
      });
    void appWindow
      .onResized(async () => {
        try {
          setMaximized(await appWindow.isMaximized());
        } catch {
          // 忽略单次查询失败
        }
      })
      .then((unlistenFn) => {
        if (disposed) unlistenFn();
        else unlisten = unlistenFn;
      })
      .catch(() => {
        // 事件订阅失败静默（非 Tauri 环境）
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  return (
    <header
      className="relative flex h-10 shrink-0 select-none items-stretch border-b border-edge bg-surface"
      data-testid="titlebar"
    >
      {/* 拖拽背景层：整条可拖动窗口；双击最大化由 Tauri 注入脚本处理 */}
      <div className="absolute inset-0" data-tauri-drag-region data-testid="titlebar-drag-region" />

      {/* 应用标识（pointer-events-none：点击落到拖拽层） */}
      <div className="pointer-events-none relative flex items-center gap-2.5 pl-4">
        <span className="h-2.5 w-2.5 rounded-full bg-accent" aria-hidden="true" />
        <span className="text-[13px] font-semibold tracking-wide text-text-primary">Photo Hub</span>
      </div>

      {/* 弹性空区：透传拖拽 */}
      <div className="pointer-events-none relative flex-1" />

      {/* 右侧动作位（全局搜索框 / 任务抽屉开关；控件自身 pointer-events-auto） */}
      <div className="pointer-events-none relative flex items-center gap-1.5 pr-1">
        {actions}
      </div>

      {/* 窗口控制三钮 */}
      <div className="pointer-events-none relative flex items-stretch">
        <TitleBarButton
          label={t("titlebar.minimize")}
          testId="titlebar-minimize"
          onClick={() => void getCurrentWindow().minimize()}
        >
          <GlyphMinimize />
        </TitleBarButton>
        <TitleBarButton
          label={maximized ? t("titlebar.restore") : t("titlebar.maximize")}
          testId="titlebar-maximize"
          onClick={() => void getCurrentWindow().toggleMaximize()}
        >
          {maximized ? <GlyphRestore /> : <GlyphMaximize />}
        </TitleBarButton>
        <TitleBarButton
          label={t("titlebar.close")}
          testId="titlebar-close"
          danger
          onClick={() => void getCurrentWindow().close()}
        >
          <GlyphClose />
        </TitleBarButton>
      </div>
    </header>
  );
}
