import React from "react";
import ReactDOM from "react-dom/client";

import "@/i18n";
import { initAppLanguage } from "@/i18n/settingsLanguage";
import { initAppTheme } from "@/lib/theme";
import "@/styles/app.css";
import { AppRoutes } from "@/app/routes";
import { initSettings } from "@/stores/settingsStore";
import { initImportStore } from "@/stores/importStore";
import { initAi } from "@/stores/aiStore";
import { initSmartTagAutoIndex } from "@/features/albums/lib/smartTagAutoIndex";

// 启动时加载一次设置并订阅远端变更（内部静默容错）
initAppLanguage();
initAppTheme();
void initSettings();
// 订阅唯一事件通道 app://event：设备扫描 / 导入进度 / 总结等（内部静默容错）
void initImportStore();
// 订阅 AI 模型下载 / 索引进度事件（M4，内部静默容错）
void initAi();
// 语义索引收尾后自动重建智能相册标签索引（内部静默容错）
void initSmartTagAutoIndex();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <AppRoutes />
  </React.StrictMode>,
);
