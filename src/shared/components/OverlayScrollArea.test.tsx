import { fireEvent, render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { beforeAll, expect, it, vi } from "vitest";
import i18n from "@/i18n";
import OverlayScrollArea from "./OverlayScrollArea";

beforeAll(() => {
  Object.defineProperty(HTMLElement.prototype, "clientHeight", { configurable: true, get: () => 200 });
  Object.defineProperty(HTMLElement.prototype, "scrollHeight", { configurable: true, get: () => 1000 });
});
it("drags beyond the rail and supports keyboard scrolling", () => {
  render(<I18nextProvider i18n={i18n}><OverlayScrollArea testId="scroll-area"><div>People</div></OverlayScrollArea></I18nextProvider>);
  const track = screen.getByRole("scrollbar");
  vi.spyOn(track, "getBoundingClientRect").mockReturnValue({ top: 0, left: 0, right: 20, bottom: 200, width: 20, height: 200, x: 0, y: 0, toJSON: () => ({}) });
  const thumb = track.querySelector<HTMLElement>("[data-scroll-thumb]")!;
  fireEvent.mouseDown(thumb, { button: 0, clientY: 20 });
  fireEvent.mouseMove(window, { clientY: 120 });
  expect(screen.getByTestId("scroll-area").scrollTop).toBeGreaterThan(400);
  fireEvent.mouseUp(window);
  fireEvent.keyDown(track, { key: "End" });
  expect(screen.getByTestId("scroll-area").scrollTop).toBe(800);
  expect(track).toHaveAttribute("aria-valuenow", "100");
});
