import {
  createBrowserRouter,
  Navigate,
  RouterProvider,
  useSearchParams,
} from "react-router";

import { lazy, Suspense } from "react";
import AppShell from "./shell/AppShell";
import GalleryPage from "@/features/gallery/pages/GalleryPage";
import RecentPage from "@/features/gallery/pages/RecentPage";
import TrashPage from "@/features/gallery/pages/TrashPage";
import MemoriesPage from "@/features/memories/pages/MemoriesPage";
import GearPage from "@/features/gear/pages/GearPage";
import MapPage from "@/features/map/pages/MapPage";
import ImportPage from "@/features/import/pages/ImportPage";
import SimilarPage from "@/features/similar/pages/SimilarPage";
import StoragePage from "@/features/storage/pages/StoragePage";
import SplashPage from "@/app/SplashPage";
import { useWindowReveal } from "@/lib/windowReveal";
import OnboardingPage from "@/features/onboarding/pages/OnboardingPage";
import { usePhotoLibraries } from "@/features/onboarding/lib/usePhotoLibraries";
import PeoplePage from "@/features/people/pages/PeoplePage";
import CullingPage from "@/features/culling/pages/CullingPage";
import { AlbumsIndexPage, AlbumEntryPage } from "@/features/albums/pages/AlbumsPages";
import SettingsPage from "@/features/settings/pages/SettingsPage";
import TetheringWindowPage from "@/features/tethering/TetheringWindowPage";
import { useSettingsStore } from "@/stores/settingsStore";

const AdvancedEditorWindowPage = lazy(() => import("@/features/editor/components/AdvancedEditorWindowPage"));

/**
 * 主壳守卫（2026-10-09 单库多照片库定案，M5 收尾）：设置未加载完成时空白等待；
 * 加载后**尚无任何照片库且未完成引导** → 送 /onboarding（数据库就位 → 引导
 * 建立第一个照片库），替代旧达芬奇式「启动先选库」闸。已有照片库（或引导已
 * 完成）直进主壳——多照片库并存、画廊全局跨库混排。后端不可用时登记表按空
 * 处理：首启会进引导，引导可「稍后再建」跳过，不阻塞开发调试。
 */
export function GatedShell() {
  const loaded = useSettingsStore((s) => s.loaded);
  const onboardingCompleted = useSettingsStore((s) => s.settings.onboardingCompleted);
  const libraries = usePhotoLibraries();
  useWindowReveal(loaded);

  if (!loaded) return null;
  if (libraries !== null && libraries.length === 0 && !onboardingCompleted) {
    return <Navigate to="/onboarding" replace />;
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
  // 首次引导（M5：数据库就位 → 引导建立第一个照片库）
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
      { path: "map", element: <MapPage /> },
      { path: "search", element: <SearchRedirect /> },
      { path: "similar", element: <SimilarPage /> },
      // 选片会话（Culling V1）：会话列表 + 全屏过片层（浮层内挂载）
      { path: "culling", element: <CullingPage /> },
      { path: "people", element: <PeoplePage /> },
      { path: "albums", element: <AlbumsIndexPage /> },
      // 手工相册详情（/albums/:id，纯数字参数）/ 智能标签语义结果共用一个槽位
      { path: "albums/:tag", element: <AlbumEntryPage /> },
      { path: "import", element: <ImportPage /> },
      // 存储（M3）：照片库登记管理——新建/从文件夹建立/重定位/移除登记
      { path: "storage", element: <StoragePage /> },
      { path: "settings", element: <SettingsPage /> },
    ],
  },
  // 未知路径回落画廊（主壳守卫内：尚无照片库会被送去引导）
  { path: "*", element: <Navigate to="/gallery" replace /> },
]);

export function AppRoutes() {
  return <RouterProvider router={router} />;
}
