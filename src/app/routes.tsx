import {
  createBrowserRouter,
  Navigate,
  RouterProvider,
  useSearchParams,
} from "react-router";

import { lazy, Suspense, useEffect, useRef, useState } from "react";
import AppShell from "./shell/AppShell";
import GalleryPage from "@/features/gallery/pages/GalleryPage";
import RecentPage from "@/features/gallery/pages/RecentPage";
import TrashPage from "@/features/gallery/pages/TrashPage";
import MemoriesPage from "@/features/memories/pages/MemoriesPage";
import GearPage from "@/features/gear/pages/GearPage";
import MapPage from "@/features/map/pages/MapPage";
import MapPhotosPage from "@/features/map/pages/MapPhotosPage";
import ImportPage from "@/features/import/pages/ImportPage";
import SimilarPage from "@/features/similar/pages/SimilarPage";
import StoragePage from "@/features/storage/pages/StoragePage";
import SplashPage from "@/app/SplashPage";
import { useWindowReveal } from "@/lib/windowReveal";
import OnboardingPage from "@/features/onboarding/pages/OnboardingPage";
import DatabasePickerPage from "@/features/database/pages/DatabasePickerPage";
import { databaseSwitch } from "@/ipc/api";
import { resetLibrarySession } from "@/lib/librarySession";
import { useDatabases } from "@/lib/useDatabases";
import PeoplePage from "@/features/people/pages/PeoplePage";
import CullingPage from "@/features/culling/pages/CullingPage";
import { AlbumsIndexPage, AlbumEntryPage } from "@/features/albums/pages/AlbumsPages";
import SettingsPage from "@/features/settings/pages/SettingsPage";
import TetheringWindowPage from "@/features/tethering/TetheringWindowPage";
import { useSettingsStore } from "@/stores/settingsStore";

const AdvancedEditorWindowPage = lazy(() => import("@/features/editor/components/AdvancedEditorWindowPage"));

/** Validate the remembered database once on startup, otherwise show the picker.
 * Manual return to the picker never repeats startup selection. */
export function GatedShell() {
  const loaded = useSettingsStore((s) => s.loaded);
  const databaseChosen = useSettingsStore((s) => s.databaseChosen);
  const databases = useDatabases();
  useWindowReveal(loaded);
  const preferredId=useSettingsStore(s=>s.settings.system.autoOpenDatabaseId);
  const attempted=useRef(false);
  const [autoOpening,setAutoOpening]=useState(false);
  const [autoOpened,setAutoOpened]=useState(false);
  const [startupError,setStartupError]=useState<string|null>(null);
  const preferredExists=!!preferredId && !!databases?.databases.some(db=>db.id===preferredId);
  const shouldAutoOpen=loaded && databases!==null && !databaseChosen && preferredExists && !attempted.current;
  useEffect(()=>{
    if(databaseChosen) {attempted.current=true;return;}
    if(!shouldAutoOpen || !preferredId) return;
    attempted.current=true;setAutoOpening(true);
    if(databases?.activeId!==preferredId) resetLibrarySession();
    void databaseSwitch(preferredId).then(()=>{
      setAutoOpened(true);
      useSettingsStore.getState().setDatabaseChosen(true);
    }).catch(error=>setStartupError(error instanceof Error ? error.message : String(error)))
      .finally(()=>setAutoOpening(false));
  },[databaseChosen,shouldAutoOpen,preferredId,databases]);

  if (shouldAutoOpen || autoOpening) return null;
  if (!loaded || databases === null) return null;
  const hasActiveDatabase = autoOpened ||
    databases.activeId !== null &&
    databases.databases.some((db) => db.id === databases.activeId);
  if (!databaseChosen || !hasActiveDatabase) {
    return <Navigate to="/database-picker" replace state={startupError ? {autoOpenError:startupError} : undefined} />;
  }
  return <AppShell />;
}

/** /search → /gallery 重定向（M4.5 画廊+搜索合并；参数原样透传） */
export function SearchRedirect() {
  const [params] = useSearchParams();
  const query = params.toString();
  return <Navigate to={query ? `/gallery?${query}` : "/gallery"} replace />;
}

export const router = createBrowserRouter([
  // 启动画面（splash 窗口加载；主窗口就绪后被关闭——见 lib/windowReveal）
  { path: "/splash", element: <SplashPage /> },
  // 启动首屏：数据库选择器（达芬奇式，每次启动先选数据库）
  { path: "/database-picker", element: <DatabasePickerPage /> },
  // 新建数据库配置链（/onboarding）：独立于主壳全屏展示
  { path: "/onboarding", element: <OnboardingPage /> },
  // 联机拍摄独立窗口（后端 tethering_start 创建的第二 webview 加载；独立于
  // 主壳守卫——该窗口的 store 是全新会话态，会话真值全部来自后端命令）
  { path: "/advanced-editor", element: <Suspense fallback={null}><AdvancedEditorWindowPage /></Suspense> },
  { path: "/tethering", element: <TetheringWindowPage /> },
  {
    path: "/",
    element: <GatedShell />,
    children: [
      { index: true, element: <Navigate to="/gallery" replace /> },
      { path: "gallery", element: <GalleryPage /> },
      { path: "recent", element: <RecentPage /> },
      // 回收站（B1：软删资产管理——恢复/彻底删除）
      { path: "trash", element: <TrashPage /> },
      { path: "memories", element: <MemoriesPage /> },
      { path: "gear", element: <GearPage /> },
      // 拍摄地图（嵌套子路由 photos：气泡角标/样图点击进入 ?region=<id>）——
      // MapPage 渲染 <Outlet/>，子页以不透明层覆盖、地图保持挂载不卸载
      {
        path: "map",
        element: <MapPage />,
        children: [{ path: "photos", element: <MapPhotosPage /> }],
      },
      { path: "search", element: <SearchRedirect /> },
      { path: "similar", element: <SimilarPage /> },
      // 选片会话（Culling V1）：会话列表 + 全屏过片层（浮层内挂载）
      { path: "culling", element: <CullingPage /> },
      { path: "people", element: <PeoplePage /> },
      { path: "albums", element: <AlbumsIndexPage /> },
      // 手工相册详情（/albums/:id，纯数字参数）/ 智能标签语义结果共用一个槽位
      { path: "albums/:tag", element: <AlbumEntryPage /> },
      { path: "import", element: <ImportPage /> },
      // 存储（M3）：照片库登记管理——新建/从文件夹建立/移除登记
      { path: "storage", element: <StoragePage /> },
      { path: "settings", element: <SettingsPage /> },
    ],
  },
  // 未知路径回落画廊（仍受主壳守卫保护，未选数据库会被送去选择器）
  { path: "*", element: <Navigate to="/gallery" replace /> },
]);

export function AppRoutes() {
  return <RouterProvider router={router} />;
}
