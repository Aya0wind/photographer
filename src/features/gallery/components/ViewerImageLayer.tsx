import { useState, type ImgHTMLAttributes } from "react";

export interface ViewerImageView { scale: number; x: number; y: number; rotation: number }

/** Render at the final pixel size instead of enlarging a cached compositor texture. */
export function viewerImageSize(source: { width: number; height: number }, viewport: { width: number; height: number }, scale: number, pixelRatio = 1, alignPixels = true) {
  if (source.width <= 0 || source.height <= 0 || viewport.width <= 0 || viewport.height <= 0) return null;
  const fit = Math.min(1, viewport.width / source.width, viewport.height / source.height);
  const ratio = Number.isFinite(pixelRatio) && pixelRatio > 0 ? pixelRatio : 1;
  const pixel = (value: number) => alignPixels ? Math.round(value * ratio) / ratio : value;
  return {
    width: Math.max(1 / ratio, pixel(source.width * fit * scale)),
    height: Math.max(1 / ratio, pixel(source.height * fit * scale)),
  };
}

export default function ViewerImageLayer({ viewport, view, dragging, animating = false, onLoad, className = "", highlightBounds, ...props }: ImgHTMLAttributes<HTMLImageElement> & {
  viewport: { width: number; height: number };
  view: ViewerImageView;
  dragging: boolean;
  animating?: boolean;
  highlightBounds?: number[] | null;
}) {
  const [source, setSource] = useState({ width: 0, height: 0 });
  const ratio = window.devicePixelRatio || 1;
  const align = !animating && !dragging;
  const size = viewerImageSize(source, viewport, view.scale, ratio, align);
  const snap = (value: number) => align ? Math.round(value * ratio) / ratio : value;
  return <><img {...props}
    className={`${className} select-none object-contain ${size ? "" : "max-h-full max-w-full"}`}
    onLoad={(event) => {
      const image = event.currentTarget;
      setSource({ width: image.naturalWidth, height: image.naturalHeight });
      onLoad?.(event);
    }}
    style={{
      ...props.style,
      ...(size ? { width: size.width, height: size.height, maxWidth: "none", maxHeight: "none" } : {}),
      // Scaling uses layout dimensions. Translate/rotate still preserve the image center.
      transform: `translate(${snap(view.x)}px, ${snap(view.y)}px) rotate(${view.rotation}deg)`,
      transition: "none",
    }}
  />
    {size && highlightBounds?.length === 4 && <div className="pointer-events-none absolute left-1/2 top-1/2 z-10"
      style={{ width: size.width, height: size.height,
        transform: `translate(calc(-50% + ${snap(view.x)}px), calc(-50% + ${snap(view.y)}px)) rotate(${view.rotation}deg)` }}>
      <div data-testid="selection-region-highlight" className="absolute rounded border-2 border-amber-400 bg-amber-400/10"
        style={{ left: `${highlightBounds[0]! * 100}%`, top: `${highlightBounds[1]! * 100}%`,
          width: `${highlightBounds[2]! * 100}%`, height: `${highlightBounds[3]! * 100}%` }} />
    </div>}
  </>;
}
