import {useCallback,useEffect,useRef,useState} from "react";
import {editPreviewPick,type AdvancedAdjustments} from "@/ipc/api";
import type {AdvancedAction} from "./advancedRecipe";
import {DEFAULT_ADVANCED} from "./advancedRecipe";
import type {RecipeHistory} from "./recipe";
import {addCurvePoint,curveSamples,moveCurvePoint,clampTone,IDENTITY,type CurveChannel,type HslAxis,type HslColorRange} from "./curves";
export interface SampleGesture {phase:"start"|"move"|"end"|"cancel";x:number;y:number;delta:number}
interface Props {sampleSize?:1|3|5|11;sessionId?:string;history:RecipeHistory;mode:"curveTarget"|"hslTarget"|null;maskId?:string;channel:CurveChannel;axis:HslAxis;dispatch:(action:AdvancedAction)=>void;onError:(error:string|null)=>void;onSample:(sample:[number,number,number],range?:HslColorRange)=>void}
interface Gesture {token:number;sessionId:string;before:RecipeHistory;maskId?:string;delta:number;ended:boolean;applied:boolean;update?: (delta:number)=>Partial<AdvancedAdjustments>}
export function useTargetAdjustment(props:Props){
 const latest=useRef(props);latest.current=props;
 const gesture=useRef<Gesture|null>(null),sequence=useRef(0);
 const [active,setActive]=useState(false),[busy,setBusy]=useState(false);
 const cancel=useCallback(()=>{const g=gesture.current;gesture.current=null;sequence.current++;if(g?.applied&&g.sessionId===latest.current.sessionId)latest.current.dispatch({type:"restoreHistory",history:g.before});setActive(false);setBusy(false);},[]);
 function apply(g:Gesture){
  if(!g.update||!g.delta)return;
  const patch=g.update(g.delta);const a=g.maskId?g.before.present.masks?.find(m=>m.id===g.maskId)?.advanced:g.before.present.advanced;
  if(g.maskId)latest.current.dispatch({type:"maskUpdate",id:g.maskId,patch:{advanced:{...DEFAULT_ADVANCED,...a,...patch}},record:false});
  else latest.current.dispatch({type:"advanced",patch,record:false});g.applied=true;
 }
 function finish(g:Gesture){if(g.applied)latest.current.dispatch({type:"commitFrom",snapshot:g.before.present});gesture.current=null;setActive(false);}
 const onGesture=useCallback((event:SampleGesture)=>{
  if(event.phase==="cancel"){cancel();return;}
  if(event.phase==="start"){
   cancel();const p=latest.current;if(!p.mode||!p.sessionId)return;p.onError(null);
   const g:Gesture={token:++sequence.current,sessionId:p.sessionId,before:p.history,maskId:p.maskId,delta:0,ended:false,applied:false};gesture.current=g;setActive(true);setBusy(true);
   const a=g.maskId?g.before.present.masks?.find(m=>m.id===g.maskId)?.advanced:g.before.present.advanced;
   void editPreviewPick(p.sessionId,g.before.present,event.x,event.y,p.mode==="curveTarget"?"point":"hsl",g.maskId,p.channel,p.sampleSize??1).then(result=>{
    if(gesture.current!==g||sequence.current!==g.token)return;
    if(!result.sample)throw Error("target-sample-missing");
    if(p.mode==="curveTarget"){
     const rgb=result.sample;const x=clampTone(p.channel==="red"?rgb[0]:p.channel==="green"?rgb[1]:p.channel==="blue"?rgb[2]:rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722);
     const raw=p.channel==="rgb"?a?.curves:a?.channelCurves?.[p.channel];const points=raw?.length?raw:IDENTITY;
     const near=points.findIndex(([input])=>Math.abs(input-x)<1);
     const samples=curveSamples(points),floor=Math.floor(x),output=samples[floor]+(samples[Math.min(255,floor+1)]-samples[floor])*(x-floor);
     const added=near>=0?{points,index:near}:addCurvePoint(points,x,output);if(added.index<0)throw Error("target-curve-limit");
     g.update=delta=>{const point=added.points[added.index];const curve=moveCurvePoint(added.points,added.index,point[0],point[1]+delta*255);return p.channel==="rgb"?{curves:curve}:{channelCurves:{...a?.channelCurves,[p.channel]:curve}};};
    } else {
     const range=result.range;if(!range)throw Error("target-hsl-no-color");
     const original={hue:0,saturation:0,lightness:0,...a?.hslEight?.[range]};
     g.update=delta=>({hslEight:{...a?.hslEight,[range]:{...original,[p.axis]:Math.max(-100,Math.min(100,original[p.axis]+delta*100))}}});
    }
    p.onSample(result.sample,result.range??undefined);apply(g);if(g.ended)finish(g);
   }).catch(error=>{if(gesture.current===g){cancel();p.onError(String(error));}}).finally(()=>{if(sequence.current===g.token)setBusy(false);});
   return;
  }
  const g=gesture.current;if(!g)return;g.delta=event.delta;apply(g);
  if(event.phase==="end"){g.ended=true;if(g.update)finish(g);}
 },[cancel]);
 useEffect(()=>{cancel();},[props.mode,props.sessionId,props.maskId,props.channel,props.axis,props.sampleSize,cancel]);
 useEffect(()=>()=>{sequence.current++;gesture.current=null;},[]);
 return {active,busy,onGesture,cancel};
}
