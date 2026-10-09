import { act, renderHook, waitFor } from "@testing-library/react";
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

it("allows editing the proxy before original preparation finishes and retains the adjusted frame on upgrade", async () => {
  let finish!:(value:EditorPreviewSession)=>void;
  vi.mocked(editPreviewPrepare).mockImplementation(() => new Promise(resolve => {finish=resolve;}));
  const {result}=renderHook(() => useEditorPreview(1,recipe,"photos"));
  await waitFor(() => expect(result.current.session?.sessionId).toBe("session"));
  expect(result.current.loading).toBe(false);
  expect(result.current.preparingSource).toBe(true);
  expect(result.current.url).toBe("adjusted-frame");
  await act(async () => finish({...proxy,sourceUrl:"native",bitDepth:"16 bit",nativeReady:true}));
  await waitFor(() => expect(result.current.preparingSource).toBe(false));
  expect(result.current.session?.bitDepth).toBe("16 bit");
  expect(result.current.url).toBe("adjusted-frame");
});

it("does not replace an editable cached preview with a blocking error on native preparation failure", async () => {
  vi.mocked(editPreviewPrepare).mockRejectedValue(new Error("source unavailable"));
  const {result}=renderHook(() => useEditorPreview(1,recipe,"photos"));
  await waitFor(() => expect(result.current.sourceError).toContain("source unavailable"));
  expect(result.current.session).toBe(proxy);
  expect(result.current.loading).toBe(false);
  expect(result.current.error).toBeNull();
});
