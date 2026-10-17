import {renderHook} from "@testing-library/react";
import {expect,it} from "vitest";
import {useCanvasZoom} from "./useCanvasZoom";

it("preserves the center when auto margins disappear and reappear at the fit boundary",()=>{
 const viewport=document.createElement("div"),frame=document.createElement("div");frame.dataset.testid="editor-canvas-frame";viewport.append(frame);
 let layout={width:500,height:300,left:50,top:50};
 Object.defineProperties(viewport,{clientWidth:{value:600},clientHeight:{value:400}});
 for(const [property,key] of Object.entries({offsetWidth:"width",offsetHeight:"height",offsetLeft:"left",offsetTop:"top"}))Object.defineProperty(frame,property,{get:()=>layout[key as keyof typeof layout]});
 const {rerender}=renderHook(({zoom})=>useCanvasZoom({current:viewport},zoom),{initialProps:{zoom:1}});
 layout={width:1000,height:600,left:0,top:0};rerender({zoom:2});
 expect(viewport.scrollLeft).toBe(200);expect(viewport.scrollTop).toBe(100);
 layout={width:500,height:300,left:50,top:50};rerender({zoom:1});
 expect(viewport.scrollLeft).toBe(0);expect(viewport.scrollTop).toBe(0);
});

it("preserves a panned position and leaves unrelated renders alone",()=>{
 const viewport=document.createElement("div"),frame=document.createElement("div");frame.dataset.testid="editor-canvas-frame";viewport.append(frame);
 let width=1000,height=600;
 Object.defineProperties(viewport,{clientWidth:{value:600},clientHeight:{value:400}});
 Object.defineProperties(frame,{offsetWidth:{get:()=>width},offsetHeight:{get:()=>height}});
 const {rerender}=renderHook(({zoom})=>useCanvasZoom({current:viewport},zoom),{initialProps:{zoom:2}});
 viewport.scrollLeft=300;viewport.scrollTop=180;
 rerender({zoom:2});expect(viewport.scrollLeft).toBe(300);expect(viewport.scrollTop).toBe(180);
 width=1500;height=900;rerender({zoom:3});
 expect(viewport.scrollLeft).toBe(600);expect(viewport.scrollTop).toBe(370);
});

it("uses the pre-resize scroll position if the browser clamps it during zoom-out",()=>{
 const viewport=document.createElement("div"),frame=document.createElement("div");frame.dataset.testid="editor-canvas-frame";viewport.append(frame);
 let width=1000,height=1000;
 Object.defineProperties(viewport,{clientWidth:{value:600},clientHeight:{value:400}});
 Object.defineProperties(frame,{offsetWidth:{get:()=>width},offsetHeight:{get:()=>height}});
 viewport.scrollLeft=350;viewport.scrollTop=550;
 const {rerender}=renderHook(({zoom})=>useCanvasZoom({current:viewport},zoom),{initialProps:{zoom:2}});
 width=800;height=800;viewport.scrollLeft=200;viewport.scrollTop=400;
 rerender({zoom:1.6});
 expect(viewport.scrollLeft).toBe(220);expect(viewport.scrollTop).toBe(400);
});
