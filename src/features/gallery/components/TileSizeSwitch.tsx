import { useTranslation } from "react-i18next";
import type { GalleryTileSize } from "../lib/useGalleryTileSize";

/** Match the browse toolbar's circular buttons; click alternates the two sizes. */
export default function TileSizeSwitch({ value, onChange }: {
  value: GalleryTileSize;
  onChange: (next: GalleryTileSize) => void;
}) {
  const { t } = useTranslation();
  const next: GalleryTileSize = value === "small" ? "medium" : "small";
  const label = t("gallery.tileSize.toggle", { size: t(`gallery.tileSize.${next}`) });
  const columns = next === "small" ? 3 : 2;
  const side = columns === 3 ? 3 : 5.5;
  const gap = columns === 3 ? 1.5 : 2;
  const start = (20 - columns * side - (columns - 1) * gap) / 2;
  return (
    <button type="button" className="ui-icon-button" onClick={() => onChange(next)}
      aria-label={label} title={label} data-testid="gallery-tile-size" data-size={value}>
      <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor"
        strokeWidth="1.5" aria-hidden="true">
        {Array.from({ length: columns * columns }, (_, index) => (
          <rect key={index} x={start + (index % columns) * (side + gap)}
            y={start + Math.floor(index / columns) * (side + gap)}
            width={side} height={side} rx="0.8" />
        ))}
      </svg>
    </button>
  );
}
