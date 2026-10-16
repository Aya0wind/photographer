import {useState} from "react";
import {it,expect,vi} from "vitest";
import {render,screen,fireEvent} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import AdvancedToolPanel from "./AdvancedToolPanel";
import type {ExportOptions} from "@/ipc/api";
import {advancedRecipe} from "../lib/advancedRecipe";
it("selects lossless formats and hides JPEG quality for PNG and TIFF",()=>{
 const noop=vi.fn();function Harness(){const [format,setFormat]=useState<NonNullable<ExportOptions["format"]>>("jpeg");const [space,setSpace]=useState<NonNullable<ExportOptions["colorSpace"]>>("srgb");return <I18nextProvider i18n={i18n}><AdvancedToolPanel onCancelCrop={noop} straightening={false} setStraightening={noop} sampling={{picker:null,setPicker:noop,sample:null,histogram:[]}} tool="output" recipe={advancedRecipe()} dispatch={noop} beginGesture={noop} endGesture={noop} context={{current:{width:600,height:400}}} setCropDraft={noop} cropDraft={null} setTool={noop} cropRatio={null} setCropRatio={noop} color="#ffffff" setColor={noop} selectedText={null} setSelectedText={noop} brushWidth={0.1} setBrushWidth={noop} exporting={false} exportFolder={noop} exportAlbum={noop} exportFormat={format} setExportFormat={setFormat} exportColorSpace={space} setExportColorSpace={setSpace}/></I18nextProvider>;}
 render(<Harness/>);const selector=screen.getByRole("combobox",{name:"导出格式"});expect(selector).toHaveValue("jpeg");const color=screen.getByRole("combobox",{name:"色彩空间"});expect(color).toHaveValue("srgb");fireEvent.change(color,{target:{value:"displayP3"}});expect(color).toHaveValue("displayP3");expect(screen.getByRole("slider")).toBeInTheDocument();
 for(const format of ["png","tiff"]){fireEvent.change(selector,{target:{value:format}});expect(selector).toHaveValue(format);expect(screen.queryByRole("slider")).not.toBeInTheDocument();}
 fireEvent.change(selector,{target:{value:"pcraft"}});expect(selector).toHaveValue("pcraft");expect(screen.queryByRole("combobox",{name:"色彩空间"})).not.toBeInTheDocument();expect(screen.queryByRole("slider")).not.toBeInTheDocument();expect(screen.getByText(/保存当前图层和蒙版/)).toBeInTheDocument();
});
