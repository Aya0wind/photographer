import i18n from "i18next";
import { initReactI18next } from "react-i18next";

import zh from "./zh.json";

void i18n.use(initReactI18next).init({
  resources: {
    zh: {
      translation: zh,
    },
  },
  lng: "zh",
  fallbackLng: "zh",
  interpolation: {
    // React 已经对渲染内容做了转义，无需 i18next 再转义一次
    escapeValue: false,
  },
});

export default i18n;
