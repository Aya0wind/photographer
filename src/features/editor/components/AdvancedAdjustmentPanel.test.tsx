import {expect,it,vi} from "vitest";
import {render,screen,within,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import AdvancedAdjustmentPanel from "./AdvancedAdjustmentPanel";
import {advancedRecipe,DEFAULT_DEVELOPMENT} from "../lib/advancedRecipe";
vi.mock("./LutPanel",()=>({default:()=>null}));
vi.mock("./CurveEditor",()=>({default:()=>null}));
vi.mock("./ParametricCurvePanel",()=>({default:()=>null}));
it("embeds lens correction as a collapsed adjustment section and reports its active state",()=>{
 const open=vi.fn(),end=vi.fn();
 render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel lensPanel={<div>Lens controls</div>} onLensOpenChange={open} recipe={advancedRecipe()} onBasic={vi.fn()} onAdvanced={vi.fn()} onReset={vi.fn()} begin={vi.fn()} end={end} sampling={{picker:null,setPicker:vi.fn(),sample:null,histogram:[],available:false}}/></I18nextProvider>);
 const section=screen.getByText(i18n.t("advancedEditor.lens.title")).closest("details")!;
 expect(section.open).toBe(false);
 section.open=true;fireEvent(section,new Event("toggle"));expect(open).toHaveBeenLastCalledWith(true);
 section.open=false;fireEvent(section,new Event("toggle"));expect(open).toHaveBeenLastCalledWith(false);expect(end).toHaveBeenCalled();
});
it("selects HSL colours on the gradient slider and edits only that range",()=>{
 const change=vi.fn();
 render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel recipe={advancedRecipe()} onBasic={vi.fn()} onAdvanced={change} onReset={vi.fn()} begin={vi.fn()} end={vi.fn()} sampling={{picker:null,setPicker:vi.fn(),sample:null,histogram:[],available:false}}/></I18nextProvider>);
 const group=screen.getByText("HSL").closest("details")!;group.open=true;
 expect(within(group).queryByRole("combobox")).not.toBeInTheDocument();
 const colour=within(group).getByRole("slider",{name:"颜色范围"});fireEvent.change(colour,{target:{value:"5"}});
 expect(colour).toHaveAttribute("aria-valuetext","蓝色");
 const saturation=within(group).getByRole("slider",{name:"饱和度"});fireEvent.change(saturation,{target:{value:"25"}});
 expect(change).toHaveBeenCalledWith({hslEight:{blues:{hue:0,saturation:25,lightness:0}}},false);
 expect(saturation).toHaveClass("editor-range");
 expect(saturation.parentElement?.style.getPropertyValue("--slider-gradient")).toContain("linear-gradient");
});
it("shows a sampled color readout beside picker controls",()=>{
 render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel recipe={advancedRecipe()} onBasic={vi.fn()} onAdvanced={vi.fn()} onReset={vi.fn()} begin={vi.fn()} end={vi.fn()} sampling={{picker:"gray",setPicker:vi.fn(),sample:[64,128,192],histogram:[]}}/></I18nextProvider>);
 expect(screen.getByRole("status")).toHaveTextContent("取样颜色");expect(screen.getByRole("status")).toHaveTextContent("RGB 64.0 · 128.0 · 192.0");
});
it("offers a shared sample area for HSL and curve pickers",()=>{
 const sampleSize=vi.fn();
 render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel recipe={advancedRecipe()} onBasic={vi.fn()} onAdvanced={vi.fn()} onReset={vi.fn()} begin={vi.fn()} end={vi.fn()} sampling={{picker:"hsl",setPicker:vi.fn(),sample:null,histogram:[],sampleSize:3,setSampleSize:sampleSize}}/></I18nextProvider>);
 expect(screen.getByRole("combobox",{name:"取色范围"})).toHaveValue("3");
 fireEvent.change(screen.getByRole("combobox",{name:"取色范围"}),{target:{value:"11"}});expect(sampleSize).toHaveBeenCalledWith(11);
});
it("places tone, effects and detail controls in their Camera Raw groups",()=>{
 const recipe=advancedRecipe();recipe.advanced!.development={...DEFAULT_DEVELOPMENT,shadows:20};const reset=vi.fn();
 render(<I18nextProvider i18n={i18n}><AdvancedAdjustmentPanel recipe={recipe} onBasic={vi.fn()} onAdvanced={vi.fn()} onReset={reset} begin={vi.fn()} end={vi.fn()} sampling={{picker:null,setPicker:vi.fn(),sample:null,histogram:[],available:false}}/></I18nextProvider>);
 const tone=screen.getByRole("heading",{name:"影调"}).closest("details")!;
 for(const name of ["曝光（EV）","阴影","高光","白场","黑场"])expect(within(tone).getByRole("spinbutton",{name})).toBeInTheDocument();
 fireEvent.click(within(tone).getByRole("button",{name:"重置影调"}));expect(reset).toHaveBeenCalledWith("light");
 const effects=screen.getByText("效果",{selector:"summary > span"}).closest("details")!;effects.open=true;
 for(const name of ["纹理","清晰度","去朦胧","颗粒数量"])expect(within(effects).getByRole("spinbutton",{name})).toBeInTheDocument();
 const detail=screen.getByText("细节",{selector:"summary > span"}).closest("details")!;detail.open=true;
 expect(within(detail).getByRole("spinbutton",{name:"锐化半径"})).toBeInTheDocument();
 expect(screen.queryByText("阴影与高光")).not.toBeInTheDocument();
});
