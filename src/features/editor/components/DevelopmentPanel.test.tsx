import {useReducer} from "react";
import {expect,it} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import DevelopmentPanel from "./DevelopmentPanel";
import {advancedRecipe,advancedReducer,type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory,type RecipeHistory} from "../lib/recipe";
function Harness(){const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initRecipeHistory(advancedRecipe()));return <I18nextProvider i18n={i18n}><DevelopmentPanel adjustments={history.present.advanced!} onChange={(patch,record)=>dispatch({type:"advanced",patch,record})} onReset={group=>dispatch({type:"resetDevelopmentGroup",group})} begin={()=>{}} end={()=>{}}/><button onClick={()=>dispatch({type:"undo"})}>Undo</button><output data-testid="recipe">{JSON.stringify(history.present)}</output></I18nextProvider>;}
function setup(){const view=render(<Harness/>);view.container.querySelectorAll("details").forEach(d=>d.open=true);return ()=>JSON.parse(screen.getByTestId("recipe").textContent!).advanced.development;}
function number(label:string,value:string){const input=screen.getByRole("spinbutton",{name:label});fireEvent.focus(input);fireEvent.change(input,{target:{value}});fireEvent.blur(input);}
it("uses native radius/detail defaults and preserves other groups when resetting",()=>{
 const value=setup();expect(screen.getByRole("spinbutton",{name:"锐化半径"})).toHaveValue(1);expect(screen.getByRole("spinbutton",{name:"锐化细节"})).toHaveValue(25);
 number("阴影","30");number("清晰度","20");number("锐化半径","2");
 fireEvent.click(screen.getByRole("button",{name:"重置锐化与降噪"}));expect(value().sharpenRadius).toBe(1);expect(value().shadows).toBe(30);expect(value().clarity).toBe(20);
 fireEvent.click(screen.getByRole("button",{name:"Undo"}));expect(value().sharpenRadius).toBe(2);
});
it("supports vignette style, validated numeric values and individual reset",()=>{
 const value=setup();number("颗粒数量","200");expect(value().grainAmount).toBe(100);
 fireEvent.change(screen.getByLabelText("暗角样式"),{target:{value:"colorPriority"}});expect(value().vignetteStyle).toBe("colorPriority");
 fireEvent.click(screen.getByRole("button",{name:"重置颗粒数量"}));expect(value().grainAmount).toBe(0);expect(value().vignetteStyle).toBe("colorPriority");
 fireEvent.click(screen.getByRole("button",{name:"重置颗粒与暗角"}));expect(value().vignetteStyle).toBe("highlightPriority");
});
