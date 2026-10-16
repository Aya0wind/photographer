import type {EditRecipe,EditorLensSettings} from "@/ipc/api";
import {planeToSource} from "./geometry";
export const DEFAULT_LENS:EditorLensSettings={enabled:true,distortion:0,redCyan:0,blueYellow:0,vignette:0,midpoint:50,horizontal:0,vertical:0,scale:100,purple:0,green:0,extendEdges:false};
type Point={x:number;y:number};
type Matrix=NonNullable<EditorLensSettings["guide"]>;
type Vector=[number,number];
export interface LensGuide {axis:"vertical"|"horizontal";from:Point;to:Point}
function vanishing(lines:LensGuide[],size:{width:number;height:number}):[number,number,number]{
  const radius=Math.hypot(size.width,size.height)/2;
  const point=(p:Point)=>[(p.x-0.5)*size.width/radius,(p.y-0.5)*size.height/radius];
  const equation=(g:LensGuide)=>{const a=point(g.from),b=point(g.to);return [a[1]-b[1],b[0]-a[0],a[0]*b[1]-b[0]*a[1]];};
  const a=equation(lines[0]),b=equation(lines[1]);
  return [a[1]*b[2]-b[1]*a[2],a[2]*b[0]-b[2]*a[0],a[0]*b[1]-b[0]*a[1]];
}
function unit(p:Vector):Vector{const length=Math.hypot(...p);if(length<1e-8)throw new Error("Guide lines must be distinct");return [p[0]/length,p[1]/length];}
function multiply(a:Matrix,b:Matrix):Matrix{return Array.from({length:9},(_,i)=>[0,1,2].reduce((sum,k)=>sum+a[Math.floor(i/3)*3+k]*b[k*3+i%3],0)) as Matrix;}
/** Rectify manually supplied vanishing directions, including the current display rotation. */
export function fitLensGuides(lines:LensGuide[],value:EditorLensSettings,size:{width:number;height:number},recipe:EditRecipe):EditorLensSettings{
  const vertical=lines.filter(g=>g.axis==="vertical"),horizontal=lines.filter(g=>g.axis==="horizontal");
  if(vertical.length<2&&horizontal.length<2)throw new Error("Draw two vertical or two horizontal guide lines");
  const v=vertical.length>=2?vanishing(vertical,size):null,h=horizontal.length>=2?vanishing(horizontal,size):null;
  let kh=0,kv=0;
  if(v&&h){const determinant=v[0]*h[1]-h[0]*v[1];if(Math.abs(determinant)<1e-10)throw new Error("Guide directions are not independent");kh=(v[1]*h[2]-h[1]*v[2])/determinant;kv=(h[0]*v[2]-v[0]*h[2])/determinant;}
  else if(v){const denominator=v[0]*v[0]+v[1]*v[1];if(denominator<1e-12)throw new Error("Vertical guides must be distinct");kh=-v[2]*v[0]/denominator;kv=-v[2]*v[1]/denominator;}
  else if(h){const denominator=h[0]*h[0]+h[1]*h[1];if(denominator<1e-12)throw new Error("Horizontal guides must be distinct");kh=-h[2]*h[0]/denominator;kv=-h[2]*h[1]/denominator;}
  const center=planeToSource(0.5,0.5,recipe,size);
  function direction(x:number,y:number):Vector{const p=planeToSource(0.5+x*0.01,0.5+y*0.01,recipe,size);return unit([(p[0]-center[0])*size.width,(p[1]-center[1])*size.height]);}
  const targetH=direction(1,0),targetV=direction(0,1);
  let sourceH:Vector,sourceV:Vector;
  if(h)sourceH=unit([h[0],h[1]]);else{sourceV=unit([v![0],v![1]]);sourceH=[sourceV[1],-sourceV[0]];}
  if(v)sourceV=unit([v[0],v[1]]);else sourceV=[-sourceH[1],sourceH[0]];
  if(sourceH[0]*targetH[0]+sourceH[1]*targetH[1]<0)sourceH=[-sourceH[0],-sourceH[1]];
  if(sourceV[0]*targetV[0]+sourceV[1]*targetV[1]<0)sourceV=[-sourceV[0],-sourceV[1]];
  const det=sourceH[0]*sourceV[1]-sourceV[0]*sourceH[1];if(Math.abs(det)<0.05)throw new Error("Guide directions are too close");
  const factor=Math.sqrt(Math.abs(det));
  const a=(targetH[0]*sourceV[1]-targetV[0]*sourceH[1])/det*factor,b=(-targetH[0]*sourceV[0]+targetV[0]*sourceH[0])/det*factor;
  const c=(targetH[1]*sourceV[1]-targetV[1]*sourceH[1])/det*factor,d=(-targetH[1]*sourceV[0]+targetV[1]*sourceH[0])/det*factor;
  const correction:Matrix=[a,b,0,c,d,0,kh,kv,1];
  const guide=multiply(correction,value.guide??[1,0,0,0,1,0,0,0,1]);
  if(guide.some(v=>!Number.isFinite(v)||Math.abs(v)>100))throw new Error("Required perspective correction exceeds the supported range");
  return {...value,guide};
}
