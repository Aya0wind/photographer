import type { AiModelStatus, AiQualityTier } from "@/ipc/api";

/**
 * AI 三档画质（快速/普通/精准）前端常量与纯函数（与后端 lane 契约对齐）：
 * - 档位→所需模型 id 映射是展示/缺件判定唯一真值（切档本身只走
 *   settings_set(ai.qualityTier)，索引重建由后端指纹机制自动触发，无新命令）。
 * - 模型 id 别名：现行后端清单的语义视觉模型 id 为 siglip2-visual，契约档位表
 *   用 siglip2-vision——归一化让两代 id 都能命中（后端落地新清单后自然收敛）。
 */

export type QualityTier = AiQualityTier;

/** 展示顺序 */
export const QUALITY_TIERS: readonly QualityTier[] = ["fast", "normal", "accurate"] as const;

/** 契约档位→所需模型 id 集合（不得偏移；null 档共用件如 tokenizer 在各档都出现） */
export const TIER_REQUIRED_MODELS: Record<QualityTier, readonly string[]> = {
  fast: ["scrfd-10g", "siglip2-vision", "siglip2-text", "siglip2-tokenizer"],
  normal: ["scrfd", "siglip2-vision", "siglip2-text", "siglip2-tokenizer"],
  accurate: ["scrfd", "siglip2-vision-fp16", "siglip2-text-fp16", "siglip2-tokenizer"],
};

/** 人脸识别（聚类）模型：各档共用，不在档位表内 */
export const FACE_SHARED_MODEL = "arcface";

/** 模型 id 归一（别名折叠到契约 id） */
const MODEL_ID_ALIASES: Record<string, string> = {
  "siglip2-visual": "siglip2-vision",
};

export function normalizeModelId(id: string): string {
  return MODEL_ID_ALIASES[id] ?? id;
}

/** 档位所需模型 id（契约序） */
export function requiredModelIds(tier: QualityTier): readonly string[] {
  return TIER_REQUIRED_MODELS[tier];
}

/** 档位所需语义三件（嵌入对：视觉/文本/分词器） */
export function tierSemanticIds(tier: QualityTier): readonly string[] {
  return TIER_REQUIRED_MODELS[tier].filter((id) => id.startsWith("siglip2"));
}

/** 档位所需人脸件（检测模型随档位 + 聚类共用件 arcface） */
export function tierFaceIds(tier: QualityTier): readonly string[] {
  return [
    ...TIER_REQUIRED_MODELS[tier].filter((id) => id.startsWith("scrfd")),
    FACE_SHARED_MODEL,
  ];
}

/** 档位缺件：所需模型里 state!=="done" 的条目（清单未收录的 id 也算缺，status=null） */
export interface TierModelGap {
  /** 契约 id（归一后；发起下载用 gap.status.id 原始清单 id） */
  id: string;
  /** 清单内快照；null=后端清单暂无此模型（无法发起下载） */
  status: AiModelStatus | null;
}

/** 按 id 建索引（归一 key → 最新快照） */
function indexById(models: readonly AiModelStatus[]): Map<string, AiModelStatus> {
  const map = new Map<string, AiModelStatus>();
  for (const model of models) map.set(normalizeModelId(model.id), model);
  return map;
}

/** 一组契约 id 的就绪判定/缺件（ids 全 done 才算齐备） */
export function gapsForIds(
  ids: readonly string[],
  models: readonly AiModelStatus[],
): TierModelGap[] {
  const byId = indexById(models);
  const gaps: TierModelGap[] = [];
  for (const id of ids) {
    const status = byId.get(id) ?? null;
    if (!status || status.state !== "done") gaps.push({ id, status });
  }
  return gaps;
}

/** 档位缺件清单 */
export function tierModelGaps(
  tier: QualityTier,
  models: readonly AiModelStatus[],
): TierModelGap[] {
  return gapsForIds(TIER_REQUIRED_MODELS[tier], models);
}

/** 档位所需模型是否齐备（缺件为空） */
export function tierModelsReady(
  tier: QualityTier,
  models: readonly AiModelStatus[],
): boolean {
  return tierModelGaps(tier, models).length === 0;
}

/** 档位就绪计数（N/M 展示；M=契约所需件数，含清单未收录） */
export function tierReadyCount(
  tier: QualityTier,
  models: readonly AiModelStatus[],
): { ready: number; total: number } {
  const total = TIER_REQUIRED_MODELS[tier].length;
  return { ready: total - tierModelGaps(tier, models).length, total };
}

/** 语义功能门控：当前档语义三件齐备（替代旧「组内全部 done」——新清单含
 *  多档变体后，普通档用户不应因未装 fp16 件而被禁用语义开关） */
export function tierSemanticReady(
  tier: QualityTier,
  models: readonly AiModelStatus[],
): boolean {
  return gapsForIds(tierSemanticIds(tier), models).length === 0;
}

/** 人脸功能门控：当前档检测模型 + arcface 齐备 */
export function tierFaceReady(
  tier: QualityTier,
  models: readonly AiModelStatus[],
): boolean {
  return gapsForIds(tierFaceIds(tier), models).length === 0;
}
