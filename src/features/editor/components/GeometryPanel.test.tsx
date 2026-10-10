import {expect,it,vi} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import GeometryPanel from "./GeometryPanel";
import {defaultRecipe} from "../lib/recipe";
it("exposes flip, angle input and a single full reset action",()=>{
 const dispatch=vi.fn();
 render(<I18nextProvider i18n={i18n}><GeometryPanel recipe={{...defaultRecipe(),geometry:{angle:15,flipHorizontal:false,flipVertical:false}}} dispatch={dispatch} context={{width:400,height:300}} begin={()=>{}} end={()=>{}}/></I18nextProvider>);
 fireEvent.click(screen.getByRole("button",{name:"水平翻转"}));expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({type:"geometry",patch:{flipHorizontal:true},record:true}));
 const angle=screen.getByRole("spinbutton",{name:"旋转角度"});fireEvent.focus(angle);fireEvent.change(angle,{target:{value:"30"}});fireEvent.blur(angle);
 expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({type:"geometry",patch:{angle:30},record:true}));
 fireEvent.click(screen.getByRole("button",{name:"重置变换与裁剪"}));expect(dispatch).toHaveBeenLastCalledWith({type:"geometryReset"});
});
