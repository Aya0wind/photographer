import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { editPreviewRender, editPreviewStats } from "@/ipc/api";
import { preparePreviewImage, releasePreviewImage } from "./previewImages";
import { startPreviewScheduler } from "./previewScheduler";
import { advancedRecipe } from "./advancedRecipe";

vi.mock("@/ipc/api", () => ({ editPreviewRender: vi.fn(), editPreviewStats: vi.fn() }));
vi.mock("./previewImages", () => ({ preparePreviewImage: vi.fn(), releasePreviewImage: vi.fn() }));
beforeEach(() => {
  vi.useFakeTimers(); vi.clearAllMocks();
  vi.mocked(preparePreviewImage).mockResolvedValue();
  vi.mocked(editPreviewStats).mockResolvedValue({} as Awaited<ReturnType<typeof editPreviewStats>>);
});
afterEach(() => vi.useRealTimers());

it("coalesces slider changes and never swaps to a second resolution after settling", async () => {
  vi.mocked(editPreviewRender).mockResolvedValue("frame");
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",advancedRecipe,callbacks);
  for(let i=0;i<20;i++) scheduler.wake();
  await vi.advanceTimersByTimeAsync(40);
  expect(editPreviewRender).toHaveBeenCalledTimes(1);
  expect(editPreviewRender).toHaveBeenLastCalledWith("session",expect.any(Object),true,21);
  expect(callbacks.setUrl).toHaveBeenCalledWith("frame");
  await vi.advanceTimersByTimeAsync(1000);
  expect(editPreviewRender).toHaveBeenCalledTimes(1);
  scheduler.close();
});

it("shows completed frames during dragging and follows with the newest parameters", async () => {
  let complete!:(url:string)=>void;
  vi.mocked(editPreviewRender).mockImplementationOnce(() => new Promise(resolve => {complete=resolve;}))
    .mockResolvedValueOnce("latest");
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",advancedRecipe,callbacks);
  await vi.advanceTimersByTimeAsync(40);
  scheduler.wake(); scheduler.wake();
  expect(callbacks.setUrl).not.toHaveBeenCalled();
  complete("stale");
  await vi.advanceTimersByTimeAsync(40);
  expect(callbacks.setUrl).toHaveBeenNthCalledWith(1,"stale");
  expect(callbacks.setUrl).toHaveBeenCalledTimes(2);
  expect(callbacks.setUrl).toHaveBeenCalledWith("latest");
  expect(editPreviewRender).toHaveBeenCalledTimes(2);
  scheduler.close();
});

it("does not publish a frame after the editor has closed", async () => {
  let complete!:(url:string)=>void;
  vi.mocked(editPreviewRender).mockImplementation(() => new Promise(resolve => {complete=resolve;}));
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",advancedRecipe,callbacks);
  await vi.advanceTimersByTimeAsync(40);
  scheduler.close(); complete("closed");
  await vi.advanceTimersByTimeAsync(0);
  expect(callbacks.setUrl).not.toHaveBeenCalled();
  expect(releasePreviewImage).toHaveBeenCalledWith("closed");
});

it("uses the decoded source for an unedited image and renders the first actual change",async()=>{
  vi.mocked(editPreviewRender).mockResolvedValue("edited");
  let recipe=advancedRecipe();
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",()=>recipe,callbacks,"source");
  await vi.advanceTimersByTimeAsync(40);
  expect(editPreviewRender).not.toHaveBeenCalled();
  expect(callbacks.setUrl).toHaveBeenCalledWith("source");
  recipe={...recipe,advanced:{...recipe.advanced!,exposure:0.5}};scheduler.wake();
  await vi.advanceTimersByTimeAsync(40);
  expect(editPreviewRender).toHaveBeenCalledTimes(1);
  expect(callbacks.setUrl).toHaveBeenLastCalledWith("edited");
  recipe=advancedRecipe();scheduler.wake();
  await vi.advanceTimersByTimeAsync(40);
  expect(editPreviewRender).toHaveBeenCalledTimes(1);
  expect(callbacks.setUrl).toHaveBeenLastCalledWith("source");
  scheduler.close();
});

it("does not postpone rendering indefinitely when slider events arrive every 10 ms",async()=>{
  vi.mocked(editPreviewRender).mockResolvedValue("live");
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",advancedRecipe,callbacks);
  for(let i=0;i<20;i++) {
    scheduler.wake();
    await vi.advanceTimersByTimeAsync(10);
  }
  expect(vi.mocked(editPreviewRender).mock.calls.length).toBeGreaterThan(3);
  expect(callbacks.setUrl.mock.calls.length).toBeGreaterThan(3);
  scheduler.close();
});

it("a selected LUT is an edit and cannot be skipped as a neutral source",async()=>{
  vi.mocked(editPreviewRender).mockResolvedValue("lut-frame");
  const recipe=advancedRecipe();recipe.advanced!.lookup={id:"builtin:warm",amount:50,enabled:true};
  const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
  const scheduler=startPreviewScheduler("session",()=>recipe,callbacks,"source");
  await vi.advanceTimersByTimeAsync(16);
  expect(editPreviewRender).toHaveBeenCalledTimes(1);expect(callbacks.setUrl).toHaveBeenCalledWith("lut-frame");
  scheduler.close();
});

it("local tone and migrated filters cannot be skipped as a neutral source",async()=>{
 vi.mocked(editPreviewRender).mockResolvedValue("edited");
 let recipe=advancedRecipe();recipe.masks=[{id:"m",name:"Area",kind:"brush",enabled:true,inverted:false,density:100,feather:0,from:{x:0,y:0},to:{x:1,y:1},strokes:[],adjustments:{brightness:20,contrast:0,saturation:0}}];
 const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};
 const scheduler=startPreviewScheduler("session",()=>recipe,callbacks,"source");
 await vi.advanceTimersByTimeAsync(16);expect(editPreviewRender).toHaveBeenCalledTimes(1);
 recipe={...recipe,masks:[{...recipe.masks[0],enabled:false}]};scheduler.wake();
 await vi.advanceTimersByTimeAsync(16);expect(callbacks.setUrl).toHaveBeenLastCalledWith("source");
 recipe={...advancedRecipe(),legacyAdjustments:{brightness:10,contrast:5,saturation:-20}};scheduler.wake();
 await vi.advanceTimersByTimeAsync(16);expect(editPreviewRender).toHaveBeenCalledTimes(2);expect(callbacks.setUrl).toHaveBeenLastCalledWith("edited");
 scheduler.close();
});

it("recognizes extended color edits while skipping disabled black-white settings",async()=>{
 vi.mocked(editPreviewRender).mockResolvedValue("color-frame");
 let recipe=advancedRecipe();recipe.advanced!.blackWhite={enabled:false,weights:[40,60,40,60,20,80],tint:null};
 const callbacks={setUrl:vi.fn(),setError:vi.fn(),setPending:vi.fn()};const scheduler=startPreviewScheduler("s",()=>recipe,callbacks,"source");
 await vi.advanceTimersByTimeAsync(16);expect(editPreviewRender).not.toHaveBeenCalled();
 recipe={...recipe,advanced:{...recipe.advanced!,blackWhite:{...recipe.advanced!.blackWhite!,enabled:true}}};scheduler.wake();
 await vi.advanceTimersByTimeAsync(16);expect(editPreviewRender).toHaveBeenCalledTimes(1);
 recipe={...advancedRecipe(),advanced:{...advancedRecipe().advanced!,selectiveColor:{relative:false,ranges:{reds:[30,0,0,0]}}}};scheduler.wake();
 await vi.advanceTimersByTimeAsync(16);expect(editPreviewRender).toHaveBeenCalledTimes(2);
 scheduler.close();
});
