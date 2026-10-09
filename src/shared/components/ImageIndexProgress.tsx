import { useTranslation } from "react-i18next";
import type { IndexStatus } from "@/ipc/api/types";

/** Component progress explains why an overall photo is not fully indexed yet;
 * it adds no separate controls and never counts a partial photo as completed. */
export default function ImageIndexProgress({ status }: { status: IndexStatus | null }) {
  const { t }=useTranslation();
  if(!status) return null;
  return <div className="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-[10px] text-text-muted" data-testid="image-index-parts">
    <span>{t("settings.ai.index.thumbPart",{done:status.thumb.done,total:status.thumb.total})}</span>
    <span>{t("settings.ai.index.exifPart",{done:status.exif.done,total:status.exif.total})}</span>
    {status.eyes && status.blur && <span>{t("settings.ai.index.selectionPart",{
      eyes:status.eyes.done,blur:status.blur.done,total:status.image?.total ?? status.thumb.total,
    })}</span>}
  </div>;
}
