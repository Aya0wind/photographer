import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useTranslation } from "react-i18next";
import { AnimatePresence, motion } from "motion/react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import NewLibraryDialog from "@/features/library/NewLibraryDialog";

/**
 * 顶部菜单栏（经典工业软件惯例）：文件 / 查看 / 工具 / 帮助。
 * - 点击菜单按钮开/关下拉；已打开时 hover 相邻菜单直接切换；
 * - 点击外部或 Escape 关闭；菜单项支持禁用态、分隔线、右对齐快捷键提示位
 *   （v1 仅展示不绑定按键，预留 Ctrl/⌘ 结构）；
 * - 「新建库…」打开可复用 NewLibraryDialog（设置页「库」选项卡共用）；
 * - 「退出」调 window.close()：后端 CloseRequested 按 system.closeToTray
 *   决定收托盘或退出，前端不做真实退出。
 */

type MenuEntry =
  | {
      kind: "item";
      key: string;
      /** 快捷键提示位（仅展示） */
      shortcut?: string;
      disabled?: boolean;
      /** 查看菜单：勾选当前页 */
      checked?: boolean;
      onSelect?: () => void;
    }
  | { kind: "separator"; key: string };

interface MenuDef {
  key: "file" | "view" | "tools" | "help";
  entries: MenuEntry[];
}

const VIEW_ITEMS: { key: string; to: string; shortcut?: string }[] = [
  { key: "gallery", to: "/gallery", shortcut: "Ctrl+1" },
  { key: "search", to: "/search", shortcut: "Ctrl+2" },
  { key: "tasks", to: "/tasks", shortcut: "Ctrl+4" },
  { key: "settings", to: "/settings", shortcut: "Ctrl+5" },
];

/** 全局导航快捷键：Ctrl+1..5 → 五个主页面（与侧栏导航一一对应；导入页无菜单项占 Ctrl+3） */
const CTRL_NAV: Record<string, string> = {
  "1": "/gallery",
  "2": "/search",
  "3": "/import",
  "4": "/tasks",
  "5": "/settings",
};

/** 勾选标记（查看菜单当前页），16 viewBox 手写 SVG */
function CheckGlyph() {
  return (
    <svg
      viewBox="0 0 16 16"
      width="10"
      height="10"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.8"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
    >
      <path d="M3.5 8.5l3 3 6-6.5" />
    </svg>
  );
}

function MenuEntryView({
  menuKey,
  entry,
  onActivate,
}: {
  menuKey: string;
  entry: MenuEntry;
  onActivate: () => void;
}) {
  const { t } = useTranslation();

  if (entry.kind === "separator") {
    return <div role="separator" className="my-1 border-t border-edge/70" />;
  }

  return (
    <button
      type="button"
      role="menuitem"
      disabled={entry.disabled}
      aria-disabled={entry.disabled}
      onClick={() => {
        if (entry.disabled) return;
        entry.onSelect?.();
        onActivate();
      }}
      className={`flex w-full items-center gap-2.5 px-3 py-1.5 text-left text-xs transition-colors ${
        entry.disabled
          ? "cursor-default text-text-muted opacity-50"
          : "text-text-secondary hover:bg-panel hover:text-text-primary"
      }`}
      data-testid={`menu-item-${entry.key}`}
    >
      {/* 勾选位（固定宽：未勾选占位对齐） */}
      <span className="flex w-3.5 shrink-0 items-center justify-center">
        {entry.checked ? <CheckGlyph /> : null}
      </span>
      <span className="flex-1 whitespace-nowrap">{t(`menu.${menuKey}.${entry.key}`)}</span>
      {entry.shortcut && (
        <span className="shrink-0 pl-4 font-mono text-[10px] text-text-muted">{entry.shortcut}</span>
      )}
    </button>
  );
}

export default function MenuBar() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const [openKey, setOpenKey] = useState<MenuDef["key"] | null>(null);
  const [newLibOpen, setNewLibOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement | null>(null);

  // 打开状态下：点击外部 / Escape 关闭
  useEffect(() => {
    if (openKey === null) return;
    function onPointerDown(e: MouseEvent): void {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setOpenKey(null);
    }
    function onKeyDown(e: KeyboardEvent): void {
      if (e.key === "Escape") setOpenKey(null);
    }
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [openKey]);

  // 全局导航快捷键（#9）：Ctrl+1..5 → 五个主页面；输入框聚焦时不劫持（数字键无
  // Ctrl 组合的输入不受影响；带 Ctrl 的系统层无冲突）
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent): void {
      if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return;
      const to = CTRL_NAV[e.key];
      if (!to) return;
      e.preventDefault();
      navigate(to);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [navigate]);

  const menus: MenuDef[] = [
    {
      key: "file",
      entries: [
        { kind: "item", key: "newLibrary", shortcut: "Ctrl+N", onSelect: () => setNewLibOpen(true) },
        {
          kind: "item",
          key: "openLibrary",
          shortcut: "Ctrl+O",
          onSelect: () => navigate("/library-picker"),
        },
        {
          kind: "item",
          key: "importPhotos",
          shortcut: "Ctrl+I",
          onSelect: () => navigate("/import"),
        },
        { kind: "separator", key: "file-sep" },
        // close() → 后端 CloseRequested → 按 system.closeToTray 收托盘/退出
        { kind: "item", key: "quit", shortcut: "Alt+F4", onSelect: () => void getCurrentWindow().close() },
      ],
    },
    {
      key: "view",
      entries: VIEW_ITEMS.map(({ key, to, shortcut }) => ({
        kind: "item" as const,
        key,
        shortcut,
        checked: location.pathname === to,
        onSelect: () => navigate(to),
      })),
    },
    {
      key: "tools",
      entries: [
        { kind: "item", key: "taskCenter", onSelect: () => navigate("/tasks") },
        { kind: "separator", key: "tools-sep" },
        // v1 先落任务中心页；日志锚点（#logs）待任务中心拆分后接入
        { kind: "item", key: "importLogs", onSelect: () => navigate("/tasks") },
      ],
    },
    {
      key: "help",
      entries: [{ kind: "item", key: "about", disabled: true }],
    },
  ];

  return (
    <div ref={rootRef} className="flex items-center" data-testid="menubar">
      {menus.map((menu) => {
        const open = openKey === menu.key;
        return (
          <div key={menu.key} className="relative">
            <button
              type="button"
              aria-haspopup="menu"
              aria-expanded={open}
              onClick={() => setOpenKey(open ? null : menu.key)}
              onMouseEnter={() => {
                // 已打开时 hover 相邻菜单直接切换（工业软件惯例）
                if (openKey !== null && openKey !== menu.key) setOpenKey(menu.key);
              }}
              className={`pointer-events-auto rounded px-2.5 py-1 text-xs transition-colors ${
                open ? "bg-panel text-text-primary" : "text-text-secondary hover:text-text-primary"
              }`}
              data-testid={`menu-button-${menu.key}`}
            >
              {t(`menu.${menu.key}`)}
            </button>
            <AnimatePresence>
              {open && (
                <motion.div
                  role="menu"
                  aria-label={t(`menu.${menu.key}`)}
                  initial={{ opacity: 0, y: -2 }}
                  animate={{ opacity: 1, y: 0 }}
                  exit={{ opacity: 0, y: -2 }}
                  transition={{ duration: 0.15, ease: "easeOut" }}
                  className="absolute left-0 top-full z-50 mt-0.5 min-w-[200px] rounded-md border border-edge bg-surface py-1 shadow-lg"
                  data-testid={`menu-panel-${menu.key}`}
                >
                  {menu.entries.map((entry) => (
                    <MenuEntryView
                      key={entry.key}
                      menuKey={menu.key}
                      entry={entry}
                      onActivate={() => setOpenKey(null)}
                    />
                  ))}
                </motion.div>
              )}
            </AnimatePresence>
          </div>
        );
      })}
      <NewLibraryDialog open={newLibOpen} onClose={() => setNewLibOpen(false)} />
    </div>
  );
}
