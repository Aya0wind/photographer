import { useTranslation } from "react-i18next";
import { GALLERY_LAYOUT_ORDER, type GalleryLayout } from "../lib/useGalleryLayout";

/** Match the browse toolbar's circular buttons; click cycles to the next of the four layouts. */
export default function LayoutSwitch({ value, onChange }: {
  value: GalleryLayout;
  onChange: (next: GalleryLayout) => void;
}) {
  const { t } = useTranslation();
  const next = GALLERY_LAYOUT_ORDER[
    (GALLERY_LAYOUT_ORDER.indexOf(value) + 1) % GALLERY_LAYOUT_ORDER.length
  ];
  const label = t("gallery.layout.toggle", { next: t(`gallery.layout.${next}`) });
  return (
    <button type="button" className="ui-icon-button" onClick={() => onChange(next)}
      aria-label={label} title={label} data-testid="gallery-layout" data-layout={value}>
      <svg viewBox="0 0 20 20" width="17" height="17" fill="none" stroke="currentColor"
        strokeWidth="1.5" aria-hidden="true">
        {next === "square" && (
          /* 3×3 等大方块 */
          Array.from({ length: 9 }, (_, index) => {
            const side = 4.4;
            const gap = 1.2;
            const start = (20 - 3 * side - 2 * gap) / 2;
            return (
              <rect key={index} x={start + (index % 3) * (side + gap)}
                y={start + Math.floor(index / 3) * (side + gap)}
                width={side} height={side} rx="0.8" />
            );
          })
        )}
        {next === "tiles" && (
          /* 方格但每格内一个小内框：完整图片在格内留边 */
          Array.from({ length: 4 }, (_, index) => {
            const side = 5.5;
            const gap = 2;
            const start = (20 - 2 * side - gap) / 2;
            const x = start + (index % 2) * (side + gap);
            const y = start + Math.floor(index / 2) * (side + gap);
            return (
              <g key={index}>
                <rect x={x} y={y} width={side} height={side} rx="0.8" />
                <rect x={x + 1.5} y={y + 1.5} width={side - 3} height={side - 3} rx="0.5" />
              </g>
            );
          })
        )}
        {next === "justify" && (
          /* 两行横条：行内等高、宽度不等（对齐行示意） */
          <>
            <rect x="1.5" y="3.5" width="5" height="4.5" rx="0.8" />
            <rect x="7.5" y="3.5" width="6" height="4.5" rx="0.8" />
            <rect x="14.5" y="3.5" width="4" height="4.5" rx="0.8" />
            <rect x="1" y="11" width="6.5" height="4.5" rx="0.8" />
            <rect x="8.5" y="11" width="4.5" height="4.5" rx="0.8" />
            <rect x="14" y="11" width="5" height="4.5" rx="0.8" />
          </>
        )}
        {next === "masonry" && (
          /* 三列高低错落矩形（瀑布流示意） */
          <>
            <rect x="2" y="2.5" width="4.5" height="7" rx="0.8" />
            <rect x="7.75" y="5.5" width="4.5" height="6.5" rx="0.8" />
            <rect x="13.5" y="2.5" width="4.5" height="4.5" rx="0.8" />
          </>
        )}
      </svg>
    </button>
  );
}
