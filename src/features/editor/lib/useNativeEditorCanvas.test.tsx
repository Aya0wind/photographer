import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { ipc } from "@/ipc";
import { listen } from "@tauri-apps/api/event";
import { useNativeEditorCanvas } from "./useNativeEditorCanvas";
import { advancedRecipe } from "./advancedRecipe";

vi.mock("@tauri-apps/api/core",()=>({isTauri:()=>true}));
vi.mock("@tauri-apps/api/event",()=>({listen:vi.fn()}));
vi.mock("@/ipc",()=>({ipc:vi.fn()}));
let receive:(event:{payload:{sessionId:string;revision?:number;ok:boolean;error?:string}})=>void;
let host:{current:HTMLElement|null};
const recipe=advancedRecipe();
it("hides an empty viewport and rejects its old confirmation after layout resumes",async()=>{
 const {result,rerender}=renderHook(({source})=>useNativeEditorCanvas("session",recipe,host,true,false,source),{initialProps:{source:"shown"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.anything()));
 host.current!.getBoundingClientRect=()=>new DOMRect(0,0,0,0);rerender({source:"empty"});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_hide",expect.anything()));
 const barrier=Math.max(...vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_hide").map(([,args])=>(args as {revision:number}).revision));
 host.current!.getBoundingClientRect=()=>new DOMRect(10,20,500,300);rerender({source:"restored"});await waitFor(()=>expect(vi.mocked(ipc).mock.calls.some(([command,args])=>command==="edit_native_frame"&&(args as {revision:number}).revision>barrier)).toBe(true));
 act(()=>receive({payload:{sessionId:"session",revision:barrier-1,ok:true}}));expect(result.current).toBe(false);
 const current=Math.max(...vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame").map(([,args])=>(args as {revision:number}).revision));act(()=>receive({payload:{sessionId:"session",revision:current,ok:true}}));expect(result.current).toBe(true);
});
it("ignores an in-flight frame failure from before a modal hide and resumes after it settles",async()=>{
 let reject!:(error:Error)=>void;let frames=0;
 vi.mocked(ipc).mockImplementation(async command=>{if(command==="edit_native_open")return true as never;if(command==="edit_native_frame"){if(++frames===1)await new Promise((_resolve,fail)=>{reject=fail;});return true as never;}return undefined as never;});
 const {result,rerender}=renderHook(({enabled})=>useNativeEditorCanvas("session",recipe,host,enabled,false,"source"),{initialProps:{enabled:true}});
 await waitFor(()=>expect(frames).toBe(1));rerender({enabled:false});rerender({enabled:true});
 await act(async()=>reject(Error("old frame failure")));await waitFor(()=>expect(frames).toBe(2));
 const current=Math.max(...vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame").map(([,args])=>(args as {revision:number}).revision));act(()=>receive({payload:{sessionId:"session",revision:current,ok:true}}));expect(result.current).toBe(true);
});
it("does not submit another queued frame after the editor unmounts",async()=>{
 let resolve!:()=>void;let frames=0;vi.mocked(ipc).mockImplementation(async command=>{if(command==="edit_native_open")return true as never;if(command==="edit_native_frame"){frames++;await new Promise<void>(done=>{resolve=done;});return true as never;}return undefined as never;});
 const {rerender,unmount}=renderHook(({split})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",undefined,split),{initialProps:{split:0.2}});await waitFor(()=>expect(frames).toBe(1));rerender({split:0.8});unmount();await act(async()=>resolve());
 expect(frames).toBe(1);expect(ipc).toHaveBeenCalledWith("edit_native_close",{sessionId:"session"});
});
it("keeps full frames when an older backend has not acknowledged recipe reuse",async()=>{
 vi.mocked(ipc).mockImplementation(async command=>command==="edit_native_open"?true:undefined as never);
 const {rerender}=renderHook(({split})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",undefined,split),{initialProps:{split:0.2}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe,view:expect.objectContaining({compareSplit:0.2})})));
 rerender({split:0.8});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe,view:expect.objectContaining({compareSplit:0.8})})));
 expect(vi.mocked(ipc).mock.calls.some(([command,payload])=>command==="edit_native_frame"&&(payload as {recipe:unknown}).recipe===null)).toBe(false);
});
it("serializes recipe changes and coalesces view updates while a frame is in flight",async()=>{
 let release!:()=>void;vi.mocked(ipc).mockImplementation(async command=>{if(command==="edit_native_open")return true as never;if(command==="edit_native_frame"){await new Promise<void>(resolve=>{release=resolve;});return true as never;}return undefined as never;});
 const {rerender}=renderHook(({value,split})=>useNativeEditorCanvas("session",value,host,true,false,"source",undefined,split),{initialProps:{value:recipe,split:0.2}});
 await waitFor(()=>expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame")).toHaveLength(1));
 rerender({value:recipe,split:0.4});rerender({value:recipe,split:0.8});await act(async()=>{await new Promise(resolve=>setTimeout(resolve,20));});
 expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame")).toHaveLength(1);
 await act(async()=>release());await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe:null,recipeRevision:1,view:expect.objectContaining({compareSplit:0.8})})));
 const changed={...recipe,rotateQuarter:1 as const};rerender({value:changed,split:0.8});await act(async()=>release());await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe:changed,recipeRevision:3})));
 await act(async()=>release());
});
it("sends a pending brush stroke as view data while keeping the persistent recipe unchanged",async()=>{
 const frame=host.current!.firstElementChild as HTMLElement;const controls=document.createElementNS("http://www.w3.org/2000/svg","svg");frame.appendChild(controls);
 controls.dataset.nativeMaskId="m";controls.dataset.nativeMaskGuide=JSON.stringify([0.2,0.5,0.8,0.5,0.1,0.1,3,1]);const stroke={points:[{x:0.2,y:0.5},{x:0.8,y:0.5}],widthRel:0.1,hardness:0.8,opacity:0.7,flow:0.5,erase:true};controls.dataset.nativeMaskStroke=JSON.stringify(stroke);
 const {rerender}=renderHook(({version})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",version),{initialProps:{version:"painting"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe,view:expect.objectContaining({maskStroke:stroke})})));
 delete controls.dataset.nativeMaskStroke;rerender({version:"canceled"});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe:null,recipeRevision:1,view:expect.objectContaining({maskStroke:null})})));
});
it("passes live mask guides and overlay visibility without requesting an encoded preview",async()=>{
 const frame=host.current!.firstElementChild as HTMLElement;const controls=document.createElementNS("http://www.w3.org/2000/svg","svg");frame.appendChild(controls);
 controls.dataset.nativeMaskId="m";controls.dataset.nativeMaskGuide=JSON.stringify([0.2,0.5,0.8,0.5,0.4,0.6,2,1]);
 const {rerender}=renderHook(({version})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",version),{initialProps:{version:"shown"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({maskId:"m",maskGuide:[0.2,0.5,0.8,0.5,0.4,0.6,2,1]})})));
 controls.dataset.nativeMaskGuide=JSON.stringify([0.2,0.5,0.7,0.6,0.3,0.4,2,0]);rerender({version:"hidden"});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({maskGuide:[0.2,0.5,0.7,0.6,0.3,0.4,2,0]})})));
 expect(vi.mocked(ipc).mock.calls.some(([command])=>command==="edit_preview_mask")).toBe(false);
});
it("updates clipping switches without reopening the source or requesting a bitmap",async()=>{
 const {rerender}=renderHook(({clipping})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",undefined,0.5,clipping),{initialProps:{clipping:0}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({clipping:0})})));
 rerender({clipping:3});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({clipping:3,compareSplit:0.5})})));
 expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_open")).toHaveLength(1);
 expect(vi.mocked(ipc).mock.calls.some(([command])=>command==="edit_preview_clipping")).toBe(false);
});
it("sends split changes as view parameters and temporarily disables the split for full original comparison",async()=>{
 const {rerender}=renderHook(({split,original})=>useNativeEditorCanvas("session",recipe,host,true,original,"source",undefined,split),{initialProps:{split:0.5,original:false}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({compareSplit:0.5})})));
 rerender({split:0.8,original:false});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({compareSplit:0.8})})));
 rerender({split:0.8,original:true});await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({original:true,view:expect.objectContaining({compareSplit:null})})));
 expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_open")).toHaveLength(1);
});
it("keeps the native child viewport when film settings and the prepared source change", async () => {
 const {rerender}=renderHook(({value,source})=>useNativeEditorCanvas("session",value,host,true,false,source),{initialProps:{value:recipe,source:"proxy"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe})));
 const film={...recipe,colorPipeline:{version:1 as const,backend:"rawlab" as const,coreRevision:"pinned"}};
 rerender({value:film,source:"linear-raw"});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe:film})));
 rerender({value:recipe,source:"linear-raw"});
 expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_open")).toHaveLength(1);
 expect(vi.mocked(ipc).mock.calls.some(([command])=>command==="edit_native_close"||command==="edit_native_hide")).toBe(false);
});
beforeEach(()=>{
  vi.clearAllMocks();
  vi.mocked(ipc).mockImplementation(async (command)=>["edit_native_open","edit_native_frame"].includes(command)?true:undefined as never);
  vi.mocked(listen).mockImplementation(async (_event,handler)=>{receive=handler as typeof receive;return ()=>{};});
  vi.stubGlobal("requestAnimationFrame",(callback:FrameRequestCallback)=>setTimeout(()=>callback(performance.now()),0));
  vi.stubGlobal("cancelAnimationFrame",clearTimeout);
  const element=document.createElement("main"),frame=document.createElement("div");
  frame.dataset.testid="editor-canvas-frame";element.appendChild(frame);document.body.appendChild(element);
  element.getBoundingClientRect=()=>new DOMRect(10,20,500,300);
  frame.getBoundingClientRect=()=>new DOMRect(30,40,200,100);
  host={current:element};
});
afterEach(()=>{host?.current?.remove();document.documentElement.classList.remove("native-editor-canvas");vi.unstubAllGlobals();});

it("activates only after a successful native frame and sends metadata rather than image bytes",async()=>{
  const {result,rerender}=renderHook(({enabled})=>useNativeEditorCanvas("session",recipe,host,enabled,false,"source"),{initialProps:{enabled:true}});
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({sessionId:"session",recipe,view:expect.objectContaining({uv:[0,0,1,1]})})));
  expect(result.current).toBe(false);
  act(()=>receive({payload:{sessionId:"session",ok:true}}));
  expect(result.current).toBe(true);
  expect(document.documentElement).not.toHaveClass("native-editor-canvas");
  rerender({enabled:false});
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_hide",expect.objectContaining({sessionId:"session",revision:expect.any(Number)})));
  expect(result.current).toBe(false);
  expect(document.documentElement).not.toHaveClass("native-editor-canvas");
});

it("returns to ordinary preview on GPU failure and releases the native window on unmount",async()=>{
  const log=vi.spyOn(console,"warn").mockImplementation(()=>{});
  const {result,unmount}=renderHook(()=>useNativeEditorCanvas("session",recipe,host,true,false,"source"));
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.any(Object)));
  act(()=>receive({payload:{sessionId:"session",ok:true}}));
  expect(result.current).toBe(true);
  act(()=>receive({payload:{sessionId:"session",ok:false,error:"device lost"}}));
  expect(result.current).toBe(false);
  expect(document.documentElement).not.toHaveClass("native-editor-canvas");
  unmount();expect(ipc).toHaveBeenCalledWith("edit_native_close",{sessionId:"session"});log.mockRestore();
});

it("zooms by changing image coordinates while leaving the native window bounds fixed",async()=>{
  const {rerender}=renderHook(({source})=>useNativeEditorCanvas("session",recipe,host,true,false,source),{initialProps:{source:"initial"}});
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.any(Object)));
  const first=vi.mocked(ipc).mock.calls.find(([command])=>command==="edit_native_frame")![1] as {view:{rect:number[];image:number[]}};
  const frame=host.current!.firstElementChild as HTMLElement;
  frame.getBoundingClientRect=()=>new DOMRect(-70,-10,400,200);
  rerender({source:"zoomed"});
  await waitFor(()=>expect(vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame")).toHaveLength(2));
  const second=vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame")[1][1] as typeof first;
  expect(second.view.rect).toEqual(first.view.rect);
  expect(second.view.image).not.toEqual(first.view.image);
});

it("keeps stale frames hidden while a modal is open and resumes with a newer revision",async()=>{
  const {result,rerender}=renderHook(({enabled})=>useNativeEditorCanvas("session",recipe,host,enabled,false,"source"),{initialProps:{enabled:true}});
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.any(Object)));
  rerender({enabled:false});
  act(()=>receive({payload:{sessionId:"session",ok:true}}));
  expect(result.current).toBe(false);
  const barrier=Math.max(...vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_hide").map(([,args])=>(args as {revision:number}).revision));
  rerender({enabled:true});
  await waitFor(()=>expect(vi.mocked(ipc).mock.calls.some(([command,args])=>command==="edit_native_frame" && (args as {revision:number}).revision>barrier)).toBe(true));
  act(()=>receive({payload:{sessionId:"session",revision:barrier-1,ok:true}}));expect(result.current).toBe(false);
  act(()=>receive({payload:{sessionId:"session",revision:barrier-1,ok:false,error:"old frame"}}));expect(result.current).toBe(false);
  const current=Math.max(...vi.mocked(ipc).mock.calls.filter(([command])=>command==="edit_native_frame").map(([,args])=>(args as {revision:number}).revision));
  act(()=>receive({payload:{sessionId:"session",revision:current,ok:true}}));expect(result.current).toBe(true);
});

it("clips the floating zoom HUD and removes the clip after it hides",async()=>{
 const hud=document.createElement("div");hud.dataset.nativeOverlay="";hud.dataset.visible="true";
 hud.getBoundingClientRect=()=>new DOMRect(110,250,180,40);host.current!.appendChild(hud);
 const {rerender}=renderHook(({version})=>useNativeEditorCanvas("session",recipe,host,true,false,"source",version),{initialProps:{version:"visible"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({overlays:[[100,230,180,40,16]]})})));
 hud.dataset.visible="false";rerender({version:"hidden"});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({overlays:[]})})));
});

it("sends crop guides and pixel scale without a JPEG request",async()=>{
 const frame=host.current!.firstElementChild as HTMLElement;frame.dataset.cropGuide=JSON.stringify([0.1,0.2,0.7,0.6]);
 renderHook(()=>useNativeEditorCanvas("session",recipe,host,true,false,"source"));
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({view:expect.objectContaining({cropGuide:[0.1,0.2,0.7,0.6],scale:1})})));
});
it("resends the full recipe when only the prepared source changes",async()=>{
 const {rerender}=renderHook(({source})=>useNativeEditorCanvas("session",recipe,host,true,false,source),{initialProps:{source:"proxy"}});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe})));
 vi.mocked(ipc).mockClear();rerender({source:"linear"});
 await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_frame",expect.objectContaining({recipe})));
});
