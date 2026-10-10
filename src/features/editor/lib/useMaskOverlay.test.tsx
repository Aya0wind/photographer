import {act,renderHook,waitFor} from "@testing-library/react";
import {afterEach,beforeEach,expect,it,vi} from "vitest";
import {editPreviewMask} from "@/ipc/api";
import {useMaskOverlay} from "./useMaskOverlay";
import {advancedRecipe} from "./advancedRecipe";
vi.mock("@/ipc/api",()=>({editPreviewMask:vi.fn()}));
const revoke=vi.fn();
beforeEach(()=>{vi.clearAllMocks();vi.stubGlobal("URL",{revokeObjectURL:revoke});vi.stubGlobal("Image",class {src="";decode(){return Promise.resolve();}});});
afterEach(()=>vi.unstubAllGlobals());
function photo(){const recipe=advancedRecipe();recipe.masks=[{id:"m",name:"Area",kind:"linear" as const,enabled:true,inverted:false,density:100,feather:0,from:{x:0.2,y:0.5},to:{x:0.8,y:0.5},strokes:[]}];return recipe;}
it("reuses overlay during local color changes and releases URLs on shape replacement/unmount",async()=>{
 vi.mocked(editPreviewMask).mockResolvedValueOnce("blob:first").mockResolvedValueOnce("blob:second");
 const recipe=photo();const {result,rerender,unmount}=renderHook(({recipe})=>useMaskOverlay("s",recipe,"m",true,vi.fn()),{initialProps:{recipe}});
 await waitFor(()=>expect(result.current).toBe("blob:first"));
 const adjusted={...recipe,masks:[{...recipe.masks![0],adjustments:{brightness:30,contrast:0,saturation:0}}]};rerender({recipe:adjusted});
 expect(editPreviewMask).toHaveBeenCalledTimes(1);
 rerender({recipe:{...adjusted,masks:[{...adjusted.masks[0],inverted:true}]}});
 await waitFor(()=>expect(result.current).toBe("blob:second"));expect(revoke).toHaveBeenCalledWith("blob:first");
 unmount();expect(revoke).toHaveBeenCalledWith("blob:second");
});
it("discards and releases outdated overlay responses",async()=>{
 let resolve!:(url:string)=>void;vi.mocked(editPreviewMask).mockImplementationOnce(()=>new Promise(r=>{resolve=r;})).mockResolvedValueOnce("blob:new");
 const recipe=photo();const {result,rerender}=renderHook(({recipe})=>useMaskOverlay("s",recipe,"m",true,vi.fn()),{initialProps:{recipe}});
 rerender({recipe:{...recipe,masks:[{...recipe.masks![0],feather:5}]}});
 await waitFor(()=>expect(result.current).toBe("blob:new"));
 await act(async()=>{resolve("blob:old");});
 expect(result.current).toBe("blob:new");expect(revoke).toHaveBeenCalledWith("blob:old");
});
