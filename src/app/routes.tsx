import { createBrowserRouter, Navigate, RouterProvider } from "react-router";

import AppShell from "./shell/AppShell";
import GalleryPage from "@/features/gallery/pages/GalleryPage";
import ImportPage from "@/features/import/pages/ImportPage";
import OnboardingPage from "@/features/onboarding/pages/OnboardingPage";
import SearchPage from "@/features/search/pages/SearchPage";
import SettingsPage from "@/features/settings/pages/SettingsPage";
import TasksPage from "@/features/tasks/pages/TasksPage";
import { useSettingsStore } from "@/stores/settingsStore";

/** 主壳守卫：设置未加载完成时空白等待；未完成首启引导则强制进入向导。 */
function GatedShell() {
  const loaded = useSettingsStore((s) => s.loaded);
  const completed = useSettingsStore((s) => s.settings.onboardingCompleted);

  if (!loaded) return null;
  if (!completed) return <Navigate to="/onboarding" replace />;
  return <AppShell />;
}

export const router = createBrowserRouter([
  // 首次引导向导：独立于主壳全屏展示
  { path: "/onboarding", element: <OnboardingPage /> },
  {
    path: "/",
    element: <GatedShell />,
    children: [
      { index: true, element: <Navigate to="/gallery" replace /> },
      { path: "gallery", element: <GalleryPage /> },
      { path: "search", element: <SearchPage /> },
      { path: "import", element: <ImportPage /> },
      { path: "tasks", element: <TasksPage /> },
      { path: "settings", element: <SettingsPage /> },
    ],
  },
  // 未知路径统一回落到画廊
  { path: "*", element: <Navigate to="/gallery" replace /> },
]);

export function AppRoutes() {
  return <RouterProvider router={router} />;
}
