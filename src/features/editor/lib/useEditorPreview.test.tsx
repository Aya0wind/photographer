import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { editPreviewOpen, editPreviewPrepare, editPreviewClose, editPreviewRender, editPreviewActivate, type EditorPreviewSession } from "@/ipc/api";
import { useEditorPreview } from "./useEditorPreview";
import { advancedRecipe } from "./advancedRecipe";
import {startPreviewScheduler} from "./previewScheduler";

vi.mock("@/ipc/api", () => ({ editPreviewOpen:vi.fn(),editPreviewPrepare:vi.fn(),editPreviewClose:vi.fn(),editPreviewRender:vi.fn(),editPreviewActivate:vi.fn() }));
vi.mock("./previewScheduler", () => ({ startPreviewScheduler:vi.fn((_id,_recipe,callbacks) => {
  callbacks.setUrl("adjusted-frame");
  return {wake:vi.fn(),close:vi.fn()};
}) }));
vi.mock("./previewImages",()=>({preparePreviewImage:vi.fn().mockResolvedValue(undefined),releasePreviewImage:vi.fn()}));
const recipe=advancedRecipe();
const proxy:EditorPreviewSession={sessionId:"session",sourceUrl:"proxy",width:6000,height:4000,
  sensorRaw:true,bitDepth:"8 bit preview",histogram:[],warnings:[],nativeReady:false};
beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(editPreviewPrepare).mockReset();vi.mocked(editPreviewActivate).mockReset();vi.mocked(editPreviewRender).mockReset();
  vi.mocked(editPreviewOpen).mockResolvedValue(proxy);
  vi.mocked(editPreviewClose).mockResolvedValue();
});

it("keeps the cached RAW proxy for editing without starting native RAW preparation",async()=>{
  const {result}=renderHook(()=>useEditorPreview(1,recipe,"photos"));
  await waitFor(()=>expect(result.current.session?.sessionId).toBe("session"));
  expect(editPreviewPrepare).not.toHaveBeenCalled();
  expect(result.current.session?.bitDepth).toBe("8 bit preview");
  expect(result.current.url).toBe("adjusted-frame");
  expect(result.current.loading).toBe(false);
});
it("stops ordinary rendering after native activation and retains the same proxy",async()=>{
  const {result,rerender}=renderHook(({native,recipe})=>useEditorPreview(1,recipe,"photos",native),{initialProps:{native:false,recipe}});
  await waitFor(()=>expect(result.current.session).toBe(proxy));
  const ordinary=vi.mocked(startPreviewScheduler).mock.results[0].value;
  rerender({native:true,recipe});
  expect(ordinary.close).toHaveBeenCalledTimes(1);
  const changed={...recipe,adjustments:{brightness:25,contrast:0,saturation:0}};
  rerender({native:true,recipe:changed});expect(startPreviewScheduler).toHaveBeenCalledTimes(1);
  expect(result.current.session).toBe(proxy);
  expect(editPreviewPrepare).not.toHaveBeenCalled();
  expect(result.current.error).toBeNull();
  rerender({native:false,recipe:changed});expect(startPreviewScheduler).toHaveBeenCalledTimes(2);
  expect(vi.mocked(startPreviewScheduler).mock.calls[1][1]()).toBe(changed);
  expect(editPreviewOpen).toHaveBeenCalledTimes(1);
});

it("prepares a RAW source only for explicit color pipelines and publishes its candidate frame",async()=>{
 const linear={...recipe,colorPipeline:{version:1 as const,backend:"rawlab" as const,coreRevision:"pinned"}};
 const ready={...proxy,sourceUrl:"neutral-raw",previewUrl:"prepared-frame",nativeReady:true,bitDepth:"32 bit linear proxy"};
 vi.mocked(editPreviewPrepare).mockResolvedValue(ready);
 vi.mocked(editPreviewActivate).mockResolvedValue(ready);
 const {result}=renderHook(()=>useEditorPreview(1,linear,"photos"));
 await waitFor(()=>expect(result.current.session).toBe(ready));
 expect(editPreviewPrepare).toHaveBeenCalledWith("session",linear);
 expect(editPreviewActivate).toHaveBeenCalledWith("session",linear);
 expect(editPreviewRender).not.toHaveBeenCalled();
});

it("activates the prepared source with edits made while decoding",async()=>{
 const initial={...recipe,colorPipeline:{version:1 as const,backend:"rawlab" as const,coreRevision:"pinned"}};
 let finish!:(value:EditorPreviewSession)=>void;
 vi.mocked(editPreviewPrepare).mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
 const ready={...proxy,nativeReady:true,previewUrl:"current-frame"};vi.mocked(editPreviewActivate).mockResolvedValue(ready);
 const {result,rerender}=renderHook(({value})=>useEditorPreview(1,value,"photos"),{initialProps:{value:initial}});
 await waitFor(()=>expect(editPreviewPrepare).toHaveBeenCalledTimes(1));
 const changed={...initial,advanced:{...initial.advanced!,exposure:2}};rerender({value:changed});
 await act(async()=>{finish(ready);});
 await waitFor(()=>expect(result.current.session).toBe(ready));
 expect(editPreviewActivate).toHaveBeenCalledWith("session",changed);
});
it("drops prepared work when the editor closes",async()=>{
 const initial={...recipe,colorPipeline:{version:1 as const,backend:"rawlab" as const,coreRevision:"pinned"}};
 let finish!:(value:EditorPreviewSession)=>void;
 vi.mocked(editPreviewPrepare).mockImplementation(()=>new Promise(resolve=>{finish=resolve;}));
 const {unmount}=renderHook(()=>useEditorPreview(1,initial,"photos"));
 await waitFor(()=>expect(editPreviewPrepare).toHaveBeenCalledTimes(1));unmount();
 await act(async()=>{finish(proxy);});
 expect(editPreviewActivate).not.toHaveBeenCalled();expect(editPreviewClose).toHaveBeenCalledWith("session");
});
