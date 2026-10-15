import type { AssetDto } from "@/ipc/api";

/**
 * 连拍堆叠折叠（M6，画廊展示层）：日期组内 burstId 相同的**连续**资产折叠为
 * 一张堆叠卡——封面=组内第一张，角标「连拍 N」（N=burstCount，缺失时用实际
 * 连续段长度；含封面）；单张不成组、burstId 缺失不参与。
 *
 * 折叠只影响网格展示：查看器/胶片条仍用未折叠全量（点击堆叠卡打开封面，
 * 胶片条天然顺序翻完整组）。RAW+JPG 合并（mergeRawJpgCards）在前——
 * 折叠判定用合并后代表资产的 burstId。
 */

export interface BurstCollapse {
  /** 展示资产（连拍封面 + 非连拍单张，保持原序） */
  assets: AssetDto[];
  /** 封面 assetId → 连拍张数 N（含封面；仅成组项） */
  badges: Map<number, number>;
  members: Map<number, AssetDto[]>;
  /** 折叠前的真实照片数（组头计数口径） */
  totalCount: number;
}

/**
 * 折叠连续连拍段：两端指针扫连续同 burstId（非空）的 run——
 * run 长度 ≥2 才成组（单张即使带 burstId 也不显示角标）。
 */
export function collapseBursts(assets: AssetDto[]): BurstCollapse {
  const display: AssetDto[] = [];
  const badges = new Map<number, number>();
  const members = new Map<number, AssetDto[]>();

  let runStart = 0;
  const flushRun = (endExclusive: number): void => {
    const run = assets.slice(runStart, endExclusive);
    if (run.length >= 2) {
      // 成组：封面=段内第一张；N=burstCount（含封面），缺失回退实际段长
      const cover = run[0];
      const count = typeof cover.burstCount === "number" && cover.burstCount >= 2
        ? cover.burstCount
        : run.length;
      display.push(cover);
      badges.set(cover.id, count);
      members.set(cover.id, run);
    } else {
      display.push(...run);
    }
  };

  /** 有效连拍 id（null/undefined/0 均视为未分组——后端 id 从 1 起，0 作防御哨兵） */
  const burstKey = (value: number | null | undefined): number | null =>
    typeof value === "number" && value > 0 ? value : null;

  for (let i = 0; i < assets.length; i += 1) {
    const current = assets[i];
    const start = assets[runStart];
    const sameRun =
      burstKey(start.burstId) !== null && current.burstId === start.burstId && i > runStart;
    if (!sameRun && i > runStart) {
      flushRun(i);
      runStart = i;
    }
  }
  flushRun(assets.length);

  return { assets: display, badges, members, totalCount: assets.length };
}

/** Expand only collapsed cards, preserving ordinary single-photo operations. */
export async function burstActionTargets(targets: AssetDto[], members: ReadonlyMap<number, AssetDto[]>,
  fetchMembers: (ids:number[])=>Promise<AssetDto[]>): Promise<AssetDto[]> {
  const covers = targets.filter(asset=>members.has(asset.id)).map(asset=>asset.id);
  if (!covers.length) return targets;
  const complete = await fetchMembers(covers);
  if (!Array.isArray(complete)) throw new Error("无法读取连拍组，请重试");
  const resolved=new Set(complete.map(asset=>asset.id));
  if(covers.some(id=>!resolved.has(id)))throw new Error("连拍组已改变，请刷新后重试");
  const expanded = new Map<number, AssetDto>();
  for (const asset of targets) for (const member of members.get(asset.id)??[asset]) expanded.set(member.id,member);
  for (const asset of complete) expanded.set(asset.id,asset);
  return [...expanded.values()];
}
