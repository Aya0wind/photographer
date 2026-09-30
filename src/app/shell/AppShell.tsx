import { useEffect, useState } from "react";
import { Outlet, useLocation, useNavigate } from "react-router";
import { motion } from "motion/react";

import { motionInitial, TRANS, useMotionOn } from "@/lib/motion";

import Sidebar from "./Sidebar";
import TitleBar from "./TitleBar";
import GlobalSearchBox from "./GlobalSearchBox";
import ShortcutsModal from "./ShortcutsModal";
import DeviceDialog from "@/features/import/DeviceDialog";
import { TaskDrawerToggle, TaskDrawerPanel } from "@/features/tasks/TaskDrawer";
import SummaryModalHost from "@/features/tasks/SummaryModal";
import { useNativeBehaviorGuard } from "@/features/gallery/lib/nativeBehaviorGuard";
import { isMacPlatform } from "@/lib/platform";

/**
 * 应用主壳：整窗顶部一条自绘标题栏（TitleBar：应用标识 + 全局搜索框 +
 * 任务抽屉开关 + 窗口控制，无边框窗口拖拽/双击最大化由 Tauri drag-region 处理；
 * 顶部菜单栏已移除——导航走侧栏，Ctrl+1..5 快捷键在本壳保留），下方左侧固定
 * 侧栏 + 内容区。页面切换直接替换，不做退场/进场动画。
 * DeviceDialog 全局挂载：任何页面下设备扫描完成都会弹出导入提示。
 * 任务抽屉（M4.5 A2）全局挂载：替代右下角浮动进度卡的唯一任务入口，
 * 开关在 TitleBar 动作位（运行中任务数徽标），面板常驻轮询索引状态。
 * 界面动画关闭（settings.appearance.animations=false）时根节点挂 .no-motion。
 */

/** 全局导航快捷键（原菜单栏能力，菜单移除后保留）：Ctrl+1..5 → 主页面 */
const CTRL_NAV: Record<string, string> = {
  "1": "/gallery",
  "2": "/search",
  "3": "/import",
  "5": "/settings",
};

export default function AppShell() {
  const navigate = useNavigate();
  const [taskDrawerOpen, setTaskDrawerOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const motionOn = useMotionOn();

  // 屏蔽 WebView 原生行为：右键菜单（瓦片/图片改用自定义菜单）与开发者
  // 工具快捷键（F12 / Ctrl+Shift+I/J/C）
  useNativeBehaviorGuard();

  // Ctrl+1..5：五个主页面（与侧栏导航一一对应；导入页占 Ctrl+3）
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

  // 全局「?」键：快捷键速查弹窗（shift+/ 产生 ?；输入框内不触发）
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "?") return;
      const target = e.target;
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.isContentEditable)
      ) {
        return;
      }
      e.preventDefault();
      setShortcutsOpen((v) => !v);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  return (
    <div
      className={`flex h-full w-full flex-col overflow-hidden bg-bg font-sans text-text-primary ${
        isMacPlatform() ? "mac-window-content" : ""
      } ${
        motionOn ? "" : "no-motion"
      }`}
    >
      <TitleBar
        actions={
          <>
            <GlobalSearchBox />
            <TaskDrawerToggle open={taskDrawerOpen} onClick={() => setTaskDrawerOpen((v) => !v)} />
          </>
        }
      />
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="relative min-w-0 flex-1 overflow-hidden">
          <div className="h-full">
            <PageTransition>
              <Outlet />
            </PageTransition>
          </div>
        </main>
      </div>
      <DeviceDialog />
      <TaskDrawerPanel open={taskDrawerOpen} onClose={() => setTaskDrawerOpen(false)} />
      <ShortcutsModal open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />
      {/* 导入完成总结弹窗（全局：任何页面弹出；此前在任务页） */}
      <SummaryModalHost />
    </div>
  );
}

/** 子页切换过渡（动画批次 #4）：AnimatePresence mode="wait" + 短淡入。
 *  退场 150ms 内完成（无感延迟）；动画关（useMotionOn=false）直通渲染。 */
function PageTransition({ children }: { children: React.ReactNode }) {
  const motionOn = useMotionOn();
  const location = useLocation();
  if (!motionOn) return <>{children}</>;
  // 仅入场动画、无退场：Outlet 是活组件，退场层会渲染新路由内容（双层同
  // 内容），且 wait/popLayout 都有空窗或克隆问题——入场 y 位移不透明度不
  // 归零，首帧即有内容（无闪烁），切页仍有轻量动效。
  return (
    <motion.div
      key={location.pathname}
      className="h-full"
      initial={motionInitial(motionOn, { y: 8 })}
      animate={{ y: 0 }}
      transition={TRANS.slide}
    >
      {children}
    </motion.div>
  );
}
