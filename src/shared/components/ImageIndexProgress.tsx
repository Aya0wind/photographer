import { useTranslation } from "react-i18next";
import type { IndexStatus } from "@/ipc/api/types";

/** Component progress explains why an overall photo is not fully indexed yet;
 * it adds no separate controls and never counts a partial photo as completed.
 * scope="image"（默认）：缩略图 + 详细信息两段；scope="selection"：选片
 * （闭眼 + 清晰度）一段——2026-10-12 拆分后分属两行。 */
export default function ImageIndexProgress({
  status,
  scope = "image",
}: { status: IndexStatus | null; scope?: "image" | "selection" }) {
  const { t }=useTranslation();
  if(!status) return null;
  if (scope === "selection") {
    return <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[10px] text-text-muted" data-testid="image-index-parts">
      {status.eyes && status.blur && <span>{t("settings.ai.index.selectionPart",{
        eyes:status.eyes.done,blur:status.blur.done,total:status.selection?.total ?? status.thumb.total,
      })}</span>}
    </div>;
  }
  return <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[10px] text-text-muted" data-testid="image-index-parts">
    <span>{t("settings.ai.index.thumbPart",{done:status.thumb.done,total:status.thumb.total})}</span>
    <span>{t("settings.ai.index.exifPart",{done:status.exif.done,total:status.exif.total})}</span>
  </div>;
}
