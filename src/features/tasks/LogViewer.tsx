import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { importLogsPage, type LogLevel, type LogRow } from "@/ipc/api";
import { formatDateTime } from "@/lib/format";

/**
 * 日志查看器：等宽字体行（时间/级别/消息），级别筛选（error/warn/info），
 * 游标分页“加载更多”。数据不可用时优雅降级为空态。
 */

const PAGE_SIZE = 50;

const LEVEL_COLOR: Record<LogLevel, string> = {
  error: "text-red-400",
  warn: "text-yellow-300",
  info: "text-text-secondary",
};

const ALL_LEVELS: LogLevel[] = ["error", "warn", "info"];

export default function LogViewer({ jobId }: { jobId: number }) {
  const { t } = useTranslation();
  const [rows, setRows] = useState<LogRow[]>([]);
  const [exhausted, setExhausted] = useState(false);
  const [loading, setLoading] = useState(false);
  const [levels, setLevels] = useState<Set<LogLevel>>(() => new Set());
  const loadingRef = useRef(false);
  const cursorRef = useRef(0);

  async function load(reset: boolean): Promise<void> {
    if (loadingRef.current) return;
    loadingRef.current = true;
    setLoading(true);
    try {
      const afterId = reset ? 0 : cursorRef.current;
      const page = await importLogsPage(jobId, afterId, PAGE_SIZE);
      const safePage = Array.isArray(page) ? page : [];
      setRows((prev) => (reset ? safePage : [...prev, ...safePage]));
      cursorRef.current = safePage.length > 0 ? safePage[safePage.length - 1].id : afterId;
      setExhausted(safePage.length < PAGE_SIZE);
    } finally {
      loadingRef.current = false;
      setLoading(false);
    }
  }

  useEffect(() => {
    setRows([]);
    setExhausted(false);
    void load(true);
    // 仅随 jobId 重载（load 闭包依赖 cursorRef，无需追踪）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [jobId]);

  function toggleLevel(level: LogLevel): void {
    setLevels((prev) => {
      const next = new Set(prev);
      if (next.has(level)) next.delete(level);
      else next.add(level);
      return next;
    });
  }

  const visible = levels.size === 0 ? rows : rows.filter((r) => levels.has(r.level));

  return (
    <div data-testid={`log-viewer-${jobId}`}>
      {/* 级别筛选 */}
      <div className="flex items-center gap-1.5">
        <span className="text-[11px] text-text-muted">{t("logs.filter")}</span>
        {ALL_LEVELS.map((level) => {
          const active = levels.has(level);
          return (
            <button
              key={level}
              type="button"
              onClick={() => toggleLevel(level)}
              aria-pressed={active}
              className={`rounded px-1.5 py-0.5 font-mono text-[11px] transition-colors ${
                active ? `${LEVEL_COLOR[level]} bg-panel` : "text-text-muted hover:text-text-secondary"
              }`}
            >
              {level}
            </button>
          );
        })}
        <span className="ml-1 text-[11px] text-text-muted">
          {t("logs.count", { count: visible.length })}
        </span>
      </div>

      {/* 日志行 */}
      <div className="mt-1.5 max-h-48 overflow-y-auto rounded-md border border-panel bg-bg p-1.5">
        {visible.length === 0 ? (
          <p className="py-3 text-center font-mono text-[11px] text-text-muted">
            {t("logs.empty")}
          </p>
        ) : (
          visible.map((row) => (
            <div key={row.id} className="flex gap-2 whitespace-nowrap py-px font-mono text-[11px] leading-relaxed">
              <span className="shrink-0 text-text-muted">{formatDateTime(row.ts)}</span>
              <span className={`w-10 shrink-0 uppercase ${LEVEL_COLOR[row.level] ?? "text-text-secondary"}`}>
                {row.level}
              </span>
              <span className="min-w-0 flex-1 truncate text-text-secondary" title={row.message}>
                {row.message}
              </span>
            </div>
          ))
        )}
      </div>

      {/* 游标分页 */}
      <div className="mt-1.5 text-center">
        {!exhausted ? (
          <button
            type="button"
            disabled={loading}
            onClick={() => void load(false)}
            className="rounded border border-panel px-2.5 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-40"
          >
            {loading ? t("tasks.loading") : t("logs.loadMore")}
          </button>
        ) : (
          <span className="text-[11px] text-text-muted">{t("logs.allLoaded")}</span>
        )}
      </div>
    </div>
  );
}
