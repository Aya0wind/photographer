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

it("keeps the displayed frame while rendering and drops outdated results", async () => {
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
  expect(releasePreviewImage).toHaveBeenCalledWith("stale");
  expect(callbacks.setUrl).toHaveBeenCalledTimes(1);
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
