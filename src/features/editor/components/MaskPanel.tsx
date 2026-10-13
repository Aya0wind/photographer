import {useState} from "react";
import {useTranslation} from "react-i18next";
import type {EditLocalMask,EditRecipe} from "@/ipc/api";
import {DEFAULT_ADVANCED,type AdvancedAction} from "../lib/advancedRecipe";
import {newLayerId} from "../lib/recipe";
import Slider from "./AdjustmentSlider";
import AdvancedAdjustmentPanel from "./AdvancedAdjustmentPanel";
import type {MaskBrush} from "./MaskCanvasControls";
import type {CurveSampling} from "./CurveEditor";
const BUTTON="rounded-lg border border-edge px-2 py-1.5 text-xs text-text-secondary hover:border-accent disabled:opacity-30";
export default function MaskPanel({recipe,selected,onSelect,editing,onEditing,brush,onBrush,dispatch,begin,end,onModalChange,sampling,showOverlay=true,onShowOverlay,cancel}:{
 cancel?:()=>void;
 recipe:EditRecipe;selected:string|null;onSelect:(id:string|null)=>void;editing:boolean;onEditing:(value:boolean)=>void;
 brush:MaskBrush;onBrush:(value:MaskBrush)=>void;dispatch:(action:AdvancedAction)=>void;begin:()=>void;end:()=>void;onModalChange:(value:boolean)=>void;
 sampling?:CurveSampling;
 showOverlay?:boolean;onShowOverlay?:(value:boolean)=>void;
}) {
 const [dragged,setDragged]=useState<string|null>(null);
 const {t}=useTranslation();const masks=recipe.masks??[],mask=masks.find(m=>m.id===selected);
 function update(patch:Partial<EditLocalMask>,record=true){if(mask)dispatch({type:"maskUpdate",id:mask.id,patch,record});}
 function add(kind:EditLocalMask["kind"]){
  end();const id=newLayerId();dispatch({type:"maskAdd",mask:{id,name:t(`advancedEditor.masks.${kind}`),kind,enabled:true,inverted:false,density:100,feather:0,from:{x:0.3,y:0.5},to:{x:0.7,y:0.5},strokes:[]}});onSelect(id);onEditing(true);
 }
 const local=mask?{...recipe,masks:undefined,legacyAdjustments:undefined,adjustments:mask.adjustments,advanced:mask.advanced}:recipe;
 return <div className="space-y-4">
  <div className="flex flex-wrap gap-1">{(["brush","linear","radial"] as const).map(kind=><button key={kind} type="button" className={BUTTON} disabled={masks.length>=32} onClick={()=>add(kind)}>＋ {t(`advancedEditor.masks.${kind}`)}</button>)}</div>
  {!masks.length&&<p className="text-xs leading-relaxed text-text-muted">{t("advancedEditor.masks.empty")}</p>}
  <p className="text-[11px] text-text-muted">{t("advancedEditor.masks.stackHint")}</p>
  <div className="space-y-1">{[...masks].reverse().map(item=><div key={item.id} draggable onDragStart={e=>{setDragged(item.id);e.dataTransfer.setData("application/x-photo-mask",item.id);e.dataTransfer.effectAllowed="move";}} onDragEnd={()=>setDragged(null)} onDragOver={e=>{if(dragged&&dragged!==item.id){e.preventDefault();e.dataTransfer.dropEffect="move";}}} onDrop={e=>{e.preventDefault();e.stopPropagation();if(dragged&&dragged!==item.id){end();dispatch({type:"maskReorder",id:dragged,targetId:item.id});}setDragged(null);}} className={`flex items-center gap-2 rounded-xl border p-2 ${selected===item.id?"border-accent bg-accent/10":"border-edge bg-bg"} ${dragged===item.id?"opacity-40":""}`}>
   <span aria-hidden="true" className="cursor-grab text-text-muted">⠿</span>
   <svg aria-hidden="true" className="h-10 w-10 shrink-0 rounded-md bg-[#202020]" viewBox="0 0 100 100"><rect width="100" height="100" fill={item.inverted?"#ddd":"#202020"}/>{item.kind==="radial"?<circle cx={item.from.x*100} cy={item.from.y*100} r={Math.hypot(item.to.x-item.from.x,item.to.y-item.from.y)*100} fill={item.inverted?"#202020":"#ddd"}/>:item.kind==="linear"?<path d={`M${item.from.x*100} ${item.from.y*100} L${item.to.x*100} ${item.to.y*100}`} stroke="#ddd" strokeWidth="35"/>:item.strokes.filter(s=>!s.erase).map((s,i)=><polyline key={i} points={s.points.map(p=>`${p.x*100},${p.y*100}`).join(" ")} stroke="#ddd" strokeWidth={s.widthRel*100} strokeLinecap="round" fill="none"/>)}</svg>
   <input type="checkbox" aria-label={t("advancedEditor.masks.enabled",{name:item.name})} checked={item.enabled} onChange={e=>{end();dispatch({type:"maskUpdate",id:item.id,patch:{enabled:e.target.checked}});}}/>
   <button className={`${BUTTON} min-w-0 flex-1 truncate text-left aria-pressed:border-accent aria-pressed:text-accent`} aria-pressed={selected===item.id} onKeyDown={e=>{if(!e.altKey||!["ArrowUp","ArrowDown"].includes(e.key))return;e.preventDefault();const index=masks.findIndex(m=>m.id===item.id),target=masks[index+(e.key==="ArrowUp"?1:-1)];if(target){end();dispatch({type:"maskReorder",id:item.id,targetId:target.id});}}} title={t("advancedEditor.masks.stackHint")} onClick={()=>{end();onSelect(item.id);}}>{item.name||t(`advancedEditor.masks.${item.kind}`)}</button>
  </div>)}</div>
  {mask&&<>
   <label className="block text-xs text-text-secondary">{t("advancedEditor.masks.name")}<input className="mt-1 w-full rounded-lg border border-edge bg-bg p-2" value={mask.name} maxLength={120} onFocus={begin} onBlur={end} onChange={e=>update({name:e.target.value},false)}/></label>
   <div className="flex gap-2"><button className={BUTTON} disabled={masks.length>=32} onClick={()=>{end();const id=newLayerId();dispatch({type:"maskDuplicate",id:mask.id,newId:id,name:t("advancedEditor.masks.copyName",{name:mask.name})});onSelect(id);}}>{t("advancedEditor.masks.duplicate")}</button>
    <button className={BUTTON} onClick={()=>{end();dispatch({type:"maskRemove",id:mask.id});onSelect(masks.find(m=>m.id!==mask.id)?.id??null);}}>{t("advancedEditor.masks.remove")}</button>
    <button className={`${BUTTON} aria-pressed:text-accent`} aria-pressed={mask.inverted} onClick={()=>update({inverted:!mask.inverted})}>{t("advancedEditor.masks.invert")}</button></div>
   <div className="flex gap-2">{[true,false].map(value=><button key={String(value)} className={`${BUTTON} flex-1 aria-pressed:border-accent aria-pressed:text-accent`} aria-pressed={editing===value} onClick={()=>{end();onEditing(value);}}>{t(value?"advancedEditor.masks.editArea":"advancedEditor.masks.adjust")}</button>)}</div>
   <p className="text-xs leading-relaxed text-text-muted">{t(editing?`advancedEditor.masks.${mask.kind}Hint`:"advancedEditor.masks.adjustHint")}</p>
   {onShowOverlay&&<label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={showOverlay} onChange={e=>onShowOverlay(e.target.checked)}/>{t("advancedEditor.masks.showOverlay")}</label>}
   <Slider label={t("advancedEditor.masks.density")} unit="%" min={0} max={100} defaultValue={100} value={mask.density} onChange={density=>update({density},false)} onSet={density=>update({density})} onReset={()=>update({density:100})} begin={begin} end={end}/>
   <Slider label={t("advancedEditor.masks.feather")} unit="%" min={0} max={100} value={mask.feather} onChange={feather=>update({feather},false)} onSet={feather=>update({feather})} onReset={()=>update({feather:0})} begin={begin} end={end}/>
   {editing&&mask.kind==="brush"&&<div className="space-y-2 border-t border-edge pt-3">
    <label className="flex items-center gap-2 text-xs"><input type="checkbox" checked={brush.erase} onChange={e=>onBrush({...brush,erase:e.target.checked})}/>{t("advancedEditor.masks.eraser")}</label>
    {(["widthRel","hardness","opacity","flow"] as const).map(key=><Slider key={key} label={t(`advancedEditor.masks.${key}`)} unit="%" min={key==="widthRel"?0.1:0} max={key==="widthRel"?50:100} step={0.1} value={brush[key]*100} onChange={value=>onBrush({...brush,[key]:value/100})} onSet={value=>onBrush({...brush,[key]:value/100})} onReset={()=>onBrush({...brush,[key]:key==="widthRel"?0.05:1})} defaultValue={key==="widthRel"?5:100} begin={()=>{}} end={()=>{}}/>)}
   </div>}
   {!editing&&<AdvancedAdjustmentPanel cancel={cancel} key={mask.id} recipe={local} begin={begin} end={end} onModalChange={onModalChange} sampling={sampling??{available:false,picker:null,setPicker:()=>{},sample:null,histogram:[]}}
    onResetDevelopment={group=>{end();dispatch({type:"maskResetDevelopmentGroup",id:mask.id,group});}}
    onBasic={(patch,record=false)=>update({adjustments:{brightness:0,contrast:0,saturation:0,...mask.adjustments,...patch}},record)}
    onAdvanced={(patch,record=true)=>update({advanced:{...DEFAULT_ADVANCED,...mask.advanced,...patch}},record)}
    onReset={group=>{end();dispatch({type:"maskResetAdjustmentGroup",id:mask.id,group});}}/>}
  </>}
 </div>;
}
