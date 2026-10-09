import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import "@/i18n";
import DateRangePicker from "./DateRangePicker";

it("选择和快捷范围只改草稿，确定一次才应用",()=>{
  const apply=vi.fn();
  render(<DateRangePicker from="" to="" onApply={apply} ranges={()=>[{key:"recent7",label:"近7天",from:"2026-10-02",to:"2026-10-08"}]}/>);
  fireEvent.click(screen.getByTestId("search-date-picker"));
  fireEvent.change(screen.getByTestId("search-from"),{target:{value:"2026-01-01"}});
  expect(apply).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId("search-quick-recent7"));
  expect(apply).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId("search-date-apply"));
  expect(apply).toHaveBeenCalledExactlyOnceWith({from:"2026-10-02",to:"2026-10-08"});
  expect(screen.queryByTestId("search-date-dialog")).not.toBeInTheDocument();
});

it("取消不修改原范围，倒置日期不能确认",()=>{
  const apply=vi.fn();
  render(<DateRangePicker from="2026-01-01" to="2026-01-30" onApply={apply} ranges={()=>[]}/>);
  fireEvent.click(screen.getByTestId("search-date-picker"));
  fireEvent.change(screen.getByTestId("search-from"),{target:{value:"2026-02-01"}});
  expect(screen.getByTestId("search-date-apply")).toBeDisabled();
  fireEvent.click(screen.getByRole("button",{name:"取消"}));
  expect(apply).not.toHaveBeenCalled();
  fireEvent.click(screen.getByTestId("search-date-picker"));
  expect(screen.getByTestId("search-from")).toHaveValue("2026-01-01");
});
