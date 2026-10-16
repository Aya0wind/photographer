import {expect,it} from "vitest";
import {sliderParameter,sliderPosition} from "./sliderScale";

it.each([[-5,7],[-50,100],[-100,100]])("centers zero while retaining both limits for %s to %s",(min,max)=>{
 expect(sliderPosition(0,min,max)).toBe((min+max)/2);
 for(const value of [min,min/2,0,max/2,max]){
  expect(sliderParameter(sliderPosition(value,min,max),min,max)).toBeCloseTo(value,10);
 }
 expect(sliderPosition(min,min,max)).toBe(min);
 expect(sliderPosition(max,min,max)).toBe(max);
});

it.each([[0,100],[.1,4],[50,400]])("keeps one-sided ranges linear for %s to %s",(min,max)=>{
 for(const value of [min,(min+max)/2,max]){
  expect(sliderPosition(value,min,max)).toBe(value);
  expect(sliderParameter(value,min,max)).toBe(value);
 }
});
