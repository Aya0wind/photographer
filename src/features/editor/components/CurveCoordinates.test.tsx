import {useState} from "react";
import {expect,it,vi} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import CurveCoordinates from "./CurveCoordinates";
import type {CurvePoint} from "../lib/curves";
function Harness({changed=()=>{}}:{changed?:(p:CurvePoint[])=>void}){const [points,setPoints]=useState<CurvePoint[]>([[0,0],[100,120],[160,180],[255,255]]);return <I18nextProvider i18n={i18n}><CurveCoordinates points={points} selected={1} onChange={p=>{setPoints(p);changed(p);}}/><output data-testid="points">{JSON.stringify(points)}</output></I18nextProvider>;}
function set(label:string,value:string){const input=screen.getByRole("spinbutton",{name:label});fireEvent.focus(input);fireEvent.change(input,{target:{value}});fireEvent.blur(input);}
it("clamps input before neighbouring anchors and output into the tone range",()=>{
 const changed=vi.fn();render(<Harness changed={changed}/>);set("输入","200");set("输出","300");
 expect(JSON.parse(screen.getByTestId("points").textContent!)[1]).toEqual([159,255]);expect(changed).toHaveBeenCalledTimes(2);
});
it("escape, empty and unchanged input do not generate edits",()=>{
 const changed=vi.fn();render(<Harness changed={changed}/>);const input=screen.getByRole("spinbutton",{name:"输入"});input.focus();fireEvent.change(input,{target:{value:"80"}});fireEvent.keyDown(input,{key:"Escape"});
 expect(input).toHaveValue(100);set("输入","");set("输入","100");expect(changed).not.toHaveBeenCalled();
});
it("disables numeric controls until an anchor is selected",()=>{
 render(<I18nextProvider i18n={i18n}><CurveCoordinates points={[[0,0],[255,255]]} selected={null} onChange={()=>{}}/></I18nextProvider>);
 expect(screen.getByRole("spinbutton",{name:"输入"})).toBeDisabled();expect(screen.getByRole("spinbutton",{name:"输出"})).toBeDisabled();
});
