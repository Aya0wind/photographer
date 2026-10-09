import type { AddProtocolAction, StyleSpecification } from "maplibre-gl";
import outlineUrl from "../assets/world-outline.json?url";

export const MAP_STYLE_URL = "https://tiles.openfreemap.org/styles/dark";
const CACHE_NAME = "photohub-map-v1";
const MAX_BYTES = 128 * 1024 * 1024;
const MAX_ENTRY_BYTES = 8 * 1024 * 1024;
const MAX_ENTRIES = 1024;
let writes = Promise.resolve();
let sizes: Map<string, number> | null = null;
let cacheBytes = 0;

/** Global WebView cache, shared by libraries; unavailable storage never blocks a map. */
async function openCache(): Promise<Cache | null> {
  try { return await globalThis.caches?.open(CACHE_NAME) ?? null; }
  catch { return null; }
}

function persist(cache: Cache, url: string, body: ArrayBuffer, headers: Headers) {
  if (body.byteLength > MAX_ENTRY_BYTES) return;
  writes = writes.then(async () => {
    if (!sizes) {
      const keys = await cache.keys();
      sizes = new Map(await Promise.all(keys.map(async key => [key.url,
        Number((await cache.match(key))?.headers.get("x-ph-size") ?? 0)] as const)));
      cacheBytes = [...sizes.values()].reduce((sum, size) => sum + size, 0);
    }
    const stored = new Headers(headers);
    stored.set("x-ph-stored", String(Date.now()));
    stored.set("x-ph-size", String(body.byteLength));
    await cache.put(url, new Response(body, { headers: stored }));
    cacheBytes += body.byteLength - (sizes.get(url) ?? 0);
    sizes.set(url, body.byteLength);
    for (const [key, size] of sizes) {
      if (cacheBytes <= MAX_BYTES && sizes.size <= MAX_ENTRIES) break;
      await cache.delete(key);
      cacheBytes -= size;
      sizes.delete(key);
    }
  }).catch(() => { /* Quota/private-mode failure leaves ordinary HTTP loading usable. */ });
}

/** Metadata expires daily; URL-versioned tiles, fonts and sprites last a month.
 * An expired copy remains usable offline. Never persist errors or aborted requests. */
export async function fetchMapResource(url: string, signal: AbortSignal): Promise<Response> {
  if (signal.aborted) throw new DOMException("Aborted", "AbortError");
  const cache = await openCache();
  const cached = await cache?.match(url).catch(() => undefined);
  if (signal.aborted) throw new DOMException("Aborted", "AbortError");
  const ttl = /\/styles\/|\/planet$/.test(url) ? 86400_000 : 30 * 86400_000;
  const saved = Number(cached?.headers.get("x-ph-stored") ?? 0);
  if (cached && Date.now() - saved < ttl) return cached;
  try {
    const response = await fetch(url, { signal });
    if (!response.ok) throw new Error(`Map HTTP ${response.status}`);
    if (cache) {
      // Read separately so cache maintenance is never on the rendering critical path.
      void response.clone().arrayBuffer().then(body => {
        if (!signal.aborted) persist(cache, url, body, response.headers);
      }).catch(() => {});
    }
    return response;
  } catch (error) {
    if (cached && !signal.aborted) return cached;
    throw error;
  }
}

export function transformMapRequest(url: string) {
  return { url: url.startsWith("https://tiles.openfreemap.org/")
    ? `phmap://${encodeURIComponent(url)}` : url };
}

export const mapProtocol: AddProtocolAction = async (request, controller) => {
  const url = decodeURIComponent(request.url.slice("phmap://".length));
  if (!url.startsWith("https://tiles.openfreemap.org/")) throw new Error("Invalid map resource");
  const response = await fetchMapResource(url, controller.signal);
  return { data: request.type === "json" ? await response.json() : await response.arrayBuffer() };
};

export function withLocalOutline(style?: StyleSpecification): StyleSpecification {
  const layers = style?.layers ?? [{ id: "background", type: "background" as const,
    paint: { "background-color": "#12171e" } }];
  const firstContent = layers.findIndex(layer => layer.type !== "background");
  const position = firstContent < 0 ? layers.length : firstContent;
  return {
    ...(style ?? { version: 8 }),
    sources: { ...style?.sources, "ph-outline": {
      type: "geojson", data: outlineUrl, attribution: "Natural Earth", maxzoom: 4,
    } },
    layers: [...layers.slice(0, position), {
      id: "ph-land", type: "fill", source: "ph-outline",
      paint: { "fill-color": "#252e38", "fill-outline-color": "#3a4652" },
    }, ...layers.slice(position)],
  };
}
