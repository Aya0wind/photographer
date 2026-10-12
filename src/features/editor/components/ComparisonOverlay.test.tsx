import {useState} from "react";
import {expect,it,vi,beforeEach,afterEach} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import ComparisonOverlay from "./ComparisonOverlay";
beforeEach(()=>vi.stubGlobal("PointerEvent",class extends MouseEvent{pointerId:number;constructor(type:string,o:PointerEventInit={}){super(type,o);this.pointerId=o.pointerId??1;}}));afterEach(()=>vi.unstubAllGlobals());
function setup(){const parent=vi.fn();function Harness(){const [value,setValue]=useState(0.5);return <I18nextProvider i18n={i18n}><div onPointerDown={parent}><ComparisonOverlay position={value} onChange={setValue}/></div></I18nextProvider>;}render(<Harness/>);const slider=screen.getByRole("slider");slider.setPointerCapture=vi.fn();slider.parentElement!.getBoundingClientRect=()=>new DOMRect(100,0,200,300);return {slider,parent};}
it("drags within photo coordinates without panning, clamps endpoints and releases capture",()=>{
 const {slider,parent}=setup();fireEvent.pointerDown(slider,{button:0,pointerId:2,clientX:200});expect(parent).not.toHaveBeenCalled();
 fireEvent.pointerMove(slider,{pointerId:3,clientX:300});expect(slider).toHaveAttribute("aria-valuenow","50");
 fireEvent.pointerMove(slider,{pointerId:2,clientX:240});expect(slider).toHaveAttribute("aria-valuenow","70");
 fireEvent.pointerMove(slider,{pointerId:2,clientX:20});expect(slider).toHaveAttribute("aria-valuenow","0");fireEvent.pointerMove(slider,{pointerId:2,clientX:500});expect(slider).toHaveAttribute("aria-valuenow","100");
 fireEvent.pointerUp(slider,{pointerId:2});fireEvent.pointerMove(slider,{pointerId:2,clientX:200});expect(slider).toHaveAttribute("aria-valuenow","100");
});
it("supports keyboard steps and does not continue after cancellation",()=>{
 const {slider}=setup();fireEvent.keyDown(slider,{key:"ArrowRight"});expect(slider).toHaveAttribute("aria-valuenow","51");fireEvent.keyDown(slider,{key:"ArrowLeft",shiftKey:true});expect(slider).toHaveAttribute("aria-valuenow","41");
 fireEvent.keyDown(slider,{key:"End"});expect(slider).toHaveAttribute("aria-valuenow","100");
 fireEvent.pointerDown(slider,{button:0,clientX:200});fireEvent.pointerCancel(slider);fireEvent.pointerMove(slider,{clientX:200});expect(slider).toHaveAttribute("aria-valuenow","100");
 fireEvent.keyDown(slider,{key:"Home"});expect(slider).toHaveAttribute("aria-valuenow","0");
});
