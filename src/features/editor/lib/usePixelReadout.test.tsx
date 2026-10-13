import {act,renderHook} from "@testing-library/react";
import {afterEach,expect,it,vi} from "vitest";
import {editPreviewPick} from "@/ipc/api";
import {rgbToLab,usePixelReadout} from "./usePixelReadout";
import {advancedRecipe} from "./advancedRecipe";
vi.mock("@/ipc/api",()=>({editPreviewPick:vi.fn()}));
afterEach(()=>{vi.useRealTimers();vi.clearAllMocks();});
it("converts known sRGB values to D65 Lab",()=>{
 expect(rgbToLab([0,0,0])).toEqual([0,0,0]);
 const white=rgbToLab([255,255,255]);expect(white[0]).toBeCloseTo(100,3);expect(white[1]).toBeCloseTo(0,3);
 const red=rgbToLab([255,0,0]);expect(red[0]).toBeCloseTo(53.24,1);expect(red[1]).toBeCloseTo(80.09,1);
});
it("coalesces moves and never restores a readout after leaving the image",async()=>{
 vi.useFakeTimers();let resolve!:(value:{sample:[number,number,number]})=>void;
 vi.mocked(editPreviewPick).mockReturnValue(new Promise(r=>{resolve=r;}));
 const {result,unmount}=renderHook(()=>usePixelReadout("s",advancedRecipe(),true));
 act(()=>{result.current.hover([.1,.2]);result.current.hover([.8,.6]);});
 await act(()=>vi.advanceTimersByTimeAsync(80));
 expect(editPreviewPick).toHaveBeenCalledTimes(1);expect(editPreviewPick).toHaveBeenCalledWith("s",expect.anything(),.8,.6,"readout");
 act(()=>result.current.hover(null));await act(async()=>{resolve({sample:[255,0,0]});});
 expect(result.current.rgb).toBeNull();unmount();
});
