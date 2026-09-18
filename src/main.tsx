import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";

import "@/i18n";
import "@/styles/app.css";
import { AppRoutes } from "@/app/routes";
import { initSettings } from "@/stores/settingsStore";
import { initImportStore } from "@/stores/importStore";

// 启动时加载一次设置并订阅远端变更（内部静默容错）
void initSettings();
// 订阅唯一事件通道 app://event：设备扫描 / 导入进度 / 总结等（内部静默容错）
void initImportStore();

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AppRoutes />
    </QueryClientProvider>
  </React.StrictMode>,
);
