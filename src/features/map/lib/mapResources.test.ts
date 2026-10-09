import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";

const url = "https://tiles.openfreemap.org/planet/1/0/0.pbf";
let entries: Map<string, Response>;
let cache: { match: ReturnType<typeof vi.fn>; put: ReturnType<typeof vi.fn>;
  keys: ReturnType<typeof vi.fn>; delete: ReturnType<typeof vi.fn> };

beforeEach(() => {
  vi.resetModules();
  entries = new Map();
  cache = {
    match: vi.fn(async (key: string | Request) => entries.get(typeof key === "string" ? key : key.url)?.clone()),
    put: vi.fn(async (key: string, value: Response) => { entries.set(key, value.clone()); }),
    keys: vi.fn(async () => [...entries.keys()].map(key => new Request(key))),
    delete: vi.fn(async (key: string) => entries.delete(key)),
  };
  vi.stubGlobal("caches", { open: vi.fn(async () => cache) });
});
afterEach(() => vi.unstubAllGlobals());

describe("map resource cache", () => {
  it("persists successful downloads and serves them without another network request", async () => {
    const network = vi.fn(async () => new Response(new Uint8Array([1, 2, 3])));
    vi.stubGlobal("fetch", network);
    const { fetchMapResource } = await import("./mapResources");
    const first = await fetchMapResource(url, new AbortController().signal);
    expect([...new Uint8Array(await first.arrayBuffer())]).toEqual([1, 2, 3]);
    await waitFor(() => expect(cache.put).toHaveBeenCalledTimes(1));
    const second = await fetchMapResource(url, new AbortController().signal);
    expect([...new Uint8Array(await second.arrayBuffer())]).toEqual([1, 2, 3]);
    expect(network).toHaveBeenCalledTimes(1);
  });

  it("refreshes expired metadata but preserves an offline fallback", async () => {
    const styleUrl = "https://tiles.openfreemap.org/styles/dark";
    entries.set(styleUrl, new Response('{"version":8}', { headers: { "x-ph-stored": "1" } }));
    const network = vi.fn().mockRejectedValue(new Error("offline"));
    vi.stubGlobal("fetch", network);
    const { fetchMapResource } = await import("./mapResources");
    expect(await (await fetchMapResource(styleUrl, new AbortController().signal)).json()).toEqual({ version: 8 });
    expect(network).toHaveBeenCalledOnce();
    expect(cache.put).not.toHaveBeenCalled();
  });

  it("does not cache HTTP failures or bypass cancellation with a cached response", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("unavailable", { status: 503 })));
    const { fetchMapResource } = await import("./mapResources");
    await expect(fetchMapResource(url, new AbortController().signal)).rejects.toThrow("503");
    expect(cache.put).not.toHaveBeenCalled();
    entries.set(url, new Response("cached", { headers: { "x-ph-stored": String(Date.now()) } }));
    const controller = new AbortController();
    controller.abort();
    await expect(fetchMapResource(url, controller.signal)).rejects.toMatchObject({ name: "AbortError" });
  });

  it("loads normally when persistent storage is unavailable", async () => {
    vi.stubGlobal("caches", { open: vi.fn().mockRejectedValue(new Error("storage unavailable")) });
    vi.stubGlobal("fetch", vi.fn(async () => new Response("online")));
    const { fetchMapResource } = await import("./mapResources");
    expect(await (await fetchMapResource(url, new AbortController().signal)).text()).toBe("online");
  });

  it("evicts old resources when the cache exceeds 128 MB", async () => {
    // Metadata accounts for old disk resources without allocating 128 MB in a test.
    entries.set(`${url}?old`, new Response("old", { headers: { "x-ph-size": String(128 * 1024 * 1024) } }));
    vi.stubGlobal("fetch", vi.fn(async () => new Response("new")));
    const { fetchMapResource } = await import("./mapResources");
    await fetchMapResource(url, new AbortController().signal);
    await waitFor(() => expect(cache.delete).toHaveBeenCalledWith(`${url}?old`));
    expect(entries.has(url)).toBe(true);
  });

  it("serves JSON and binary resources through the custom protocol", async () => {
    vi.stubGlobal("caches", undefined);
    vi.stubGlobal("fetch", vi.fn(async () => new Response('{"version":8}')));
    const { mapProtocol, transformMapRequest, withLocalOutline } = await import("./mapResources");
    const request = transformMapRequest(url);
    const result = await mapProtocol({ ...request, type: "json" }, new AbortController());
    expect(result.data).toEqual({ version: 8 });
    const bytes = await mapProtocol({ ...request, type: "arrayBuffer" }, new AbortController());
    expect(new TextDecoder().decode(bytes.data as ArrayBuffer)).toBe('{"version":8}');
    expect(transformMapRequest("https://other.example/tile").url).toBe("https://other.example/tile");
    const style = withLocalOutline();
    expect(style.layers.map(layer => layer.type)).toEqual(["background", "fill"]);
    expect(style.sprite).toBeUndefined();
    expect(style.glyphs).toBeUndefined();
  });
});
