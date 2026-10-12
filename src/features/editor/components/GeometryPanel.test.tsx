import {expect,it,vi} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import GeometryPanel from "./GeometryPanel";
import {defaultRecipe} from "../lib/recipe";
import {changeGeometry} from "../lib/geometry";
import {remapDraftCrop} from "../lib/cropPixels";
it("exposes flip, angle input and a single full reset action",()=>{
 const dispatch=vi.fn();
 render(<I18nextProvider i18n={i18n}><GeometryPanel recipe={{...defaultRecipe(),geometry:{angle:15,flipHorizontal:false,flipVertical:false}}} dispatch={dispatch} context={{width:400,height:300}} begin={()=>{}} end={()=>{}}/></I18nextProvider>);
 fireEvent.click(screen.getByRole("button",{name:"水平翻转"}));expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({type:"geometry",patch:{flipHorizontal:true},record:true}));
 const angle=screen.getByRole("spinbutton",{name:"旋转角度"});fireEvent.focus(angle);fireEvent.change(angle,{target:{value:"30"}});fireEvent.blur(angle);
 expect(dispatch).toHaveBeenCalledWith(expect.objectContaining({type:"geometry",patch:{angle:30},record:true}));
 fireEvent.click(screen.getByRole("button",{name:"重置变换与裁剪"}));expect(dispatch).toHaveBeenLastCalledWith({type:"geometryReset"});
});

it("uses the initial crop draft throughout an angle gesture instead of compounding its bounds",()=>{
 const recipe={...defaultRecipe(),renderer:"photocraft" as const,crop:{x:0,y:0,w:0.8,h:0.8}},draft={x:0.3,y:0.3,w:0.2,h:0.2},context={width:6000,height:4000};
 const dispatch=vi.fn(),onCropChange=vi.fn();const props={dispatch,onCropChange,context,begin:()=>{},end:()=>{},cropRatio:1.5};
 const view=render(<I18nextProvider i18n={i18n}><GeometryPanel {...props} recipe={recipe} cropDraft={draft}/></I18nextProvider>);
 const angle=screen.getByRole("slider",{name:"旋转角度"});fireEvent.pointerDown(angle);fireEvent.change(angle,{target:{value:"10"}});
 const first=onCropChange.mock.calls[0][0];const updated=changeGeometry(recipe,{angle:10},context);
 view.rerender(<I18nextProvider i18n={i18n}><GeometryPanel {...props} recipe={updated} cropDraft={first}/></I18nextProvider>);
 fireEvent.change(screen.getByRole("slider",{name:"旋转角度"}),{target:{value:"20"}});
 expect(onCropChange.mock.calls[1][0]).toEqual(remapDraftCrop(recipe,changeGeometry(recipe,{angle:20},context),draft,context,1.5));
});
