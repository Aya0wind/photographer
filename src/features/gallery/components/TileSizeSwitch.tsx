import { useTranslation } from "react-i18next";

import {
  GALLERY_TILE_PX,
  type GalleryTileSize,
} from "../lib/useGalleryTileSize";

/**
 * 缩略图尺寸切换（两档图标）：小 120 / 大 200，localStorage 全局共享（画廊与搜索一致）。
 * （历史三档的 280 已删——网格缩略图实际只有 256 一档请求，最大档纯摆设）
 */

const ORDER: readonly GalleryTileSize[] = ["small", "medium"];
/** 档位图标：居中方块边长（12 viewBox 内） */
const ICON: Record<GalleryTileSize, number> = { small: 6, medium: 11 };

export default function TileSizeSwitch({
  value,
  onChange,
}: {
  value: GalleryTileSize;
  onChange: (next: GalleryTileSize) => void;
}) {
  const { t } = useTranslation();
  return (
    <div
      className="flex shrink-0 items-center rounded-md border border-edge bg-bg p-0.5"
      role="radiogroup"
      aria-label={t("gallery.tileSize.label")}
      data-testid="gallery-tile-size"
    >
      {ORDER.map((option) => {
        const active = value === option;
        return (
          <button
            key={option}
            type="button"
            role="radio"
            aria-checked={active}
            aria-label={t(`gallery.tileSize.${option}`)}
            title={`${t(`gallery.tileSize.${option}`)}（${GALLERY_TILE_PX[option]}px）`}
            onClick={() => onChange(option)}
            className={`flex w-7 items-center justify-center rounded transition-colors ${
              active ? "bg-accent text-black" : "text-text-secondary hover:text-text-primary"
            }`}
            data-testid={`gallery-tile-size-${option}`}
          >
            <svg
              viewBox="0 0 16 16"
              width="12"
              height="12"
              fill="none"
              stroke="currentColor"
              strokeWidth="1.5"
              aria-hidden="true"
            >
              <rect
                x={(16 - ICON[option]) / 2}
                y={(16 - ICON[option]) / 2}
                width={ICON[option]}
                height={ICON[option]}
                rx="1"
              />
            </svg>
          </button>
        );
      })}
    </div>
  );
}
