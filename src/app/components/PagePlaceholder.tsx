import { useTranslation } from "react-i18next";

import type { ReactNode } from "react";

interface PagePlaceholderProps {
  titleKey: string;
  descKey: string;
  /** 卡片底部附加内容（如设置页的库根目录行） */
  children?: ReactNode;
}

/**
 * 里程碑占位页通用骨架：居中卡片（surface 底、panel 边框、圆角 12px、padding 32px），
 * 内为标题 + muted 描述。
 */
export default function PagePlaceholder({
  titleKey,
  descKey,
  children,
}: PagePlaceholderProps) {
  const { t } = useTranslation();

  return (
    <div className="flex h-full items-center justify-center p-8">
      <div className="w-full max-w-md rounded-xl border border-panel bg-surface p-8 text-center">
        <h1 className="text-lg font-semibold text-text-primary">{t(titleKey)}</h1>
        <p className="mt-2 text-sm leading-relaxed text-text-muted">{t(descKey)}</p>
        {children}
      </div>
    </div>
  );
}
