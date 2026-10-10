import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, afterEach, expect, it, vi } from "vitest";
import { ipc } from "@/ipc";
import { listen } from "@tauri-apps/api/event";
import { useNativeEditorCanvas } from "./useNativeEditorCanvas";
import { advancedRecipe } from "./advancedRecipe";

vi.mock("@tauri-apps/api/core",()=>({isTauri:()=>true}));
vi.mock("@tauri-apps/api/event",()=>({listen:vi.fn()}));
vi.mock("@/ipc",()=>({ipc:vi.fn()}));
let receive:(event:{payload:{sessionId:string;ok:boolean;error?:string}})=>void;
let host:{current:HTMLElement|null};
const recipe=advancedRecipe();
beforeEach(()=>{
  vi.clearAllMocks();
  vi.mocked(ipc).mockImplementation(async (command)=>command==="edit_native_open"?true:undefined as never);
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
