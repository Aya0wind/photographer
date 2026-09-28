import type { AssetDto } from "@/ipc/api";

/**
 * RAW+JPG 合并展示（画廊/搜索共用，M3+）：
 * pairId 相同的资产（后端将双向伙伴引用归一为同一个 ID）合并为一张卡——
 * 代表卡优先取 JPG（kind="photo"，缩略图可解码）；卡右上角合并角标「RAW+JPG」
 * （仅两格式都在时；单 RAW 卡的 RAW 水印由 AssetThumb 内部负责，不在这里重复）。
 * 开关关闭、无 pairId 或缺少 RAW/JPG 任一格式时原样透出。
 */

/** 合并卡角标文案（i18n 固定词，两格式都有才标） */
export const PAIR_BADGE = "RAW+JPG";

export interface MergedCards {
  /** 合并后的卡片列表（保持原 DESC 到达顺序，按代表卡首次出现位置排） */
  cards: AssetDto[];
  /** 代表资产 id → 角标文案（仅合并卡有） */
  badges: Map<number, string>;
}

export function mergeRawJpgCards(assets: AssetDto[], enabled: boolean): MergedCards {
  if (!enabled) return { cards: assets, badges: new Map() };

  // pairId → 成员（同 pairId 视为一次拍摄的两格式）
  const pairs = new Map<number, AssetDto[]>();
  for (const asset of assets) {
    if (typeof asset.pairId !== "number") continue;
    const list = pairs.get(asset.pairId);
    if (list) list.push(asset);
    else pairs.set(asset.pairId, [asset]);
  }

  const cards: AssetDto[] = [];
  const badges = new Map<number, string>();
  const emitted = new Set<number>();
  for (const asset of assets) {
    if (typeof asset.pairId !== "number") {
      cards.push(asset);
      continue;
    }
    const members = pairs.get(asset.pairId) ?? [asset];
    const hasPhoto = members.some((m) => m.kind === "photo");
    const hasRaw = members.some((m) => m.kind === "raw");
    if (!hasPhoto || !hasRaw) {
      cards.push(asset);
      continue;
    }
    if (emitted.has(asset.pairId)) continue;
    emitted.add(asset.pairId);
    // 代表卡优先 JPG（缩略图可解码）；全 RAW 对（异常数据）取首个
    const rep = members.find((m) => m.kind === "photo") ?? members[0];
    cards.push(rep);
    // 成对（≥2 且两格式都有）才标「RAW+JPG」；单条/同 kind 不算合并卡
    if (members.length >= 2 && hasPhoto && hasRaw) badges.set(rep.id, PAIR_BADGE);
  }
  return { cards, badges };
}
