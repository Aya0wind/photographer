import { renderHook, waitFor } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";
import capabilities from "../../src-tauri/capabilities/default.json";

const { show, close, getByLabel } = vi.hoisted(() => ({
  show: vi.fn(),
  close: vi.fn(),
  getByLabel: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({ isTauri: () => true }));
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({ show }),
  Window: { getByLabel },
}));

beforeEach(() => {
  vi.resetModules();
  show.mockReset().mockResolvedValue(undefined);
  close.mockReset().mockResolvedValue(undefined);
  getByLabel.mockReset().mockResolvedValue({ close });
});

it("hidden main window has explicit show permission and reveals before closing splash", async () => {
  expect(capabilities.windows).toContain("main");
  expect(capabilities.permissions).toContain("core:window:allow-show");
  const { useWindowReveal } = await import("./windowReveal");
  const { rerender } = renderHook(({ ready }) => useWindowReveal(ready), {
    initialProps: { ready: false },
  });
  expect(show).not.toHaveBeenCalled();
  rerender({ ready: true });
  await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
  expect(getByLabel).toHaveBeenCalledWith("splash");
  expect(show.mock.invocationCallOrder[0]).toBeLessThan(close.mock.invocationCallOrder[0]);
  rerender({ ready: true });
  expect(show).toHaveBeenCalledTimes(1);
});
