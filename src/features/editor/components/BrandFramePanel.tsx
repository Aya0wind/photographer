import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { editAnnotationFonts, type BrandFrame } from "@/ipc/api";
import { defaultBrandFrame, FRAME_BRANDS, FRAME_COLORS, FRAME_FONTS, FRAME_LAYOUTS, FRAME_TEMPLATES, materializeFrame } from "../lib/brandFrame";
import BrandFramePreview from "./BrandFramePreview";

type Preset = { id: string; name: string; frame: BrandFrame };
const STORAGE_KEY = "photo-hub.brand-frame-presets.v1";
function readPresets(): Preset[] {
  try { const value: unknown = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "[]"); return Array.isArray(value) ? value.filter(v => typeof v?.id === "string" && typeof v?.name === "string" && v?.frame?.footerSvg && FRAME_LAYOUTS.includes(v.frame.layout)).slice(0, 40) : []; } catch { return []; }
}
export default function BrandFramePanel({ value, onChange, onReadMetadata }: { value: BrandFrame; onChange: (value: BrandFrame) => void; onReadMetadata: () => void }) {
  const { t } = useTranslation();
  const [category, setCategory] = useState("all");
  const [fonts, setFonts] = useState(FRAME_FONTS);
  const [presets, setPresets] = useState(readPresets);
  const [name, setName] = useState("");
  const [error, setError] = useState("");
  useEffect(() => { let active = true; void editAnnotationFonts().then(v => { if (active) setFonts([...new Set([...FRAME_FONTS, ...v])].sort()); }).catch(() => {}); return () => { active = false; }; }, []);
  const patch = (p: Partial<BrandFrame>) => onChange(materializeFrame({ ...value, ...p }));
  const input = "h-9 w-full min-w-0 rounded-md border border-edge bg-bg px-2 text-xs text-text-primary focus:border-accent focus:outline-none";
  const label = (key: string) => t(`editor.frame.${key}`);
  const persist = (next: Preset[]) => { try { localStorage.setItem(STORAGE_KEY, JSON.stringify(next)); setPresets(next); setError(""); } catch { setError(label("storageError")); } };
  function savePreset() {
    const title = name.trim(); if (!title) return;
    // A reusable preset contains presentation only; each photo keeps its own EXIF and caption.
    const frame = { ...value, camera: "", lens: "", parameters: "", date: "", caption: "", footerSvg: "" };
    persist([...presets.filter(p => p.name !== title), { id: crypto.randomUUID(), name: title, frame: materializeFrame(frame) }].slice(-40)); setName("");
  }
  function applyStyle(frame: BrandFrame) { patch({ ...frame, camera: value.camera, lens: value.lens, parameters: value.parameters, date: value.date, caption: value.caption, enabled: true }); }
  async function importLogo(file: File | undefined) {
    if (!file) return;
    if (!["image/png", "image/jpeg", "image/webp"].includes(file.type) || file.size > 512 * 1024) { setError(label("logoError")); return; }
    try {
      const data = await new Promise<string>((resolve, reject) => { const reader = new FileReader(); reader.onload = () => resolve(String(reader.result)); reader.onerror = reject; reader.readAsDataURL(file); });
      const image = new Image(); image.src = data; await image.decode();
      if (image.width > 4096 || image.height > 4096) throw new Error();
      patch({ logo: "custom", logoImage: data }); setError("");
    } catch { setError(label("logoError")); }
  }
  return <div className="space-y-5" data-testid="brand-frame-panel">
    <div className="flex items-center justify-between"><h2 className="text-sm font-semibold">{label("title")}</h2><label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={value.enabled} onChange={e => patch({ enabled: e.target.checked })} />{label("enabled")}</label></div>
    <section><h3 className="mb-2 text-xs text-text-muted">{label("templates")}</h3><div className="mb-2 flex gap-1">{["all","glass","mobile","print","creative"].map(v => <button type="button" key={v} aria-pressed={category===v} onClick={()=>setCategory(v)} className="flex-1 rounded-md border border-edge py-1.5 text-[10px] text-text-secondary aria-pressed:border-accent aria-pressed:text-accent">{label(`filter.${v}`)}</button>)}</div><div className="grid max-h-72 grid-cols-2 gap-2 overflow-y-auto pr-1">{FRAME_TEMPLATES.filter(template => category === "all" || (category === "glass" ? template.patch.backgroundStyle === "blur" : category === "mobile" ? template.id.startsWith("mobile") : category === "print" ? ["gallery","instant","sand","museum","postcard","square-print","portrait-print"].includes(template.id) : !template.patch.backgroundStyle && !template.id.startsWith("mobile"))).map(template => {
      const frame = materializeFrame({ ...defaultBrandFrame(), ...template.patch, brand: value.brand, camera: "ILCE-7RM5", parameters: "50 mm · f/1.8 · ISO 100", date: "2026.10.22" });
      const style={...defaultBrandFrame(),...template.patch};
      const selected=(["layout","background","foreground","accent","padding","footerHeight","fontSize","font","border","canvasRatio","backgroundStyle","blur","frost","rule"] as const).every(key=>value[key]===style[key]);
      return <button type="button" aria-pressed={selected} key={template.id} onClick={() => applyStyle({ ...defaultBrandFrame(), brand: value.brand, ...template.patch })} className="overflow-hidden rounded-lg border border-edge bg-panel p-2 text-left hover:border-accent aria-pressed:border-accent aria-pressed:bg-accent/5" data-testid={`frame-template-${template.id}`}><BrandFramePreview frame={frame} src={null} aspect={1.5} miniature /><span className="mt-1.5 block text-[11px]">{label(`template.${template.id}`)}</span></button>;
    })}</div></section>
    <section className="space-y-3 border-t border-edge pt-4"><h3 className="text-xs text-text-muted">{label("appearance")}</h3>
      <label className="block space-y-1 text-xs"><span>{label("canvasRatio")}</span><select className={input} value={value.canvasRatio || 0} onChange={e => patch({ canvasRatio: Number(e.target.value) })}>{[[0,"natural"],[1,"square"],[3/2,"landscape"],[16/9,"wide"],[4/5,"portrait"],[2/3,"tall"],[9/16,"story"]].map(([ratio,key]) => <option key={key} value={ratio}>{label(`ratio.${key}`)}</option>)}</select></label>
      <label className="block space-y-1 text-xs"><span>{label("backgroundStyle")}</span><select className={input} value={value.backgroundStyle || "solid"} onChange={e => patch({ backgroundStyle: e.target.value as BrandFrame["backgroundStyle"] })}><option value="solid">{label("solid")}</option><option value="blur">{label("blurBackground")}</option></select></label>
      {value.backgroundStyle === "blur" && <div className="space-y-2">{([["blur",8,80],["frost",0,90]] as const).map(([key,min,max]) => <label key={key} className="block text-xs"><span className="flex justify-between"><span>{label(key)}</span><span className="tabular-nums text-text-muted">{value[key]}</span></span><input type="range" aria-label={label(key)} min={min} max={max} value={value[key]} onChange={e => patch({[key]: Number(e.target.value)})} className="mt-1 w-full accent-accent" /></label>)}</div>}
      <label className="block space-y-1 text-xs"><span>{label("layout")}</span><select className={input} value={value.layout} onChange={e => patch({ layout: e.target.value as BrandFrame["layout"] })}>{FRAME_LAYOUTS.map(v => <option key={v} value={v}>{label(`layout.${v}`)}</option>)}</select></label>
      <label className="block space-y-1 text-xs"><span>{label("border")}</span><select className={input} value={value.border ?? "none"} onChange={e => patch({ border: e.target.value as BrandFrame["border"] })}>{["none", "single", "double", "corners"].map(v => <option key={v} value={v}>{label(`border.${v}`)}</option>)}</select></label>
      {(["background", "foreground", "accent"] as const).map(key => <div key={key} className="space-y-1.5"><div className="flex items-center justify-between text-xs"><span>{label(key)}</span><input type="color" aria-label={label(key)} value={value[key]} onChange={e => patch({ [key]: e.target.value })} className="h-6 w-9 cursor-pointer rounded border border-edge bg-transparent" /></div><div className="flex gap-1">{FRAME_COLORS.map(color => <button key={color} type="button" aria-label={`${label(key)} ${color}`} title={color} onClick={() => patch({ [key]: color })} className="h-4 flex-1 rounded-sm border border-black/20 outline-offset-2" style={{ background: color, outline: value[key] === color ? "1px solid var(--color-accent)" : undefined }} />)}</div></div>)}
      {([ ["padding", 0, 120], ["footerHeight", 70, 260], ["fontSize", 12, 32] ] as const).map(([key, min, max]) => <label key={key} className="block text-xs"><span className="flex justify-between"><span>{label(key)}</span><span className="tabular-nums text-text-muted">{value[key]}</span></span><input className="mt-1 w-full accent-accent" type="range" aria-label={label(key)} min={min} max={max} value={value[key]} onChange={e => patch({ [key]: Number(e.target.value) })} /></label>)}
      <label className="block space-y-1 text-xs"><span>{label("font")}</span><select className={input} value={value.font} onChange={e => patch({ font: e.target.value })}>{[...new Set([value.font, ...fonts])].map(font => <option key={font} style={{ fontFamily: font }}>{font}</option>)}</select></label>
      <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={value.rule} onChange={e => patch({ rule: e.target.checked })} />{label("rule")}</label>
    </section>
    <section className="space-y-3 border-t border-edge pt-4"><h3 className="text-xs text-text-muted">{label("identity")}</h3>
      <select aria-label={label("logo")} className={input} value={value.logo} onChange={e => patch({ logo: e.target.value as BrandFrame["logo"] })}>{["wordmark", "camera", "custom", "none"].map(v => <option key={v} value={v}>{label(`logo.${v}`)}</option>)}</select>
      {value.logo === "wordmark" && <select aria-label={label("brand")} className={input} value={FRAME_BRANDS.includes(value.brand) ? value.brand : ""} onChange={e => patch({ brand: e.target.value })}><option value="">{label("chooseBrand")}</option>{FRAME_BRANDS.map(b => <option key={b}>{b}</option>)}</select>}

      {value.logo === "custom" && <label className="block cursor-pointer rounded-md border border-edge p-3 text-center text-xs hover:border-accent">{label("importLogo")}<input type="file" accept="image/png,image/jpeg,image/webp" className="sr-only" onChange={e => void importLogo(e.target.files?.[0])} />{value.logoImage && <img alt="" src={value.logoImage} className="mx-auto mt-2 h-10 max-w-full object-contain" />}</label>}
      <button type="button" className={`${input} hover:border-accent`} onClick={onReadMetadata}>{label("readMetadata")}</button>
      {([ ["camera", "showCamera"], ["lens", "showLens"], ["parameters", "showParameters"], ["date", "showDate"], ["caption", "showCaption"] ] as const).map(([key, visible]) => <label key={key} className="block space-y-1.5 text-xs"><span className="flex items-center gap-2"><input type="checkbox" aria-label={`${label("show")} ${label(key)}`} checked={value[visible]} onChange={e => patch({ [visible]: e.target.checked })} />{label(key)}</span><input aria-label={label(key)} className={input} maxLength={200} value={value[key]} onChange={e => patch({ [key]: e.target.value })} /></label>)}
    </section>
    <section className="space-y-2 border-t border-edge pt-4"><h3 className="text-xs text-text-muted">{label("saved")}</h3><div className="flex gap-2"><input aria-label={label("presetName")} className={input} placeholder={label("presetName")} maxLength={40} value={name} onChange={e => setName(e.target.value)} /><button type="button" onClick={savePreset} disabled={!name.trim()} className="shrink-0 rounded-md border border-edge px-3 text-xs hover:border-accent disabled:opacity-40">{label("save")}</button></div>{presets.map(p => <div key={p.id} className="flex gap-2"><button className={`${input} text-left hover:border-accent`} onClick={() => applyStyle(p.frame)}>{p.name}</button><button type="button" aria-label={`${label("delete")} ${p.name}`} onClick={() => persist(presets.filter(v => v.id !== p.id))} className="px-2 text-text-muted hover:text-red-400">×</button></div>)}</section>
    {error && <p role="alert" className="text-xs text-red-400">{error}</p>}
  </div>;
}
