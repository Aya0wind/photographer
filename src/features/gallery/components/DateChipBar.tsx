import { useTranslation } from "react-i18next";

import type { AssetGroupDate } from "@/ipc/api";
import { UNKNOWN_GROUP_KEY, formatDateChip } from "../lib/assetGroups";

/**
 * 顶部日期 chips 条（画廊）：assetGroupDates 数据源，横向滚动；
 * 点击滚动到对应日期组（未知日期组含「未知」chip，排最前）。当前视口所在组高亮。
 */
export default function DateChipBar({
  dates,
  currentKey,
  onJump,
}: {
  dates: AssetGroupDate[];
  /** 当前视口组键（AssetGroup.key；无高亮传 null） */
  currentKey: string | null;
  /** date 为组日期（null=未知组） */
  onJump: (date: string | null) => void;
}) {
  const { t } = useTranslation();
  // 防御排序：未知组恒在最前，其余日期降序（正常后端已排好，不信任到达顺序）
  const ordered = [...dates].sort((a, b) => {
    if (a.date === null) return b.date === null ? 0 : -1;
    if (b.date === null) return 1;
    return a.date < b.date ? 1 : a.date > b.date ? -1 : 0;
  });

  return (
    <div
      className="sp-scroll flex min-w-0 flex-1 items-center gap-1.5 overflow-x-auto"
      data-testid="gallery-chips"
    >
      {ordered.map((entry) => {
        const key = entry.date ?? UNKNOWN_GROUP_KEY;
        const active = currentKey === (entry.date === null ? UNKNOWN_GROUP_KEY : entry.date.slice(0, 10));
        return (
          <button
            key={key}
            type="button"
            onClick={() => onJump(entry.date)}
            className={`shrink-0 rounded-full border px-2.5 py-1 font-mono text-[11px] tabular-nums transition-colors ${
              active
                ? "border-accent text-accent"
                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
            }`}
            title={entry.date ?? t("gallery.unknownDate")}
            data-testid={entry.date === null ? "gallery-chip-unknown" : "gallery-chip"}
            data-date={entry.date ?? "unknown"}
            data-count={entry.count}
          >
            {entry.date === null ? t("gallery.chipUnknown") : formatDateChip(entry.date)}
            <span className="ml-1 text-text-muted">{entry.count}</span>
          </button>
        );
      })}
    </div>
  );
}
