import {beforeEach, expect, it, vi} from "vitest";
import {fireEvent, render, screen} from "@testing-library/react";
import {I18nextProvider} from "react-i18next";
import i18n from "@/i18n";
import {editFilmList, editRawlabAvailable} from "@/ipc/api";
import FilmPanel from "./FilmPanel";

vi.mock("@/ipc/api", () => ({
  editFilmList: vi.fn(), editFilmImport: vi.fn(), editRawlabAvailable: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({open: vi.fn()}));

const id = "a".repeat(64);
beforeEach(() => {
  vi.mocked(editRawlabAvailable).mockResolvedValue({available: true, coreRevision: "pinned"});
  vi.mocked(editFilmList).mockResolvedValue([
    {id, name: "滨田英明", size: 65, inputTransfer: 5, outputTransfer: 5, builtin: true,
      subtitle: "日系青空", summary: "柔和日光"},
    {id: "b".repeat(64), name: "My film", size: 33, inputTransfer: 5, outputTransfer: 5},
  ]);
});

it("offers built-in master colors without importing or first enabling the pipeline", async () => {
  const change = vi.fn();
  render(<I18nextProvider i18n={i18n}><FilmPanel value={undefined} onChange={change} begin={vi.fn()} end={vi.fn()}/></I18nextProvider>);
  const select = await screen.findByRole("combobox");
  expect(screen.getByRole("group", {name: "内置大师色彩"})).toBeInTheDocument();
  expect(screen.getByRole("group", {name: "已导入胶片"})).toBeInTheDocument();
  fireEvent.change(select, {target: {value: id}});
  expect(change).toHaveBeenCalledWith({version: 1, backend: "rawlab", coreRevision: "pinned",
    film: {id, amount: 100, enabled: true}});
});

it("shows the built-in description and retains the saved film strength", async () => {
  const change = vi.fn();
  render(<I18nextProvider i18n={i18n}><FilmPanel value={{version: 1, backend: "rawlab", coreRevision: "pinned",
    film: {id, amount: 40, enabled: true}}} onChange={change} begin={vi.fn()} end={vi.fn()}/></I18nextProvider>);
  expect(await screen.findByText("柔和日光")).toBeInTheDocument();
  fireEvent.change(screen.getByRole("combobox"), {target: {value: "b".repeat(64)}});
  expect(change).toHaveBeenCalledWith({version: 1, backend: "rawlab", coreRevision: "pinned",
    film: {id: "b".repeat(64), amount: 40, enabled: true}});
});
