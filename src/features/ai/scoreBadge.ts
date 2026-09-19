/**
 * 相似度角标分档（语义搜索结果右下百分比角标，AssetGrid 渲染）：
 * - high（> 0.75）：accent 底——强命中，一眼锁定目标
 * - mid（0.60–0.75，含端点 0.60/0.75）：muted 白——可用候选
 * - low（< 0.60）：灰——弱相关（后端已按 score 降序，角标只做提示不做过滤）
 * 阈值常量导出供测试与调用方对齐（边界：> HIGH 为 high，>= MID 为 mid）。
 */

export const SIMILARITY_HIGH_THRESHOLD = 0.75;
export const SIMILARITY_MID_THRESHOLD = 0.6;

export type SimilarityTier = "high" | "mid" | "low";

export function similarityTier(score: number): SimilarityTier {
  if (score > SIMILARITY_HIGH_THRESHOLD) return "high";
  if (score >= SIMILARITY_MID_THRESHOLD) return "mid";
  return "low";
}

/** 各档角标样式（叠在缩略图上；深底半透明保证可读，high 档 accent 强调） */
export const SIMILARITY_BADGE_CLASS: Record<SimilarityTier, string> = {
  high: "bg-accent/90 text-black",
  mid: "bg-black/60 text-white/85",
  low: "bg-black/60 text-white/45",
};
