import {expect,it} from "vitest";
import {histogramAreaPath} from "./histogramPath";

it("draws smooth cubic segments across the complete histogram without mutating bins",()=>{
 const bins=[0,2,12,40,12,2,0],original=[...bins];
 const path=histogramAreaPath(bins,40);
 expect(path).toMatch(/^M0 100 L0 /);
 expect(path.match(/ C/g)).toHaveLength(6);
 expect(path).toMatch(/ L255 100 Z$/);
 expect(bins).toEqual(original);
});

it("keeps every control point within adjacent heights, including sharp peaks and clipping edges",()=>{
 const path=histogramAreaPath([100,0,0,100,0,50,100,0],100);
 let previous=Number(path.match(/^M0 100 L0 ([\d.]+)/)![1]);
 for(const match of path.matchAll(/ C([\d.]+) ([\d.]+) ([\d.]+) ([\d.]+) ([\d.]+) ([\d.]+)/g)){
  const end=Number(match[6]),low=Math.min(previous,end),high=Math.max(previous,end);
  for(const y of [Number(match[2]),Number(match[4]),end]){
   expect(y).toBeGreaterThanOrEqual(low-.001);
   expect(y).toBeLessThanOrEqual(high+.001);
   expect(y).toBeGreaterThanOrEqual(4);
   expect(y).toBeLessThanOrEqual(100);
  }
  previous=end;
 }
});

it("handles empty and invalid data and keeps an empty channel flat",()=>{
 expect(histogramAreaPath([],1)).toBe("");
 expect(histogramAreaPath([0,0,0],0)).toBe("M0 100 L0 100 C42.5 100 85 100 127.5 100 C170 100 212.5 100 255 100 L255 100 Z");
 expect(histogramAreaPath([NaN,-5,Infinity],NaN)).not.toMatch(/NaN|Infinity/);
});
