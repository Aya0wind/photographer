import { useTranslation } from "react-i18next";

/**
 * 人物页（M4 v1 空态）：人脸聚类在 M4 后期接入（scrfd 检测 + arcface 特征 +
 * 聚类命名）。当前仅提供说明与占位网格，数据源接入后替换。
 */
export default function PeoplePage() {
  const { t } = useTranslation();

  return (
    <div className="h-full overflow-y-auto" data-testid="people-page">
      <div className="mx-auto flex h-full w-full max-w-[1600px] flex-col px-6 pt-4">
        <div className="flex shrink-0 items-baseline gap-3">
          <h1 className="text-sm font-semibold text-text-primary">{t("people.title")}</h1>
          <p className="text-xs text-text-muted">{t("people.desc")}</p>
        </div>

        {/* 占位网格（数据源 M4 后期接入；接入后为人脸聚类卡片） */}
        <div
          className="mt-4 grid flex-1 content-start grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-3 pb-6"
          data-testid="people-placeholder-grid"
        >
          {Array.from({ length: 12 }, (_, i) => (
            <div
              key={i}
              className="flex flex-col items-center gap-2 rounded-lg border border-edge/60 bg-surface/50 p-4"
              data-testid="people-placeholder-card"
            >
              <div className="flex h-16 w-16 items-center justify-center rounded-full bg-panel/60 text-text-muted">
                <svg
                  viewBox="0 0 24 24"
                  width="28"
                  height="28"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.3"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  aria-hidden="true"
                >
                  <circle cx="12" cy="8.5" r="3.5" />
                  <path d="M5 19.5c1.2-3.4 3.9-5 7-5s5.8 1.6 7 5" />
                </svg>
              </div>
              <span className="text-[11px] text-text-muted">{t("people.placeholderName")}</span>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
