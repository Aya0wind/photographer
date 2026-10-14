import {act, renderHook} from "@testing-library/react";
import {beforeEach, expect, it, vi} from "vitest";
import {useExternalImportDrop} from "./useExternalImportDrop";

const {subscribe} = vi.hoisted(() => ({subscribe: vi.fn()}));
vi.mock("@tauri-apps/api/core", () => ({isTauri: () => true}));
vi.mock("@tauri-apps/api/webview", () => ({getCurrentWebview: () => ({onDragDropEvent: subscribe})}));
beforeEach(() => {subscribe.mockReset().mockResolvedValue(vi.fn());});

it("shows the drag hint and delivers all absolute paths once on drop", () => {
  const drop = vi.fn();
  const {result} = renderHook(() => useExternalImportDrop(drop));
  const receive = subscribe.mock.calls[0][0];
  const paths = ["C:/A.JPG", "D:/photos"];
  act(() => receive({payload: {type: "enter", paths}}));
  expect(result.current).toBe(true); expect(drop).not.toHaveBeenCalled();
  act(() => receive({payload: {type: "drop", paths}}));
  expect(result.current).toBe(false); expect(drop).toHaveBeenCalledExactlyOnceWith(paths);
});

it("does not interpret internal drags as a file import and unsubscribes after a late registration", async () => {
  let finish!:(off:()=>void)=>void;
  subscribe.mockImplementation(() => new Promise(resolve => {finish = resolve;}));
  const drop = vi.fn(), off = vi.fn();
  const {result, unmount} = renderHook(() => useExternalImportDrop(drop));
  const receive = subscribe.mock.calls[0][0];
  act(() => receive({payload: {type: "enter", paths: []}}));
  act(() => receive({payload: {type: "drop", paths: []}}));
  expect(result.current).toBe(false); expect(drop).not.toHaveBeenCalled();
  unmount(); await act(async () => finish(off));
  expect(off).toHaveBeenCalledOnce();
  receive({payload: {type: "drop", paths: ["C:/late.JPG"]}});
  expect(drop).not.toHaveBeenCalled();
});
