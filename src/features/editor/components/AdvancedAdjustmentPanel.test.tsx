import {fireEvent,render,screen,waitFor} from "@testing-library/react";
import {expect,it,vi} from "vitest";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import AdvancedAdjustmentPanel from "./AdvancedAdjustmentPanel";
import {advancedRecipe} from "../lib/advancedRecipe";
const props={recipe:advancedRecipe(),onBasic:vi.fn(),onAdvanced:vi.fn(),onReset:vi.fn(),begin:vi.fn(),end:vi.fn(),
  sampling:{picker:null,setPicker:vi.fn(),sample:null,histogram:[]}};
it("provides category resets and restores individual parameters with an undoable change",()=>{
  const recipe={...advancedRecipe(),advanced:{...advancedRecipe().advanced!,exposure:2}};
  render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel {...props} recipe={recipe}/></I18nextProvider>);
  fireEvent.click(screen.getByRole("button",{name:"重置光线"}));
  expect(props.onReset).toHaveBeenCalledWith("light");
  fireEvent.click(screen.getByRole("button",{name:/^重置曝光/}));
  expect(props.onAdvanced).toHaveBeenCalledWith({exposure:0});
});
it("HSL picker selects the sampled color range and offers a clear picker action",async()=>{
  render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel {...props}
    sampling={{...props.sampling,picker:"hsl",sample:[0,0,255],hslRange:"blues"}}/></I18nextProvider>);
  await waitFor(()=>expect(screen.getByLabelText("颜色范围")).toHaveValue("blues"));
  fireEvent.click(screen.getByRole("button",{name:"从照片选取颜色"}));
  expect(props.sampling.setPicker).toHaveBeenCalledWith(null);
});

it("accepts numeric input on blur, clamps to range and rejects empty values",()=>{
  props.onAdvanced.mockClear();
  render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel {...props}/></I18nextProvider>);
  const exposure=screen.getByRole("spinbutton",{name:/曝光/});
  fireEvent.focus(exposure);fireEvent.change(exposure,{target:{value:"9"}});fireEvent.blur(exposure);
  expect(props.onAdvanced).toHaveBeenLastCalledWith({exposure:5});
  props.onAdvanced.mockClear();
  fireEvent.focus(exposure);fireEvent.change(exposure,{target:{value:""}});fireEvent.blur(exposure);
  expect(props.onAdvanced).not.toHaveBeenCalled();
});
