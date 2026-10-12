import {act,renderHook} from "@testing-library/react";
import {expect,it,vi,beforeEach,afterEach} from "vitest";
import {editPreviewClipping} from "@/ipc/api";
import {advancedRecipe} from "./advancedRecipe";
import {useClippingOverlay} from "./useClippingOverlay";
vi.mock("@/ipc/api",()=>({editPreviewClipping:vi.fn()}));
beforeEach(()=>{vi.useFakeTimers();vi.clearAllMocks();vi.stubGlobal("Image",class{src="";decode=()=>Promise.resolve();});vi.stubGlobal("URL",{revokeObjectURL:vi.fn()});});afterEach(()=>{vi.useRealTimers();vi.unstubAllGlobals();});
const recipe=advancedRecipe();const error=vi.fn();
async function tick(){await act(async()=>{await vi.advanceTimersByTimeAsync(48);});}
it("does no work while disabled and revokes replaced or abandoned overlays",async()=>{
 let old!:(url:string)=>void;vi.mocked(editPreviewClipping).mockImplementationOnce(()=>new Promise(done=>{old=done;})).mockResolvedValueOnce("blob:new");
 const {result,rerender,unmount}=renderHook(({enabled,value})=>useClippingOverlay("s",value,false,3,enabled,error),{initialProps:{enabled:false,value:recipe}});
 await tick();expect(editPreviewClipping).not.toHaveBeenCalled();rerender({enabled:true,value:recipe});await tick();
 rerender({enabled:true,value:{...recipe,advanced:{...recipe.advanced!,exposure:1}}});await tick();expect(result.current).toBe("blob:new");
 await act(async()=>old("blob:old"));expect(result.current).toBe("blob:new");expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:old");
 unmount();expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:new");
});
it("reuses original warnings during color edits and reports only current errors",async()=>{
 vi.mocked(editPreviewClipping).mockResolvedValueOnce("blob:original").mockRejectedValueOnce(Error("warning failed"));
 const {result,rerender}=renderHook(({value})=>useClippingOverlay("s",value,true,1,true,error),{initialProps:{value:recipe}});await tick();expect(result.current).toBe("blob:original");
 rerender({value:{...recipe,advanced:{...recipe.advanced!,exposure:3}}});await tick();expect(editPreviewClipping).toHaveBeenCalledTimes(1);
 rerender({value:{...recipe,rotateQuarter:1}});await tick();expect(error).toHaveBeenLastCalledWith(expect.objectContaining({message:"warning failed"}));expect(result.current).toBeNull();
});
