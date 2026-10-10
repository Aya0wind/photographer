import {useReducer,useState} from "react";
import {expect,it} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import {advancedRecipe,advancedReducer,type AdvancedAction} from "../lib/advancedRecipe";
import {initRecipeHistory,type RecipeHistory} from "../lib/recipe";
import MaskPanel from "./MaskPanel";
function Harness(){
 const [history,dispatch]=useReducer((s:RecipeHistory,a:AdvancedAction)=>advancedReducer(s,a,{width:600,height:400}),initRecipeHistory(advancedRecipe()));
 const [selected,onSelect]=useState<string|null>(null),[editing,onEditing]=useState(true),[brush,onBrush]=useState({widthRel:0.05,hardness:1,opacity:1,flow:1,erase:false});
 return <I18nextProvider i18n={i18n}><MaskPanel recipe={history.present} selected={selected} onSelect={onSelect} editing={editing} onEditing={onEditing} brush={brush} onBrush={onBrush} dispatch={dispatch} begin={()=>{}} end={()=>{}} onModalChange={()=>{}}/><output data-testid="recipe">{JSON.stringify(history.present)}</output></I18nextProvider>;
}
it("creates, duplicates and edits local adjustments without changing global tone",()=>{
 render(<Harness/>);fireEvent.click(screen.getByRole("button",{name:"＋ 画笔"}));
 fireEvent.click(screen.getByRole("button",{name:"复制"}));
 let recipe=JSON.parse(screen.getByTestId("recipe").textContent!);expect(recipe.masks).toHaveLength(2);
 fireEvent.click(screen.getByRole("button",{name:"局部调色"}));
 const exposure=screen.getByRole("spinbutton",{name:/^曝光/});fireEvent.focus(exposure);fireEvent.change(exposure,{target:{value:"2"}});fireEvent.blur(exposure);
 recipe=JSON.parse(screen.getByTestId("recipe").textContent!);
 expect(recipe.masks[1].advanced.exposure).toBe(2);expect(recipe.advanced.exposure).toBe(0);expect(recipe.masks[0].advanced).toBeUndefined();
 fireEvent.click(screen.getByRole("button",{name:"重置光线"}));
 recipe=JSON.parse(screen.getByTestId("recipe").textContent!);expect(recipe.masks[1].advanced.exposure).toBe(0);
 fireEvent.click(screen.getByRole("button",{name:"删除"}));
 expect(JSON.parse(screen.getByTestId("recipe").textContent!).masks).toHaveLength(1);
});
