import {it,expect,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import HistogramPanel from "./HistogramPanel";
it("shows an updating state and stops histogram requests when collapsed or removed",()=>{
 const onOpen=vi.fn();const view=render(<I18nextProvider i18n={i18n}><HistogramPanel bins={Array.from({length:4},()=>Array(256).fill(0))} busy onOpen={onOpen}/></I18nextProvider>);
 const details=view.container.querySelector("details")!;details.open=true;fireEvent(details,new Event("toggle"));expect(onOpen).toHaveBeenCalledWith(true);expect(screen.getByRole("status")).toHaveAccessibleName("正在更新直方图");
 expect(view.container.querySelectorAll("path")).toHaveLength(3);expect(view.container.innerHTML).not.toContain("NaN");
 details.open=false;fireEvent(details,new Event("toggle"));expect(onOpen).toHaveBeenLastCalledWith(false);view.unmount();expect(onOpen).toHaveBeenLastCalledWith(false);
});
