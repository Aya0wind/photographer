import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import { editPreviewOpen, editPreviewPrepare, editPreviewClose, type EditorPreviewSession } from "@/ipc/api";
import { useEditorPreview } from "./useEditorPreview";
import { advancedRecipe } from "./advancedRecipe";

vi.mock("@/ipc/api", () => ({ editPreviewOpen:vi.fn(),editPreviewPrepare:vi.fn(),editPreviewClose:vi.fn() }));
vi.mock("./previewScheduler", () => ({ startPreviewScheduler:vi.fn((_id,_recipe,callbacks) => {
  callbacks.setUrl("adjusted-frame");
  return {wake:vi.fn(),close:vi.fn()};
}) }));
const recipe=advancedRecipe();
const proxy:EditorPreviewSession={sessionId:"session",sourceUrl:"proxy",width:6000,height:4000,
  sensorRaw:true,bitDepth:"8 bit preview",histogram:[],warnings:[],nativeReady:false};
beforeEach(() => {
  vi.clearAllMocks();
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
  const {result,rerender}=renderHook(({native})=>useEditorPreview(1,recipe,"photos",native),{initialProps:{native:false}});
  await waitFor(()=>expect(result.current.session).toBe(proxy));
  rerender({native:true});
  expect(result.current.session).toBe(proxy);
  expect(editPreviewPrepare).not.toHaveBeenCalled();
  expect(result.current.error).toBeNull();
});
