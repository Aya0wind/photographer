import { describe, expect, it } from "vitest";

import type { AiModelStatus } from "@/ipc/api";
import {
  FACE_SHARED_MODEL,
  TIER_REQUIRED_MODELS,
  gapsForIds,
  normalizeModelId,
  requiredModelIds,
  tierFaceIds,
  tierFaceReady,
  tierModelGaps,
  tierModelsReady,
  tierReadyCount,
  tierSemanticIds,
  tierSemanticReady,
} from "./qualityTier";

function model(
  id: string,
  state: AiModelStatus["state"],
  overrides: Partial<AiModelStatus> = {},
): AiModelStatus {
  return {
    id,
    installed: state === "done",
    bytesTotal: 1024,
    downloadedBytes: state === "done" ? 1024 : 0,
    version: null,
    feature: "semantic",
    state,
    tier: null,
    ...overrides,
  };
}

describe("档位→所需模型映射（契约常量）", () => {
  it("三档映射与契约一致（fast 用 scrfd-10g；accurate 用 fp16 件；tokenizer 各档共用）", () => {
    expect([...TIER_REQUIRED_MODELS.fast]).toEqual([
      "scrfd-10g",
      "siglip2-vision",
      "siglip2-text",
      "siglip2-tokenizer",
    ]);
    expect([...TIER_REQUIRED_MODELS.normal]).toEqual([
      "scrfd",
      "siglip2-vision",
      "siglip2-text",
      "siglip2-tokenizer",
    ]);
    expect([...TIER_REQUIRED_MODELS.accurate]).toEqual([
      "scrfd",
      "siglip2-vision-fp16",
      "siglip2-text-fp16",
      "siglip2-tokenizer",
    ]);
    expect(requiredModelIds("normal")).toHaveLength(4);
  });

  it("语义/人脸件拆分：语义=siglip2 三件；人脸=档位 scrfd 件 + 共用 arcface", () => {
    expect([...tierSemanticIds("accurate")]).toEqual([
      "siglip2-vision-fp16",
      "siglip2-text-fp16",
      "siglip2-tokenizer",
    ]);
    expect([...tierFaceIds("fast")]).toEqual(["scrfd-10g", FACE_SHARED_MODEL]);
    expect([...tierFaceIds("normal")]).toEqual(["scrfd", FACE_SHARED_MODEL]);
    expect([...tierFaceIds("accurate")]).toEqual(["scrfd", FACE_SHARED_MODEL]);
  });
});

describe("模型 id 归一（别名兼容）", () => {
  it("现行清单 siglip2-visual 折叠到契约 id siglip2-vision；其余原样", () => {
    expect(normalizeModelId("siglip2-visual")).toBe("siglip2-vision");
    expect(normalizeModelId("siglip2-vision-fp16")).toBe("siglip2-vision-fp16");
    expect(normalizeModelId("scrfd")).toBe("scrfd");
  });
});

describe("缺件判定", () => {
  it("普通档：现行清单 id（siglip2-visual）+ 全 done → 齐备（别名命中）", () => {
    const models = [
      model("siglip2-visual", "done"),
      model("siglip2-text", "done"),
      model("siglip2-tokenizer", "done"),
      model("scrfd", "done"),
    ];
    expect(tierModelGaps("normal", models)).toEqual([]);
    expect(tierModelsReady("normal", models)).toBe(true);
    expect(tierReadyCount("normal", models)).toEqual({ ready: 4, total: 4 });
  });

  it("缺件=所需模型里非 done 的条目（idle/failed/downloading 都算缺）", () => {
    const models = [
      model("siglip2-visual", "done"),
      model("siglip2-text", "downloading"),
      model("siglip2-tokenizer", "failed"),
      model("scrfd", "idle"),
    ];
    const gaps = tierModelGaps("normal", models);
    // 缺件序=契约所需序（scrfd 在首位）
    expect(gaps.map((g) => g.id)).toEqual(["scrfd", "siglip2-text", "siglip2-tokenizer"]);
    // 缺件保留清单内快照（弹窗展示体积/发起下载用原始 id）
    expect(gaps[1].status?.id).toBe("siglip2-text");
    expect(tierModelsReady("normal", models)).toBe(false);
    expect(tierReadyCount("normal", models)).toEqual({ ready: 1, total: 4 });
  });

  it("清单未收录的所需件也算缺（status=null；如后端尚未上架 fp16 件）", () => {
    const models = [
      model("siglip2-visual", "done"),
      model("siglip2-text", "done"),
      model("siglip2-tokenizer", "done"),
    ];
    const gaps = tierModelGaps("accurate", models);
    expect(gaps.map((g) => g.id)).toEqual(["scrfd", "siglip2-vision-fp16", "siglip2-text-fp16"]);
    // 清单未收录 → status=null（弹窗展示「清单未收录」，无法发起下载）
    expect(gaps.every((g) => g.status === null)).toBe(true);
  });

  it("gapsForIds：done 之外的任意状态均缺；空 ids 恒齐备", () => {
    expect(gapsForIds([], [model("scrfd", "idle")])).toEqual([]);
    expect(gapsForIds(["scrfd"], [model("scrfd", "verifying")])).toHaveLength(1);
  });
});

describe("功能门控（按当前档判定）", () => {
  it("普通档语义就绪不要求 fp16 件；装齐 fp16 后精准档也就绪", () => {
    const normalOnly = [
      model("siglip2-visual", "done"),
      model("siglip2-text", "done"),
      model("siglip2-tokenizer", "done"),
    ];
    expect(tierSemanticReady("normal", normalOnly)).toBe(true);
    expect(tierSemanticReady("accurate", normalOnly)).toBe(false);

    const withFp16 = [
      ...normalOnly,
      model("siglip2-vision-fp16", "done"),
      model("siglip2-text-fp16", "done"),
    ];
    expect(tierSemanticReady("accurate", withFp16)).toBe(true);
  });

  it("人脸门控=档位检测件 + arcface；fast 档看 scrfd-10g 而非 scrfd", () => {
    const fastReady = [model("scrfd-10g", "done"), model("arcface", "done")];
    expect(tierFaceReady("fast", fastReady)).toBe(true);
    expect(tierFaceReady("normal", fastReady)).toBe(false);

    const normalReady = [model("scrfd", "done"), model("arcface", "done")];
    expect(tierFaceReady("normal", normalReady)).toBe(true);
    expect(tierFaceReady("fast", normalReady)).toBe(false);
    // arcface 缺失 → 各档人脸都不就绪
    expect(tierFaceReady("normal", [model("scrfd", "done")])).toBe(false);
  });
});
