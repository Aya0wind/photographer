import {useState} from "react";
import {expect,it} from "vitest";
import {fireEvent,render,screen,within} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import type {AdvancedAdjustments} from "@/ipc/api";
import ExtendedColorPanel from "./ExtendedColorPanel";
import {DEFAULT_ADVANCED} from "../lib/advancedRecipe";
function Harness(){const [a,set]=useState<AdvancedAdjustments>({...DEFAULT_ADVANCED});return <I18nextProvider i18n={i18n}><ExtendedColorPanel adjustments={a} onChange={patch=>set({...a,...patch})} onReset={group=>set({...a,[group]:undefined})} begin={()=>{}} end={()=>{}}/><output data-testid="params">{JSON.stringify(a)}</output></I18nextProvider>;}
function setup(){const view=render(<Harness/>);view.container.querySelectorAll("details").forEach(el=>{el.open=true;});return ()=>JSON.parse(screen.getByTestId("params").textContent!);}
function number(label:string,value:string,group?:string){const input=(group?within(screen.getByRole("group",{name:group})):screen).getByRole("spinbutton",{name:label});fireEvent.focus(input);fireEvent.change(input,{target:{value}});fireEvent.blur(input);}
it("edits one color-balance tone and offers current-tone and whole-group reset",()=>{
 const params=setup();number("青色 ↔ 红色","25");expect(params().colorBalance.midtones).toEqual([25,0,0]);
 fireEvent.change(screen.getByLabelText("色调范围"),{target:{value:"shadows"}});number("黄色 ↔ 蓝色","-10");
 expect(params().colorBalance.shadows).toEqual([0,0,-10]);
 fireEvent.click(screen.getByRole("button",{name:"重置当前色调"}));expect(params().colorBalance.midtones).toEqual([25,0,0]);expect(params().colorBalance.shadows).toEqual([0,0,0]);
 fireEvent.click(screen.getByRole("button",{name:"重置颜色平衡"}));expect(params().colorBalance).toBeUndefined();
});
it("enables black-and-white weights with upstream defaults and optional tint",()=>{
 const params=setup();const red=screen.getByRole("spinbutton",{name:"红色"});expect(red).toBeDisabled();
 fireEvent.click(screen.getByLabelText("启用黑白"));expect(red).toBeEnabled();expect(params().blackWhite.weights).toEqual([40,60,40,60,20,80]);
 number("红色","90");fireEvent.click(screen.getByLabelText("着色"));expect(params().blackWhite.tint).toBe("#E1D3B3");
 fireEvent.click(screen.getByRole("button",{name:"重置红色"}));expect(params().blackWhite.weights[0]).toBe(40);
 fireEvent.click(screen.getByRole("button",{name:"重置黑白"}));expect(params().blackWhite).toBeUndefined();
});
it("keeps selective-color ranges independent and resets only the active range",()=>{
 const params=setup();number("青色","20","可选颜色");
 fireEvent.change(screen.getByLabelText("可选颜色 · 颜色范围"),{target:{value:"blacks"}});number("黑色","-15","可选颜色");
 fireEvent.change(screen.getByLabelText("计算方式"),{target:{value:"absolute"}});
 expect(params().selectiveColor).toEqual({relative:false,ranges:{reds:[20,0,0,0],blacks:[0,0,0,-15]}});
 fireEvent.click(screen.getByRole("button",{name:"重置当前颜色"}));expect(params().selectiveColor.ranges).toEqual({reds:[20,0,0,0]});
});
