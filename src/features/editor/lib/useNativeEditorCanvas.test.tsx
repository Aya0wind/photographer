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
  expect(document.documentElement).toHaveClass("native-editor-canvas");
  rerender({enabled:false});
  await waitFor(()=>expect(ipc).toHaveBeenCalledWith("edit_native_hide",{sessionId:"session"}));
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
