import {expect,it,vi} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import CropDimensions from "./CropDimensions";
import {advancedRecipe} from "../lib/advancedRecipe";
import {pixelCropSize} from "../lib/cropPixels";
it("applies an entered size only on confirmation using rotated image dimensions",()=>{
 const recipe={...advancedRecipe(),rotateQuarter:1 as const},changed=vi.fn(),free=vi.fn();
 render(<I18nextProvider i18n={i18n}><CropDimensions recipe={recipe} context={{width:6000,height:4000}} crop={null} onChange={changed} onFree={free}/></I18nextProvider>);
 expect(screen.getByRole("spinbutton",{name:"宽度"})).toHaveValue(4000);expect(screen.getByRole("spinbutton",{name:"高度"})).toHaveValue(6000);
 fireEvent.change(screen.getByRole("spinbutton",{name:"宽度"}),{target:{value:"500"}});fireEvent.change(screen.getByRole("spinbutton",{name:"高度"}),{target:{value:"333"}});
 expect(changed).not.toHaveBeenCalled();fireEvent.click(screen.getByRole("button",{name:"调整裁剪框"}));
 expect(pixelCropSize(changed.mock.calls[0][0],{w:4000,h:6000})).toEqual({width:500,height:333});expect(free).toHaveBeenCalledTimes(1);
});
