import { Outlet, useLocation } from "react-router";
import { AnimatePresence, motion } from "motion/react";

import MenuBar from "./MenuBar";
import Sidebar from "./Sidebar";
import TitleBar from "./TitleBar";
import DeviceDialog from "@/features/import/DeviceDialog";
import ImportProgressCard from "@/features/import/ImportProgressCard";

/**
 * 应用主壳：整窗顶部一条自绘标题栏（TitleBar：应用标识 + 菜单栏 + 窗口控制，
 * 无边框窗口拖拽/双击最大化由 Tauri drag-region 处理），下方左侧固定侧栏
 * （品牌区让位顶部条）+ 内容区。页面切换时内容区做 opacity + 4px 位移过渡
 * （180ms ease-out）。DeviceDialog 全局挂载：任何页面下设备扫描完成都会弹出
 * 导入提示。ImportProgressCard 全局挂载：LR 式后台导入进度常驻右下角，不打断浏览。
 */
export default function AppShell() {
  const location = useLocation();

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-bg font-sans text-text-primary">
      <TitleBar>
        <MenuBar />
      </TitleBar>
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="relative min-w-0 flex-1 overflow-y-auto">
          <AnimatePresence mode="wait" initial={false}>
            <motion.div
              key={location.pathname}
              initial={{ opacity: 0, y: 4 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, y: -4 }}
              transition={{ duration: 0.18, ease: "easeOut" }}
              className="h-full"
            >
              <Outlet />
            </motion.div>
          </AnimatePresence>
        </main>
      </div>
      <DeviceDialog />
      <ImportProgressCard />
    </div>
  );
}
