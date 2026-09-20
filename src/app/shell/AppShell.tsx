import { useState } from "react";
import { Outlet } from "react-router";

import MenuBar from "./MenuBar";
import Sidebar from "./Sidebar";
import TitleBar from "./TitleBar";
import GlobalSearchBox from "./GlobalSearchBox";
import DeviceDialog from "@/features/import/DeviceDialog";
import { TaskDrawerToggle, TaskDrawerPanel } from "@/features/tasks/TaskDrawer";

/**
 * 应用主壳：整窗顶部一条自绘标题栏（TitleBar：应用标识 + 菜单栏 + 全局搜索框 +
 * 任务抽屉开关 + 窗口控制，无边框窗口拖拽/双击最大化由 Tauri drag-region 处理），
 * 下方左侧固定侧栏 + 内容区。页面切换直接替换，不做退场/进场动画。
 * DeviceDialog 全局挂载：任何页面下设备扫描完成都会弹出导入提示。
 * 任务抽屉（M4.5 A2）全局挂载：替代右下角浮动进度卡的唯一任务入口，
 * 开关在 TitleBar 动作位（运行中任务数徽标），面板常驻轮询索引状态。
 */
export default function AppShell() {
  const [taskDrawerOpen, setTaskDrawerOpen] = useState(false);

  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-bg font-sans text-text-primary">
      <TitleBar
        actions={
          <>
            <GlobalSearchBox />
            <TaskDrawerToggle open={taskDrawerOpen} onClick={() => setTaskDrawerOpen((v) => !v)} />
          </>
        }
      >
        <MenuBar />
      </TitleBar>
      <div className="flex min-h-0 flex-1">
        <Sidebar />
        <main className="relative min-w-0 flex-1 overflow-y-auto">
          <div className="h-full">
            <Outlet />
          </div>
        </main>
      </div>
      <DeviceDialog />
      <TaskDrawerPanel open={taskDrawerOpen} onClose={() => setTaskDrawerOpen(false)} />
    </div>
  );
}
