import { Outlet } from "react-router";

import MenuBar from "./MenuBar";
import Sidebar from "./Sidebar";
import TitleBar from "./TitleBar";
import DeviceDialog from "@/features/import/DeviceDialog";
import ImportProgressCard from "@/features/import/ImportProgressCard";

/**
 * 应用主壳：整窗顶部一条自绘标题栏（TitleBar：应用标识 + 菜单栏 + 窗口控制，
 * 无边框窗口拖拽/双击最大化由 Tauri drag-region 处理），下方左侧固定侧栏
 * （品牌区让位顶部条）+ 内容区。页面切换直接替换，不做退场/进场动画。
 * DeviceDialog 全局挂载：任何页面下设备扫描完成都会弹出
 * 导入提示。ImportProgressCard 全局挂载：LR 式后台导入进度常驻右下角，不打断浏览。
 */
export default function AppShell() {
  return (
    <div className="flex h-full w-full flex-col overflow-hidden bg-bg font-sans text-text-primary">
      <TitleBar>
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
      <ImportProgressCard />
    </div>
  );
}
