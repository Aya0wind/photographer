import {act,renderHook} from "@testing-library/react";
import {it,expect,vi,beforeEach} from "vitest";
import {open,save} from "@tauri-apps/plugin-dialog";
import {editSchemeOpen,editSchemeSave} from "@/ipc/api";
import {useSchemeTransfer} from "./useSchemeTransfer";
import {advancedRecipe} from "./advancedRecipe";
vi.mock("@tauri-apps/plugin-dialog",()=>({open:vi.fn(),save:vi.fn()}));vi.mock("@/ipc/api",()=>({editSchemeOpen:vi.fn(),editSchemeSave:vi.fn()}));beforeEach(()=>vi.clearAllMocks());
function setup(){const onApply=vi.fn(),onBusy=vi.fn();return {...renderHook(({id})=>useSchemeTransfer({assetId:id,libraryId:"lib",name:"DSC_001.ARW",recipe:advancedRecipe(),label:"Scheme",disabled:false,onApply,onBusy}),{initialProps:{id:1}}),onApply,onBusy};}
it("saves the current scheme and treats a canceled file chooser as no operation",async()=>{
 vi.mocked(save).mockResolvedValueOnce("C:/out/look.photoedit").mockResolvedValueOnce(null);vi.mocked(editSchemeSave).mockResolvedValue(undefined);
 const {result,onApply,onBusy}=setup();await act(async()=>result.current.save());expect(save).toHaveBeenCalledWith(expect.objectContaining({defaultPath:"DSC_001.photoedit"}));expect(editSchemeSave).toHaveBeenCalledWith(1,"lib",advancedRecipe(),"C:/out/look.photoedit");expect(onApply).not.toHaveBeenCalled();expect(onBusy).toHaveBeenLastCalledWith(false);
 await act(async()=>result.current.save());expect(editSchemeSave).toHaveBeenCalledTimes(1);expect(result.current.error).toBeNull();
});
it("applies a checked import once and discards a result after switching photos",async()=>{
 const imported=advancedRecipe();imported.advanced!.exposure=1;vi.mocked(open).mockResolvedValue("look.photoedit");vi.mocked(editSchemeOpen).mockResolvedValueOnce(imported);
 const {result,onApply,rerender}=setup();await act(async()=>result.current.open());expect(onApply).toHaveBeenCalledWith(imported);
 let resolve!:(value:ReturnType<typeof advancedRecipe>)=>void;vi.mocked(editSchemeOpen).mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));
 let pending!:Promise<void>;await act(async()=>{pending=result.current.open();await Promise.resolve();});rerender({id:2});await act(async()=>{resolve(imported);await pending;});expect(onApply).toHaveBeenCalledTimes(1);
});
it("reports import errors and leaves existing edits intact",async()=>{
 vi.mocked(open).mockResolvedValue("look.photoedit");vi.mocked(editSchemeOpen).mockRejectedValue(Error("Invalid resource"));const {result,onApply}=setup();await act(async()=>result.current.open());expect(result.current.error).toBe("Invalid resource");expect(onApply).not.toHaveBeenCalled();act(()=>result.current.closeError());expect(result.current.error).toBeNull();
});
it("adds the scheme extension and ignores repeated actions while a chooser is open",async()=>{
 let resolve!:(path:string|null)=>void;vi.mocked(save).mockImplementationOnce(()=>new Promise(done=>{resolve=done;}));vi.mocked(editSchemeSave).mockResolvedValue(undefined);
 const {result}=setup();let pending!:Promise<void>;act(()=>{pending=result.current.save();});await act(async()=>result.current.save());expect(save).toHaveBeenCalledTimes(1);
 await act(async()=>{resolve("C:/out/look");await pending;});expect(editSchemeSave).toHaveBeenCalledWith(1,"lib",advancedRecipe(),"C:/out/look.photoedit");
});
