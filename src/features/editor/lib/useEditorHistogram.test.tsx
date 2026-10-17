import {act,renderHook} from "@testing-library/react";
import {it,expect,vi,beforeEach,afterEach} from "vitest";
import {editPreviewHistogram} from "@/ipc/api";
import {advancedRecipe} from "./advancedRecipe";
import {startHistogramScheduler,useEditorHistogram} from "./useEditorHistogram";
vi.mock("@/ipc/api",()=>({editPreviewHistogram:vi.fn()}));beforeEach(()=>{vi.useFakeTimers();vi.clearAllMocks();});afterEach(()=>vi.useRealTimers());
const recipe=advancedRecipe();async function tick(){await act(async()=>{await vi.advanceTimersByTimeAsync(200);});}
it("keeps the displayed histogram while interaction pauses auxiliary work and resumes with the latest recipe",async()=>{
 vi.mocked(editPreviewHistogram).mockResolvedValue([[1]]);
 const {result,rerender}=renderHook(({paused,value})=>useEditorHistogram("s",value,false,true,paused),{initialProps:{paused:false,value:recipe}});
 await tick();expect(result.current.bins).toEqual([[1]]);expect(editPreviewHistogram).toHaveBeenCalledTimes(1);
 const changed={...recipe,advanced:{...recipe.advanced!,exposure:1}};
 rerender({paused:true,value:changed});await tick();expect(editPreviewHistogram).toHaveBeenCalledTimes(1);expect(result.current.bins).toEqual([[1]]);
 vi.mocked(editPreviewHistogram).mockResolvedValue([[2]]);rerender({paused:false,value:changed});await tick();
 expect(editPreviewHistogram).toHaveBeenLastCalledWith("s",changed,false);expect(result.current.bins).toEqual([[2]]);
});
it("coalesces repeated changes and keeps only one request in flight",async()=>{
 let resolve!:(bins:number[][])=>void;vi.mocked(editPreviewHistogram).mockImplementationOnce(()=>new Promise(done=>{resolve=done;})).mockResolvedValueOnce([[2]]);
 let value={recipe,original:false};const output={result:vi.fn(),busy:vi.fn(),error:vi.fn()};const worker=startHistogramScheduler("s",()=>value,output);
 worker.wake();worker.wake();await tick();expect(editPreviewHistogram).toHaveBeenCalledTimes(1);
 value={recipe:{...recipe,advanced:{...recipe.advanced!,exposure:1}},original:false};worker.wake();await tick();worker.wake();await tick();expect(editPreviewHistogram).toHaveBeenCalledTimes(1);
 await act(async()=>resolve([[1]]));await tick();expect(output.result).toHaveBeenCalledTimes(1);expect(output.result).toHaveBeenCalledWith([[2]]);expect(editPreviewHistogram).toHaveBeenLastCalledWith("s",value.recipe,false);worker.close();
});
it("does no work while hidden and reuses its queue across rapid close/open changes",async()=>{
 let resolve!:(bins:number[][])=>void;vi.mocked(editPreviewHistogram).mockImplementationOnce(()=>new Promise(done=>{resolve=done;})).mockResolvedValueOnce([[3]]);
 const {result,rerender}=renderHook(({enabled,original})=>useEditorHistogram("s",recipe,original,enabled),{initialProps:{enabled:false,original:false}});
 await tick();expect(editPreviewHistogram).not.toHaveBeenCalled();rerender({enabled:true,original:false});await tick();rerender({enabled:false,original:false});rerender({enabled:true,original:true});await tick();expect(editPreviewHistogram).toHaveBeenCalledTimes(1);
 await act(async()=>resolve([[1]]));await tick();expect(result.current.bins).toEqual([[3]]);expect(editPreviewHistogram).toHaveBeenLastCalledWith("s",recipe,true);
});
it("rejects errors and results from a closed session",async()=>{
 let reject!:(error:Error)=>void;vi.mocked(editPreviewHistogram).mockImplementationOnce(()=>new Promise((_resolve,fail)=>{reject=fail;}));const output={result:vi.fn(),busy:vi.fn(),error:vi.fn()};const worker=startHistogramScheduler("s",()=>({recipe,original:false}),output);worker.wake();await tick();worker.close();await act(async()=>reject(Error("old session")));expect(output.error).not.toHaveBeenCalled();expect(output.result).not.toHaveBeenCalled();
});
