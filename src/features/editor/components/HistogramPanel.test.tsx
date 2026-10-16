import {it,expect,vi} from "vitest";
import {fireEvent,render,screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import HistogramPanel from "./HistogramPanel";
it("opens by default without a loading animation and pauses when collapsed",()=>{
 const onOpen=vi.fn();const view=render(<I18nextProvider i18n={i18n}><HistogramPanel bins={Array.from({length:4},()=>Array(256).fill(0))} busy onOpen={onOpen}/></I18nextProvider>);
 const details=view.container.querySelector("details")!;expect(details.open).toBe(true);expect(onOpen).toHaveBeenCalledWith(true);expect(screen.queryByRole("status")).not.toBeInTheDocument();
 expect(view.container.querySelectorAll("path")).toHaveLength(3);expect(view.container.innerHTML).not.toContain("NaN");
 for(const path of view.container.querySelectorAll("path"))expect(path.getAttribute("d")).toContain(" C");
 details.open=false;fireEvent(details,new Event("toggle"));expect(onOpen).toHaveBeenLastCalledWith(false);view.unmount();expect(onOpen).toHaveBeenLastCalledWith(false);
});
