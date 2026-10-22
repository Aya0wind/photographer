import "../lib/brandFrameFonts.css";
import type { BrandFrame } from "@/ipc/api";
import { frameMetrics, frameBorderSvg, materializeFrame } from "../lib/brandFrame";

export default function BrandFramePreview({ frame, src, aspect, miniature = false }: { frame: BrandFrame; src: string | null; aspect: number; miniature?: boolean }) {
  const m = frameMetrics(frame, aspect);
  const footer = materializeFrame(frame).footerSvg;
  return <div className={miniature ? "relative w-full overflow-hidden" : "relative m-auto max-h-full max-w-full shrink-0 shadow-2xl"}
    style={{ aspectRatio: `${m.width}/${m.height}`, background: frame.background, containerType: "inline-size", overflow: "hidden", ...(!miniature ? { width: `min(100%, calc((100vh - 180px) * ${m.width / m.height}))` } : {}) }} data-testid={miniature ? undefined : "brand-frame-preview"}>
    {frame.backgroundStyle === "blur" && <>{src ? <img src={src} alt="" className="pointer-events-none absolute inset-0 h-full w-full object-cover" style={{ filter: `blur(${(frame.blur || 32) / m.width * 100}cqw)`, transform: "scale(1.12)" }} /> : <div className="absolute inset-0 bg-gradient-to-br from-[#72939a] via-[#d1bba0] to-[#32524e]" />}<div className="absolute inset-0" style={{ background: frame.background, opacity: (frame.frost ?? 50) / 100 }} /></>}
    {src ? <img src={src} alt="" draggable={false} className="absolute object-fill" style={{ left: `${m.x / m.width * 100}%`, top: `${m.y / m.height * 100}%`, width: `${1000 / m.width * 100}%`, height: `${m.photoHeight / m.height * 100}%` }} />
      : <div className="absolute bg-gradient-to-br from-[#849ca1] via-[#b3b5a0] to-[#385457]" style={{ left: `${m.x / m.width * 100}%`, top: `${m.y / m.height * 100}%`, width: `${1000 / m.width * 100}%`, height: `${m.photoHeight / m.height * 100}%` }} />}
    <img alt="" className="pointer-events-none absolute inset-0 h-full w-full" src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(`<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${m.width} ${m.height}"><g transform="translate(${m.x-frame.padding} ${m.y-frame.padding})">${frameBorderSvg(frame, m.photoHeight)}</g></svg>`)}`} />
    <div className="pointer-events-none absolute bottom-0" style={{ left: `${(m.width-m.footerWidth)/2/m.width*100}%`, width: `${m.footerWidth/m.width*100}%`, height: `${m.footerHeight / m.height * 100}%` }} dangerouslySetInnerHTML={{__html: footer.replace(/width="[\d.]+"/, 'width="100%"').replace(/height="[\d.]+"/, 'height="100%"')}} />
  </div>;
}
