import type { AssetDetailDto, BrandFrame } from "@/ipc/api";

import { OFFICIAL_BRAND_LOGOS, officialLogoData } from "./brandLogos";
export const FRAME_BRANDS = Object.keys(OFFICIAL_BRAND_LOGOS);
export const FRAME_FONTS = ["Inter", "Manrope", "Lora", "Segoe UI", "Helvetica Neue", "Arial", "Georgia", "Times New Roman", "Verdana", "Trebuchet MS", "Courier New", "Microsoft YaHei", "SimSun", "SimHei", "KaiTi"];
export const FRAME_COLORS = ["#FFFFFF", "#F4F0E8", "#E8E2D6", "#D8DFD5", "#D7E1EA", "#17202B", "#111111", "#782F37", "#B8945E", "#E9B94B", "#B8A4C9", "#30584A"];
export const FRAME_LAYOUTS = ["classic", "split", "center", "signature", "gallery", "film", "editorial", "minimal"] as const;
export type FrameLayout = typeof FRAME_LAYOUTS[number];
const escape = (s: string) => s.replace(/[&<>"']/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&apos;" })[c]!);

export function defaultBrandFrame(): BrandFrame {
  return { enabled: true, logoSize: 100, canvasRatio: 0, backgroundStyle: "solid", blur: 32, frost: 50, border: "none", layout: "classic", background: "#FFFFFF", foreground: "#202020", accent: "#B8945E", padding: 28, footerHeight: 130, fontSize: 24, font: "Inter", logo: "wordmark", brand: "SONY", camera: "", lens: "", parameters: "", date: "", caption: "", showCamera: true, showLens: false, showParameters: true, showDate: true, showCaption: false, rule: true, logoImage: "", footerSvg: "" };
}

export const FRAME_TEMPLATES: { id: string; patch: Partial<BrandFrame> }[] = [
  { id: "classic", patch: {} },
  { id: "noir", patch: { background: "#111111", foreground: "#F4F0E8", layout: "split" } },
  { id: "gallery", patch: { layout: "gallery", border: "single", padding: 72, footerHeight: 170, font: "Lora", rule: false } },
  { id: "instant", patch: { layout: "signature", padding: 45, footerHeight: 230, background: "#F4F0E8", font: "Lora", rule: false } },
  { id: "cinema", patch: { layout: "film", background: "#111111", foreground: "#E9B94B", accent: "#E9B94B", padding: 24, footerHeight: 150, font: "Courier New" } },
  { id: "minimal", patch: { layout: "minimal", padding: 0, footerHeight: 80, rule: false, fontSize: 17 } },
  { id: "center", patch: { layout: "center", footerHeight: 180, padding: 36, rule: false } },
  { id: "editorial", patch: { layout: "editorial", padding: 38, footerHeight: 210, font: "Lora", accent: "#782F37" } },
  { id: "sage", patch: { layout: "split", background: "#D8DFD5", foreground: "#243B30", accent: "#30584A", padding: 44 } },
  { id: "blue", patch: { layout: "classic", border: "corners", background: "#17202B", foreground: "#E3EBF4", accent: "#83AACD", padding: 40 } },
  { id: "wine", patch: { layout: "center", background: "#782F37", foreground: "#F4F0E8", accent: "#E9B94B", footerHeight: 180, padding: 48, rule: false } },
  { id: "sand", patch: { layout: "gallery", background: "#E8E2D6", foreground: "#524538", font: "Lora", padding: 64, footerHeight: 170, rule: false } },
  { id: "lavender", patch: { layout: "signature", background: "#B8A4C9", foreground: "#2D2438", padding: 38, footerHeight: 180, rule: false } },
  { id: "museum", patch: { layout: "gallery", border: "double", background: "#111111", foreground: "#E8E2D6", font: "Lora", padding: 90, footerHeight: 200, rule: false } },
  { id: "contact", patch: { layout: "film", background: "#F4F0E8", foreground: "#202020", accent: "#782F37", font: "Courier New", padding: 24 } },
  { id: "warm", patch: { layout: "editorial", background: "#B8945E", foreground: "#211B14", accent: "#782F37", font: "Lora", padding: 50, footerHeight: 210 } },
  { id: "glass-wide", patch: { canvasRatio: 16/9, backgroundStyle: "blur", background: "#111111", foreground: "#FFFFFF", padding: 100, footerHeight: 130, layout: "split", rule: false, frost: 36 } },
  { id: "glass-square", patch: { canvasRatio: 1, backgroundStyle: "blur", background: "#FFFFFF", foreground: "#202020", padding: 100, footerHeight: 130, layout: "minimal", rule: false, frost: 65 } },
  { id: "glass-story", patch: { canvasRatio: 9/16, backgroundStyle: "blur", background: "#17202B", foreground: "#FFFFFF", padding: 110, footerHeight: 180, layout: "center", rule: false, frost: 42 } },
  { id: "glass-natural", patch: { backgroundStyle: "blur", background: "#FFFFFF", foreground: "#202020", padding: 120, footerHeight: 160, layout: "signature", rule: false, frost: 72 } },
  { id: "mobile", patch: { layout: "split", padding: 0, footerHeight: 110, background: "#FFFFFF", foreground: "#202020", rule: false } },
  { id: "mobile-dark", patch: { layout: "split", padding: 0, footerHeight: 110, background: "#111111", foreground: "#FFFFFF", rule: false } },
  { id: "postcard", patch: { canvasRatio: 3/2, layout: "gallery", padding: 100, footerHeight: 170, background: "#F4F0E8", rule: false } },
  { id: "square-print", patch: { canvasRatio: 1, layout: "gallery", padding: 90, footerHeight: 180, border: "single", rule: false } },
  { id: "portrait-print", patch: { canvasRatio: 4/5, layout: "signature", padding: 90, footerHeight: 180, background: "#E8E2D6", rule: false } },
  { id: "panorama", patch: { canvasRatio: 16/9, layout: "minimal", padding: 50, footerHeight: 90, background: "#111111", foreground: "#FFFFFF", rule: false } },
];

export function framePhotoData(detail: Partial<AssetDetailDto>): Partial<BrandFrame> {
  const camera = detail.camera ?? "";
  const brand = FRAME_BRANDS.find(b => camera.toLowerCase().includes(b.toLowerCase())) ?? (/ILCE|ILME|DSC-/i.test(camera) ? "SONY" : /EOS/i.test(camera) ? "Canon" : "");
  return { brand, camera, lens: detail.lens ?? "", date: (detail.capturedAt ?? "").replace("T", " ").replace(/(?:Z|[+-]\d\d:\d\d)$/, ""), parameters: [detail.focalLength && `${detail.focalLength.replace(/\s*mm$/i, "")} mm`, detail.aperture && `f/${detail.aperture.replace(/^f\//i, "")}`, detail.shutter && `${detail.shutter.replace(/\s*s$/, "")} s`, detail.iso != null && `ISO ${detail.iso}`].filter(Boolean).join("   ·   ") };
}

export function frameFooterHeight(frame: BrandFrame): number {
  const logoScale = (frame.logoSize ?? 100) / 100;
  const logoHeight = frame.logo === "none" ? 0 : (frame.layout === "minimal" ? 28 : 40) * logoScale * 2.8;
  const lines = [frame.showLens && frame.lens, frame.showParameters && frame.parameters, frame.showDate && frame.date, frame.showCaption && frame.caption].filter(v => typeof v === "string" && v.trim()).length;
  const centered = frame.layout === "center";
  const textHeight = ["gallery", "signature", "minimal"].includes(frame.layout) ? frame.fontSize * 3.2 : frame.fontSize * (lines + (centered && frame.showCamera && frame.camera.trim() ? 1 : 0)) * 1.15 / (centered ? 0.43 : 0.37);
  return Math.max(frame.footerHeight, logoHeight, textHeight);
}

/** Footer artwork uses a fixed photo-width coordinate system, independent of crop/aspect. */
export function materializeFrame(frame: BrandFrame): BrandFrame {
  const w = 1000 + frame.padding * 2, h = frameFooterHeight(frame), p = frame.padding || 28, s = Math.min(frame.fontSize, h * 0.19), logoScale = (frame.logoSize ?? 100)/100;
  const center = ["center", "signature"].includes(frame.layout);
  const text = (value: string, x: number, y: number, size = s, anchor = "start", bold = false, color = frame.foreground, max = w - p * 2) => {
    if (!value.trim()) return "";
    // Conservative fitting also bounds long camera/lens names in font fallback environments.
    const units = [...value].reduce((sum, c) => sum + (/[^\x00-\xff]/.test(c) ? 1 : 0.62), 0);
    const fit = Math.min(size, max / Math.max(1, units));
    return `<text x="${x}" y="${y}" fill="${escape(color)}" font-family="${escape(frame.font)}" font-size="${fit}" font-weight="${bold && frame.font !== "Lora" ? 700 : 400}" text-anchor="${anchor}">${escape(value)}</text>`;
  };
  let brand = "", brandBottom = 0;
  const bx = frame.layout === "gallery" ? p : center ? w / 2 : p;
  const by = frame.layout === "minimal" ? h*0.57-s*0.3+28*logoScale*0.25 : h*(center ? 0.26 : 0.36);
  if (frame.logo === "custom" && frame.logoImage) {
    const boxW=160*logoScale, boxH=(frame.layout === "minimal" ? 28 : 40)*logoScale;
    brandBottom=Math.max(6,by-boxH*0.75)+boxH;
    brand = `<image href="${escape(frame.logoImage)}" x="${bx - (center ? boxW/2 : 0)}" y="${Math.max(6,by-boxH*0.75)}" width="${boxW}" height="${boxH}" preserveAspectRatio="xMidYMid meet"/>`;
  } else if (frame.logo === "camera") {
    brandBottom=Math.max(6,by-28*logoScale)+36*logoScale;
    brand = `<g transform="translate(${bx - (center ? 24*logoScale : 0)},${Math.max(6,by-28*logoScale)}) scale(${logoScale})" fill="none" stroke="${escape(frame.foreground)}" stroke-width="3"><rect x="0" y="6" width="48" height="30" rx="5"/><path d="M8 6l5-7h17l5 7"/><circle cx="24" cy="21" r="10"/></g>`;
  } else if (frame.logo === "wordmark") {
    const logo = officialLogoData(frame.brand);
    if (logo) {
      const boxH = (frame.layout === "minimal" ? 28 : 40)*logoScale, boxW = 160*logoScale;
      const x = bx - (center ? boxW / 2 : 0), y = Math.max(6, by - boxH * 0.75);
      const rgb=parseInt(frame.background.slice(1),16), dark=(((rgb>>16)&255)*0.2126+((rgb>>8)&255)*0.7152+(rgb&255)*0.0722)<115;
      const lightPlate=["Canon","FUJIFILM","OM SYSTEM","RICOH","HUAWEI"].includes(frame.brand) && (dark || frame.backgroundStyle === "blur");
      const plate=logo.white ? "#111111" : lightPlate ? "#FFFFFF" : null;
      brandBottom=y+boxH+(plate?5:0);
      brand = (plate ? `<rect x="${x-6}" y="${y-5}" width="${boxW+12}" height="${boxH+10}" rx="3" fill="${plate}"/>` : "")
        + `<image href="${logo.url}" x="${x}" y="${y}" width="${boxW}" height="${boxH}" preserveAspectRatio="${center ? "xMidYMid" : "xMinYMid"} meet"/>`;
    }
  }

  const camera = frame.showCamera ? frame.camera : "", lens = frame.showLens ? frame.lens : "";
  const parameters = frame.showParameters ? frame.parameters : "", date = frame.showDate ? frame.date : "", caption = frame.showCaption ? frame.caption : "";
  let art = frame.rule ? `<path d="M${p} 1H${w - p}" stroke="${escape(frame.accent)}" stroke-width="1"/>` : "";
  if (frame.layout === "film") {
    art += Array.from({ length: 24 }, (_, i) => `<rect x="${i * w / 24 + 5}" y="8" width="18" height="8" rx="2" fill="${escape(frame.accent)}"/>`).join("");
  }
  art += brand;
  if (frame.layout === "gallery") {
    art = brand + text(caption || camera, w * 0.68, h * 0.30, s * 1.15, "middle", true, frame.foreground, w * 0.45)
      + text([parameters, date].filter(Boolean).join("   /   "), w / 2, h * 0.58, s * 0.8, "middle")
      + text([caption ? camera : "", lens].filter(Boolean).join("   ·   "), w / 2, h * 0.82, s * 0.72, "middle");
  } else if (frame.layout === "signature") {
    art += text(caption || camera, w / 2, h * 0.54, s * 1.2, "middle")
      + text([parameters, date].filter(Boolean).join("   ·   "), w / 2, h * 0.78, s * 0.75, "middle")
      + text(lens, w / 2, h * 0.92, s * 0.65, "middle");
  } else if (center) {
    const lines = [camera, lens, parameters, date, caption].filter(v => v.trim());
    const start = Math.max(h*0.46,brandBottom+s+10), gap = (h*0.92-start)/Math.max(1,lines.length-1);
    lines.forEach((line, i) => { art += text(line, w / 2, start + i * gap, Math.min(s, Math.max(8,gap * 0.8)), "middle"); });
  } else if (frame.layout === "minimal") {
    const logoWidth=(frame.logo === "wordmark" || frame.logo === "custom" ? 180 : frame.logo === "camera" ? 58 : 0)*logoScale;
    art = brand + text([camera, lens].filter(Boolean).join("  /  "), p + logoWidth, h * 0.57, s, "start", true, frame.foreground, Math.max(40,w*0.45-logoWidth))
      + text([parameters, date, caption].filter(Boolean).join("   ·   "), w - p, h * 0.57, s * 0.85, "end", false, frame.foreground, w * 0.48);
  } else {
    art += text(camera, w - p, h * 0.31, s, "end", true, frame.foreground, w * 0.52);
    const lines = [lens, parameters, date, caption].filter(v => v.trim());
    const start = frame.layout === "split" ? h*0.53 : Math.max(h*0.53,brandBottom+s+10), gap = (h*0.92-start)/Math.max(1,lines.length-1);
    lines.forEach((line, i) => { art += text(line, frame.layout === "split" ? w - p : p, start + i * gap, Math.min(s * (frame.layout === "editorial" ? 1 : 0.88), Math.max(8,gap * 0.8)), frame.layout === "split" ? "end" : "start",false,frame.foreground,frame.layout === "split" ? w*0.55 : w-2*p); });
    if (frame.layout === "editorial") art += `<rect x="${p}" y="${h * 0.43}" width="48" height="3" fill="${escape(frame.accent)}"/>`;
  }
  return { ...frame, footerSvg: `<svg xmlns="http://www.w3.org/2000/svg" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">${art}</svg>` };
}

export function frameMetrics(frame: BrandFrame, aspect: number) {
  const footerWidth = 1000 + frame.padding * 2, photoHeight = 1000 / Math.max(0.01, aspect);
  const footerHeight=frameFooterHeight(frame), minHeight = photoHeight + frame.padding + footerHeight;
  const ratio = frame.canvasRatio || 0;
  const width = ratio > 0 ? Math.max(footerWidth, minHeight * ratio) : footerWidth;
  const height = ratio > 0 ? width / ratio : minHeight;
  return { width, height, footerWidth, footerHeight, photoHeight, x: (width - 1000) / 2, y: frame.padding + (height - minHeight) / 2 };
}

export function frameBorderSvg(frame: BrandFrame, photoHeight: number): string {
  if (!frame.border || frame.border === "none" || frame.padding < 8) return "";
  const p = frame.padding - 5, w = 1010, h = photoHeight + 10;
  const rect = (inset: number) => `<rect x="${p-inset}" y="${p-inset}" width="${w+2*inset}" height="${h+2*inset}"/>`;
  const shape = frame.border === "corners" ? `<path d="M${p+32} ${p}h-32v32 M${p+w-32} ${p}h32v32 M${p} ${p+h-32}v32h32 M${p+w-32} ${p+h}h32v-32"/>` : rect(0) + (frame.border === "double" ? rect(4) : "");
  return `<g fill="none" stroke="${escape(frame.accent)}" stroke-width="1">${shape}</g>`;
}
