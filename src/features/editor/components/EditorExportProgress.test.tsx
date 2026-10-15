import {render,screen,waitFor} from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import {beforeEach,expect,it,vi} from "vitest";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import {editExportCancel,appForceExit} from "@/ipc/api";
import EditorExportProgress from "./EditorExportProgress";
vi.mock("@/ipc/api",()=>({editExportCancel:vi.fn(),appForceExit:vi.fn()}));
beforeEach(()=>{vi.clearAllMocks();vi.mocked(editExportCancel).mockResolvedValue();vi.mocked(appForceExit).mockResolvedValue();});
function view(phase="film",onError=vi.fn()) {return render(<I18nextProvider i18n={i18n}><EditorExportProgress assetId={1} libraryId="photos" jobId={7} phase={phase} onError={onError}/></I18nextProvider>);}
it("shows stage progress and keeps cancellation pending until task completion",async()=>{
 const user=userEvent.setup();view();
 expect(screen.getByRole("progressbar")).toHaveAttribute("value","40");
 await user.click(screen.getByRole("button",{name:"取消导出"}));
 expect(editExportCancel).toHaveBeenCalledWith(1,"photos",7);
 expect(screen.getByRole("button",{name:"取消导出"})).toBeDisabled();
 expect(screen.getByRole("status")).toHaveTextContent("正在取消");
});
it("allows retry when cancellation fails and requires explicit force-quit confirmation",async()=>{
 const user=userEvent.setup(),onError=vi.fn();vi.mocked(editExportCancel).mockRejectedValue(new Error("offline"));view("decode",onError);
 await user.click(screen.getByRole("button",{name:"取消导出"}));
 await waitFor(()=>expect(onError).toHaveBeenCalled());expect(screen.getByRole("button",{name:"取消导出"})).toBeEnabled();
 await user.click(screen.getByRole("button",{name:"强制退出"}));expect(appForceExit).not.toHaveBeenCalled();
 expect(screen.getByRole("alertdialog")).toHaveTextContent("未保存的编辑");
 await user.click(screen.getByRole("button",{name:"确认强制退出"}));expect(appForceExit).toHaveBeenCalledOnce();
});
it("protects the short publication/registration step from cancellation",()=>{view("register");expect(screen.getByRole("button",{name:"取消导出"})).toBeDisabled();expect(screen.getByRole("button",{name:"强制退出"})).toBeEnabled();});
