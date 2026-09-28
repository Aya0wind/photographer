import {
  createBrowserRouter,
  Navigate,
  RouterProvider,
  useSearchParams,
} from "react-router";

import AppShell from "./shell/AppShell";
import GalleryPage from "@/features/gallery/pages/GalleryPage";
import RecentPage from "@/features/gallery/pages/RecentPage";
import TrashPage from "@/features/gallery/pages/TrashPage";
import MemoriesPage from "@/features/memories/pages/MemoriesPage";
import GearPage from "@/features/gear/pages/GearPage";
import ImportPage from "@/features/import/pages/ImportPage";
import SimilarPage from "@/features/similar/pages/SimilarPage";
import LibraryPickerPage from "@/features/library/pages/LibraryPickerPage";
import OnboardingPage from "@/features/onboarding/pages/OnboardingPage";
import PeoplePage from "@/features/people/pages/PeoplePage";
import CullingPage from "@/features/culling/pages/CullingPage";
import { AlbumsIndexPage, AlbumEntryPage } from "@/features/albums/pages/AlbumsPages";
import SettingsPage from "@/features/settings/pages/SettingsPage";
import { useSettingsStore } from "@/stores/settingsStore";

/**
 * 主壳守卫（达芬奇式启动流）：设置未加载完成时空白等待；本会话未选库
 * （libraryChosen 为会话级标志，非持久——每次启动都先 /library-picker）或
 * activeLibraryId 无效时重定向选择器；选完库才进主壳。
 * onboardingCompleted 保留兼容（提交时仍写 true），但不再作门禁。
 */
export function GatedShell() {
  const loaded = useSettingsStore((s) => s.loaded);
  const libraryChosen = useSettingsStore((s) => s.libraryChosen);
  const activeLibraryId = useSettingsStore((s) => s.settings.activeLibraryId);
  const libraries = useSettingsStore((s) => s.settings.libraries);

  if (!loaded) return null;
  const hasActiveLibrary =
    activeLibraryId !== null && libraries.some((lib) => lib.id === activeLibraryId);
  if (!libraryChosen || !hasActiveLibrary) return <Navigate to="/library-picker" replace />;
  return <AppShell />;
}

/** /search → /gallery 重定向（M4.5 画廊+搜索合并；参数原样透传） */
export function SearchRedirect() {
  const [params] = useSearchParams();
  const query = params.toString();
  return <Navigate to={query ? `/gallery?${query}` : "/gallery"} replace />;
}

export const router = createBrowserRouter([
  // 启动首屏：库选择器（达芬奇式，每次启动先选库）
  { path: "/library-picker", element: <LibraryPickerPage /> },
  // 新建库配置链 / 未配置库补完（?library=<id>）：独立于主壳全屏展示
  { path: "/onboarding", element: <OnboardingPage /> },
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
      { path: "search", element: <SearchRedirect /> },
      { path: "similar", element: <SimilarPage /> },
      // 选片会话（Culling V1）：会话列表 + 全屏过片层（浮层内挂载）
      { path: "culling", element: <CullingPage /> },
      { path: "people", element: <PeoplePage /> },
      { path: "albums", element: <AlbumsIndexPage /> },
      // 手工相册详情（/albums/:id，纯数字参数）/ 智能标签语义结果共用一个槽位
      { path: "albums/:tag", element: <AlbumEntryPage /> },
      { path: "import", element: <ImportPage /> },
      { path: "settings", element: <SettingsPage /> },
    ],
  },
  // 未知路径回落画廊（仍受主壳守卫保护，未选库会被送去选择器）
  { path: "*", element: <Navigate to="/gallery" replace /> },
]);

export function AppRoutes() {
  return <RouterProvider router={router} />;
}
