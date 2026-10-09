import { beforeEach, describe, expect, it, vi } from "vitest";
import { assetsByIds, type AssetDto } from "@/ipc/api";
import { message } from "@tauri-apps/plugin-dialog";
import { showEditorWindow } from "./editorWindow";
import { openAdvancedEditor } from "./advancedEditorStore";

vi.mock("@/ipc/api", () => ({ assetsByIds: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ message: vi.fn() }));
vi.mock("./editorWindow", () => ({ showEditorWindow: vi.fn() }));
const asset: AssetDto = { id: 1, path: "I:/photos/a.jpg", name: "a.jpg",
  kind: "photo", capturedAt: null, camera: null, sizeBytes: 10, libraryId: "photos" };

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(showEditorWindow).mockResolvedValue();
  vi.mocked(message).mockResolvedValue("OK");
});

describe("advanced editor entry", () => {
  it("uses photo ownership and carries album context into the independent window", async () => {
    expect(await openAdvancedEditor(asset, { albumId: 7 })).toBe(true);
    expect(showEditorWindow).toHaveBeenCalledWith({ asset, libraryId: "photos", albumId: 7 });
    expect(assetsByIds).not.toHaveBeenCalled();
  });
  it("refreshes missing ownership instead of silently ignoring the click", async () => {
    vi.mocked(assetsByIds).mockResolvedValue([asset]);
    expect(await openAdvancedEditor({ ...asset, libraryId: undefined })).toBe(true);
    expect(assetsByIds).toHaveBeenCalledWith([1]);
    expect(showEditorWindow).toHaveBeenCalledOnce();
  });
  it("reports missing ownership and does not open a different database's colliding asset id", async () => {
    vi.mocked(assetsByIds).mockResolvedValue([{ ...asset, path: "I:/other/a.jpg" }]);
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await openAdvancedEditor({ ...asset, libraryId: null })).toBe(false);
    expect(showEditorWindow).not.toHaveBeenCalled();
    expect(message).toHaveBeenCalledWith(expect.any(String), expect.objectContaining({ kind: "error" }));
    log.mockRestore();
  });
  it("reports window creation failures instead of dismissing the preview as if opened", async () => {
    vi.mocked(showEditorWindow).mockRejectedValueOnce(new Error("window creation failed"));
    const log = vi.spyOn(console, "error").mockImplementation(() => {});
    expect(await openAdvancedEditor(asset)).toBe(false);
    expect(message).toHaveBeenCalledWith("Error: window creation failed", expect.objectContaining({ kind: "error" }));
    log.mockRestore();
  });
});
