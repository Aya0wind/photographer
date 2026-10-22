import { describe, expect, it } from "vitest";
import { defaultBrandFrame, frameMetrics, framePhotoData, FRAME_TEMPLATES, materializeFrame } from "./brandFrame";
import { OFFICIAL_BRAND_LOGOS } from "./brandLogos";
import { defaultRecipe, initRecipeHistory, recipeForPersist, recipeReducer } from "./recipe";

describe("brand frames", () => {
  it("reads real exposure data without inventing missing EXIF", () => {
    expect(framePhotoData({ camera: "ILCE-7RM5", focalLength: "50", aperture: "1.8", shutter: "1/250", iso: 100, capturedAt: "2026-10-22T09:20:00+08:00" })).toMatchObject({ brand: "SONY", parameters: "50 mm   ·   f/1.8   ·   1/250 s   ·   ISO 100", date: "2026-10-22 09:20:00" });
    expect(framePhotoData({})).toMatchObject({ brand: "", camera: "", parameters: "", date: "" });
  });
  it("escapes user text and keeps hidden fields out of vector artwork", () => {
    const frame = materializeFrame({ ...defaultBrandFrame(), camera: '<img onerror="bad">', caption: "secret", showCaption: false });
    expect(frame.footerSvg).toContain("&lt;img onerror=&quot;bad&quot;&gt;");
    expect(frame.footerSvg).not.toContain("<img");
    expect(frame.footerSvg).not.toContain("secret");
  });
  it("has distinct ready-to-use templates and uses aspect-independent footer art", () => {
    const svgs = FRAME_TEMPLATES.map(p => materializeFrame({ ...defaultBrandFrame(), ...p.patch }).footerSvg);
    expect(new Set(svgs).size).toBeGreaterThanOrEqual(12);
    const f = defaultBrandFrame(), landscape = frameMetrics(f, 1.5), portrait = frameMetrics(f, 2 / 3);
    expect(landscape.width).toBe(portrait.width);
    expect(portrait.height).toBeGreaterThan(landscape.height);
    expect(portrait.height - portrait.photoHeight).toBeCloseTo(f.padding + f.footerHeight);
  });
  it("persists frames and supports undo, redo and reset", () => {
    const context = { width: 4000, height: 3000 };
    let state = initRecipeHistory(defaultRecipe());
    const frame = materializeFrame(defaultBrandFrame());
    state = recipeReducer(state, { type: "brandFrame", value: frame }, context);
    expect(recipeForPersist(state.present).brandFrame).toEqual(frame);
    state = recipeReducer(state, { type: "undo" }, context);
    expect(state.present.brandFrame).toBeUndefined();
    state = recipeReducer(state, { type: "redo" }, context);
    expect(state.present.brandFrame).toEqual(frame);
    state = recipeReducer(state, { type: "reset" }, context);
    expect(state.present.brandFrame).toBeUndefined();
  });
  it("embeds official vector paths instead of drawing brand names with fonts", () => {
    for (const brand of Object.keys(OFFICIAL_BRAND_LOGOS)) {
      const f=materializeFrame({...defaultBrandFrame(),brand,showCamera:false,showParameters:false,showDate:false});
      expect(f.footerSvg).toContain("data:image/svg+xml;base64,");
      expect(f.footerSvg).not.toContain("<text");
      expect(OFFICIAL_BRAND_LOGOS[brand].svg).toContain("<path");
    }
  });
  it("fits the whole photo into wide and square canvases without distorting it", () => {
    for (const ratio of [1,3/2,16/9,4/5,9/16]) {
      const f={...defaultBrandFrame(),canvasRatio:ratio};
      for(const aspect of [2/3,3/2]) {
        const m=frameMetrics(f,aspect);
        expect(m.width/m.height).toBeCloseTo(ratio);
        expect(m.x).toBeGreaterThanOrEqual(0);expect(m.y).toBeGreaterThanOrEqual(0);
        expect(m.x+1000).toBeLessThanOrEqual(m.width+0.001);
        expect(m.y+m.photoHeight).toBeLessThanOrEqual(m.height-f.footerHeight+0.001);
      }
    }
  });
  it("sizes the real logo independently and expands the footer for readable text", () => {
    const original={...defaultBrandFrame(),camera:"ILCE-7RM5",parameters:"50 mm · f/1.8 · ISO 100",date:"2026-10-22"};
    const small=materializeFrame({...original,logoSize:50}), large=materializeFrame({...original,logoSize:200});
    expect(small.footerSvg).toContain('width="80"');expect(large.footerSvg).toContain('width="320"');
    expect(frameMetrics(large,1.5).footerHeight).toBeGreaterThan(frameMetrics(small,1.5).footerHeight);
    expect(frameMetrics({...original,fontSize:40},1.5).footerHeight).toBeGreaterThan(frameMetrics({...original,fontSize:12},1.5).footerHeight);
  });
});
