import type { EditableMetadata } from "@/ipc/api";
import { parseKeywords } from "../lib/exportOptions";

const INPUT = "h-9 w-full rounded-md border border-edge bg-bg px-3 text-sm text-text-primary outline-none focus:border-accent focus:ring-1 focus:ring-accent/20";

function localDate(value: string | null): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "";
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())}T${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

export default function MetadataFields({ value, onChange }: { value: EditableMetadata; onChange: (value: EditableMetadata) => void }) {
  const patch = (update: Partial<EditableMetadata>) => onChange({ ...value, ...update });
  return <section className="space-y-4" data-testid="editor-metadata-fields">
    <div><h3 className="text-sm font-semibold text-text-primary">照片信息</h3><p className="mt-1 text-xs leading-5 text-text-muted">保存到当前图库，保留原始照片。</p></div>
    {([['title', '标题'], ['author', '作者'], ['copyright', '版权']] as const).map(([key, label]) => <label key={key} className="block"><span className="mb-1.5 block text-xs font-medium text-text-secondary">{label}</span><input className={INPUT} value={value[key]} onChange={(e) => patch({ [key]: e.target.value })} data-testid={`editor-meta-${key}`} /></label>)}
    <label className="block"><span className="mb-1.5 block text-xs font-medium text-text-secondary">描述</span><textarea className={`${INPUT} h-24 resize-y py-2`} value={value.description} onChange={(e) => patch({ description: e.target.value })} data-testid="editor-meta-description" /></label>
    <label className="block"><span className="mb-1.5 block text-xs font-medium text-text-secondary">关键词</span><input className={INPUT} value={value.keywords.join(", ")} onChange={(e) => patch({ keywords: e.target.value.split(/[,，]/) })} onBlur={() => patch({ keywords: parseKeywords(value.keywords.join(",")) })} placeholder="用逗号分隔" data-testid="editor-meta-keywords" /></label>
    <div className="border-t border-edge pt-4"><h3 className="mb-3 text-sm font-semibold text-text-primary">拍摄信息</h3>
      <label className="mb-4 block"><span className="mb-1.5 block text-xs font-medium text-text-secondary">拍摄日期</span><input type="datetime-local" step="1" className={INPUT} value={localDate(value.capturedAt)} onChange={(e) => patch({ capturedAt: e.target.value ? new Date(e.target.value).toISOString() : null })} data-testid="editor-meta-capturedAt" /></label>
      {([['camera', '相机'], ['lens', '镜头']] as const).map(([key, label]) => <label key={key} className="mb-4 block"><span className="mb-1.5 block text-xs font-medium text-text-secondary">{label}</span><input className={INPUT} value={value[key]} onChange={(e) => patch({ [key]: e.target.value })} data-testid={`editor-meta-${key}`} /></label>)}
    </div>
    <div className="border-t border-edge pt-4"><div className="mb-3 flex items-center justify-between"><h3 className="text-sm font-semibold text-text-primary">拍摄位置</h3><button type="button" className="text-xs text-accent hover:underline" onClick={() => patch({ gpsLat: null, gpsLon: null })} data-testid="editor-meta-clear-gps">清除位置</button></div>
      <div className="grid grid-cols-2 gap-3">{([['gpsLat', '纬度', 90], ['gpsLon', '经度', 180]] as const).map(([key, label, bound]) => <label key={key}><span className="mb-1.5 block text-xs text-text-secondary">{label}</span><input type="number" step="any" min={-bound} max={bound} className={INPUT} value={value[key] ?? ""} onChange={(e) => patch({ [key]: e.target.value === "" ? null : Number(e.target.value) })} data-testid={`editor-meta-${key}`} /></label>)}</div>
    </div>
  </section>;
}
