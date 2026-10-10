import {beforeEach,expect,it,vi} from "vitest";
import {render,screen,waitFor,fireEvent} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import LutPanel from "./LutPanel";
import {editLutList,editLutImport,editLutRename,editLutRemove} from "@/ipc/api";
import {open} from "@tauri-apps/plugin-dialog";
vi.mock("@tauri-apps/plugin-dialog",()=>({open:vi.fn()}));
vi.mock("@/ipc/api",()=>({editLutList:vi.fn(),editLutImport:vi.fn(),editLutRename:vi.fn(),editLutRemove:vi.fn()}));
const builtin={id:"builtin:warm",name:"Warm",size:33,builtin:true,removed:false};
const custom={id:"a".repeat(64),name:"My LUT",size:2,builtin:false,removed:false};
const change=vi.fn(),modal=vi.fn();
function renderPanel(value?:{id:string;amount:number;enabled:boolean}){
 return render(<I18nextProvider i18n={i18n}><LutPanel value={value} onChange={change} begin={()=>{}} end={()=>{}} onModalChange={modal}/></I18nextProvider>);
}
beforeEach(()=>{vi.clearAllMocks();vi.mocked(editLutList).mockResolvedValue([builtin,custom]);vi.mocked(editLutImport).mockResolvedValue(custom);vi.mocked(editLutRename).mockResolvedValue();vi.mocked(editLutRemove).mockResolvedValue();});
it("selects a look, changes strength without adding each frame to history and resets the group",async()=>{
 renderPanel({id:custom.id,amount:50,enabled:true});
 const selector=await screen.findByRole("combobox",{name:"选择调色方案"});
 fireEvent.change(selector,{target:{value:builtin.id}});expect(change).toHaveBeenCalledWith({id:builtin.id,amount:50,enabled:true});
 fireEvent.change(screen.getByRole("slider"),{target:{value:"20"}});expect(change).toHaveBeenCalledWith({id:custom.id,amount:20,enabled:true},false);
 await userEvent.setup().click(screen.getByRole("button",{name:"重置调色方案"}));expect(change).toHaveBeenCalledWith(undefined);
});
it("imports and applies a copied resource",async()=>{
 vi.mocked(open).mockResolvedValue("C:/look.cube");renderPanel();
 await screen.findByRole("combobox");await userEvent.setup().click(screen.getByRole("button",{name:"导入方案"}));
 await waitFor(()=>expect(editLutImport).toHaveBeenCalledWith("C:/look.cube"));
 expect(change).toHaveBeenCalledWith({id:custom.id,amount:100,enabled:true});
});
it("opens rename with native canvas suspended and removes without clearing a saved reference",async()=>{
 renderPanel({id:custom.id,amount:80,enabled:true});const user=userEvent.setup();await screen.findByRole("combobox");
 await user.click(screen.getByRole("button",{name:"重命名"}));expect(modal).toHaveBeenLastCalledWith(true);
 await user.clear(screen.getByRole("textbox",{name:"方案名称"}));await user.type(screen.getByRole("textbox",{name:"方案名称"}),"Renamed");
 await user.click(screen.getByRole("button",{name:"确定"}));await waitFor(()=>expect(editLutRename).toHaveBeenCalledWith(custom.id,"Renamed"));
 await user.click(screen.getByRole("button",{name:"移除"}));await user.click(screen.getByRole("button",{name:"确定"}));
 await waitFor(()=>expect(editLutRemove).toHaveBeenCalledWith(custom.id));expect(change).not.toHaveBeenCalled();
});
