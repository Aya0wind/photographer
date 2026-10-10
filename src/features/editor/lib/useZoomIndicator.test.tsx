import {act,renderHook} from "@testing-library/react";
import {beforeEach,afterEach,expect,it,vi} from "vitest";
import {useZoomIndicator} from "./useZoomIndicator";
beforeEach(()=>vi.useFakeTimers());afterEach(()=>vi.useRealTimers());
it("hides three seconds after the last zoom and wakes on another zoom",()=>{
 const {result,rerender}=renderHook(({zoom})=>useZoomIndicator(zoom),{initialProps:{zoom:1}});
 act(()=>vi.advanceTimersByTime(2999));expect(result.current.visible).toBe(true);
 act(()=>vi.advanceTimersByTime(1));expect(result.current.visible).toBe(false);
 rerender({zoom:2});expect(result.current.visible).toBe(true);
 act(()=>vi.advanceTimersByTime(2000));rerender({zoom:3});
 act(()=>vi.advanceTimersByTime(2999));expect(result.current.visible).toBe(true);
 act(()=>vi.advanceTimersByTime(1));expect(result.current.visible).toBe(false);
});
