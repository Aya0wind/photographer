import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import i18n from "@/i18n";
import BrandFramePanel from "./BrandFramePanel";
import { defaultBrandFrame, materializeFrame } from "../lib/brandFrame";
vi.mock("@/ipc/api", () => ({ editAnnotationFonts: vi.fn(async () => ["Georgia"]) }));
beforeEach(() => { localStorage.clear(); void i18n.changeLanguage("zh"); });
describe("frame presets", () => {
  it("applies a template while keeping this photo's exposure data", () => {
    const onChange = vi.fn();
    render(<I18nextProvider i18n={i18n}><BrandFramePanel value={materializeFrame({ ...defaultBrandFrame(), parameters: "ISO 640", camera: "My camera" })} onChange={onChange} onReadMetadata={() => {}} /></I18nextProvider>);
    fireEvent.click(screen.getByTestId("frame-template-noir"));
    expect(onChange.mock.lastCall?.[0]).toMatchObject({ parameters: "ISO 640", camera: "My camera", background: "#111111" });
  });
  it("saves reusable appearance without copying capture data to the next photo", () => {
    render(<I18nextProvider i18n={i18n}><BrandFramePanel value={materializeFrame({ ...defaultBrandFrame(), parameters: "ISO 640", camera: "Private camera", date: "Yesterday", caption: "Private caption" })} onChange={() => {}} onReadMetadata={() => {}} /></I18nextProvider>);
    fireEvent.change(screen.getByLabelText("方案名称"), { target: { value: "我的黑框" } });
    fireEvent.click(screen.getByRole("button", { name: "保存" }));
    const stored = JSON.parse(localStorage.getItem("photo-hub.brand-frame-presets.v1")!);
    expect(stored[0]).toMatchObject({ name: "我的黑框", frame: { parameters: "", camera: "", date: "", caption: "" } });
    expect(localStorage.getItem("photo-hub.brand-frame-presets.v1")).not.toContain("Private camera");
    fireEvent.click(screen.getByRole("button", { name: "删除 我的黑框" }));
    expect(JSON.parse(localStorage.getItem("photo-hub.brand-frame-presets.v1")!)).toEqual([]);
  });
});
