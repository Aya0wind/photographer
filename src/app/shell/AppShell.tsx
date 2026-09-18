import { Outlet, useLocation } from "react-router";
import { AnimatePresence, motion } from "motion/react";

import Sidebar from "./Sidebar";

/**
 * 应用主壳：左侧固定侧栏 + 内容区。
 * 页面切换时内容区做 opacity + 4px 位移过渡（180ms ease-out）。
 */
export default function AppShell() {
  const location = useLocation();

  return (
    <div className="flex h-full w-full overflow-hidden bg-bg font-sans text-text-primary">
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
  );
}
