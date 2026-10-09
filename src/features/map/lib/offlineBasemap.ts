import type { GeoJSONSource, GeoJSONSourceSpecification, Map as LibreMap, StyleSpecification } from "maplibre-gl";

import { mapBasemap, type BasemapData, type BasemapCollection } from "@/ipc/api/map";

import outlineUrl from "../assets/world-outline.json?url";

import { levelForZoom } from "./hierarchy";

const EMPTY: BasemapCollection = { type: "FeatureCollection", features: [] };
const SOURCES = ["world-areas", "world-labels", "detail-areas", "detail-labels"] as const;
const PALETTES = {
  dark: { ocean: "#101822", land: "#252D36", country: "#66717D", province: "#56616E", city: "#424E5C", district: "#3B4654", text: "#B8C3D1", halo: "#1C242E" },
  light: { ocean: "#DFEAF1", land: "#F5F3EC", country: "#A5ADB1", province: "#B3B8B7", city: "#C1C4C0", district: "#CDCFCA", text: "#5A666F", halo: "#F5F3EC" },
};

function palette() {
  return PALETTES[document.documentElement.dataset.theme === "light" ? "light" : "dark"];
}

/** No remote style, tiles, sprites, glyphs or fonts. MapLibre 6 draws missing glyphs locally. */
export function offlineMapStyle(): StyleSpecification {
  const colors = palette();
  const sources: Record<string, GeoJSONSourceSpecification> = Object.fromEntries(SOURCES.map((id) => [id, { type: "geojson" as const, data: EMPTY }]));
  sources["world-areas"].data = outlineUrl;
  const layers: StyleSpecification["layers"] = [
    { id: "ocean", type: "background", paint: { "background-color": colors.ocean } },
    { id: "land", type: "fill", source: "world-areas", paint: { "fill-color": colors.land } },
    { id: "country", type: "line", source: "world-areas", paint: { "line-color": colors.country, "line-width": 0.8 } },
  ];
  const boundaries = ["province", "city", "district"] as const;
  boundaries.forEach((id, i) => {
    layers.push({
      id, type: "line", source: "detail-areas", minzoom: [3.5, 6.5, 9.5][i],
      filter: ["==", ["get", "level"], i + 1],
      paint: { "line-color": colors[id], "line-width": [0.8, 0.6, 0.5][i] },
    });
  });
  // Fine labels enter collision placement first, then broader administrative names.
  for (const level of [0, 1, 2, 3]) {
    layers.push({
      id: `label-${level}`, type: "symbol", source: level === 0 ? "world-labels" : "detail-labels",
      minzoom: [0, 3.5, 6.5, 9.5][level], ...(level === 0 ? { maxzoom: 6.5 } : {}),
      filter: ["==", ["get", "level"], level],
      layout: {
        "text-field": ["get", "name"], "text-font": ["sans-serif"],
        "text-size": level === 0 ? 13 : 11, "text-padding": 10, "text-max-width": 12,
      },
      paint: { "text-color": colors.text, "text-halo-color": colors.halo, "text-halo-width": 1.5 },
    });
  }
  sources["world-areas"] = { ...sources["world-areas"], attribution: "Natural Earth · DataV" };
  return { version: 8, sources, layers };
}

function applyPalette(map: LibreMap) {
  const colors = palette();
  map.setPaintProperty("ocean", "background-color", colors.ocean);
  map.setPaintProperty("land", "fill-color", colors.land);
  for (const id of ["country", "province", "city", "district"] as const) {
    map.setPaintProperty(id, "line-color", colors[id]);
  }
  for (let i = 0; i <= 3; i++) {
    map.setPaintProperty(`label-${i}`, "text-color", colors.text);
    map.setPaintProperty(`label-${i}`, "text-halo-color", colors.halo);
  }
}

function setData(map: LibreMap, prefix: "world" | "detail", data: BasemapData) {
  (map.getSource(`${prefix}-areas`) as GeoJSONSource).setData(data.areas);
  (map.getSource(`${prefix}-labels`) as GeoJSONSource).setData(data.labels);
}

function viewportBounds(map: LibreMap): [number, number, number, number] {
  const bounds = map.getBounds();
  const padX = (bounds.getEast() - bounds.getWest()) * 0.2;
  const padY = (bounds.getNorth() - bounds.getSouth()) * 0.2;
  return [bounds.getWest() - padX, Math.max(-90, bounds.getSouth() - padY), bounds.getEast() + padX, Math.min(90, bounds.getNorth() + padY)];
}

/** One request in flight; movement coalesces to the newest view and stale results never replace it. */
export function attachOfflineBasemap(map: LibreMap, onError: (failed: boolean) => void, initiallyReady = true) {
  let enabled = initiallyReady;
  let disposed = false;
  let worldReady = false;
  let styleReady = false;
  let busy = false;
  let pending = false;
  let revision = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function load() {
    if (disposed || busy || !styleReady || !enabled) return;
    clearTimeout(timer);
    busy = true;
    pending = false;
    const request = revision;
    try {
      if (!worldReady) {
        const world = await mapBasemap(0, [-180, -90, 180, 90]);
        if (disposed || request !== revision) return;
        setData(map, "world", world);
        worldReady = true;
      }
      if (request !== revision) return;
      const level = levelForZoom(map.getZoom());
      const detail = level === 0
        ? { areas: EMPTY, labels: EMPTY }
        : await mapBasemap(level, viewportBounds(map));
      if (!disposed && request === revision) {
        setData(map, "detail", detail);
        onError(false);
      }
    } catch (error) {
      if (!disposed && request === revision) {
        console.error("[offline-map]", error);
        onError(true);
      }
    } finally {
      busy = false;
      if (!disposed && pending) void load();
    }
  }

  function reload() {
    revision++;
    pending = true;
    clearTimeout(timer);
    timer = setTimeout(() => void load(), 100);
  }

  function onLoad() {
    styleReady = true;
    applyPalette(map);
    reload();
  }
  const observer = new MutationObserver(() => {
    if (!disposed && map.getLayer("ocean")) applyPalette(map);
  });
  observer.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });
  map.on("load", onLoad);
  map.on("moveend", reload);
  if (map.isStyleLoaded()) onLoad();
  return {
    reload,
    setReady(ready: boolean) {
      if (enabled === ready) return;
      enabled = ready;
      worldReady = false;
      if (ready) reload();
      else {
        revision++;
        pending = false;
        clearTimeout(timer);
        onError(false);
      }
    },
    dispose() {
      disposed = true;
      clearTimeout(timer);
      observer.disconnect();
      map.off("load", onLoad);
      map.off("moveend", reload);
    },
  };
}
