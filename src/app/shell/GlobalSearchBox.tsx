import { useState } from "react";
import { useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import { useAiStore } from "@/stores/aiStore";
import { useSemanticGate } from "@/features/ai/useSemanticSearch";
import { SemanticGateNotice } from "@/features/ai/SemanticResultsView";

/**
 * 顶部全局搜索框（M4.5 A1，TitleBar 动作位常驻）：
 * - 圆角输入框，占位「输入一段描述搜索照片…」；回车 → /gallery?mode=semantic&q=…
 *   （搜索页按 URL 协议预填并自动执行语义搜索）
 * - 右侧过滤图标 → /gallery（条件筛选在画廊工具条）
 * - 语义搜索前置门禁（模型未齐/索引从未建立）：回车不导航不发查询，输入文字
 *   保留；输入框下拉行内提示（带一键跳设置 AI tab），条件解除自动消失
 * - 语义索引未就绪（aiStore indexStatus ai.done<total）：输入框下细提示条
 *   （不阻塞输入；数据源与任务抽屉的常驻轮询同源）
 */
export default function GlobalSearchBox() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const [value, setValue] = useState("");
  const indexStatus = useAiStore((s) => s.indexStatus);
  const gate = useSemanticGate();
  const [gateBlocked, setGateBlocked] = useState(false);

  const aiIncomplete =
    indexStatus !== null && indexStatus.ai.total > 0 && indexStatus.ai.done < indexStatus.ai.total;

  function submit(): void {
    const q = value.trim();
    if (q && gate.blocked) {
      // 门禁拦截：不发查询、不导航；文字保留在输入框，下拉提示给出原因
      setGateBlocked(true);
      return;
    }
    setGateBlocked(false);
    navigate(q ? `/gallery?mode=semantic&q=${encodeURIComponent(q)}` : "/gallery");
  }

  return (
    <div className="pointer-events-auto relative flex items-center">
      <input
        type="text"
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            submit();
          }
        }}
        placeholder={t("globalsearch.placeholder")}
        aria-label={t("globalsearch.placeholder")}
        data-testid="globalsearch-input"
        className="h-7 w-56 rounded-md border border-edge bg-bg pl-7 pr-8 text-[11px] text-text-primary outline-none transition-colors placeholder:text-text-muted focus:w-64 focus:border-accent"
      />
      {/* 左侧放大镜 */}
      <svg
        viewBox="0 0 16 16"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
        strokeLinecap="round"
        className="pointer-events-none absolute left-2 text-text-muted"
        aria-hidden="true"
      >
        <circle cx="7" cy="7" r="4.5" />
        <path d="M10.5 10.5L14.5 14.5" />
      </svg>
      {/* 右侧过滤图标：跳条件筛选视图 */}
      <button
        type="button"
        onClick={() => navigate("/gallery")}
        aria-label={t("globalsearch.filter")}
        title={t("globalsearch.filter")}
        data-testid="globalsearch-filter"
        className="absolute right-1.5 flex h-5 w-5 items-center justify-center rounded text-text-muted transition-colors hover:text-accent"
      >
        <svg
          viewBox="0 0 16 16"
          width="12"
          height="12"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.4"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden="true"
        >
          <path d="M2 4h12M4.5 8h7M6.5 12h3" />
        </svg>
      </button>

      {/* 门禁拦截：下拉行内提示（原因 + 一键跳设置）；条件解除自动消失 */}
      {gateBlocked && gate.reason !== null && (
        <div className="absolute left-0 top-full z-30 mt-1 shadow-lg" data-testid="globalsearch-gate">
          <SemanticGateNotice reason={gate.reason} testId="globalsearch-gate-notice" />
        </div>
      )}

      {/* 语义索引未就绪：细提示条（绝对定位于输入框下，不挤占标题栏） */}
      {aiIncomplete && (
        <div
          className="absolute left-0 top-full z-20 mt-1 rounded border border-edge bg-surface px-2 py-1 text-[10px] leading-tight text-text-muted shadow-lg"
          data-testid="globalsearch-hint"
          role="status"
        >
          {t("globalsearch.indexHint")}
        </div>
      )}
    </div>
  );
}
