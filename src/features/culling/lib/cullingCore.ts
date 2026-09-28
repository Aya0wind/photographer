import {
  assetsPage,
  type AssetFilters,
  type CullDecisionValue,
  type CullFinishApply,
  type CullScope,
} from "@/ipc/api";

/**
 * 选片纯逻辑核（无 React 依赖，供组件与测试共用）：
 * - 决定状态机：Map<assetId, decision> 的不可变更新 + 进度计数派生
 * - 键盘映射：全屏过片键位 → 语义动作（与查看器键位协调由浮层负责）
 * - 收尾映射：弹窗开关 → cullSessionFinish 载载组装
 * - id 快照收集：画廊筛选结果全量 id（keyset 分页直取全量）
 */

export type { CullDecisionValue };

/** 未定 = null（与 cull_decision 表「无行」语义一致） */
export type CullDecision = CullDecisionValue | null;

export type CullDecisionMap = ReadonlyMap<number, CullDecision>;

/** 进度计数（纯派生自决定表；总数独立传入——会话快照长度） */
export interface CullProgress {
  accepted: number;
  rejected: number;
  undecided: number;
  total: number;
}

/** 决定表 → 进度计数（未定 = 总数 − 已决定；总数取 max(total, 表长) 防脏数据） */
export function deriveProgress(total: number, decisions: CullDecisionMap): CullProgress {
  let accepted = 0;
  let rejected = 0;
  for (const decision of decisions.values()) {
    if (decision === "accepted") accepted += 1;
    else if (decision === "rejected") rejected += 1;
  }
  const effectiveTotal = Math.max(total, accepted + rejected);
  return { accepted, rejected, undecided: Math.max(0, effectiveTotal - accepted - rejected), total: effectiveTotal };
}

/** 不可变更新决定表（同值直返原引用，避免无谓重渲） */
export function withDecision(
  decisions: CullDecisionMap,
  assetId: number,
  decision: CullDecision,
): CullDecisionMap {
  if ((decisions.get(assetId) ?? null) === decision) return decisions;
  const next = new Map(decisions);
  if (decision === null) next.delete(assetId);
  else next.set(assetId, decision);
  return next;
}

/** 键盘动作（null = 非选片键；修饰键组合一律不接管——让位应用快捷键） */
export type CullKeyAction = "prev" | "next" | "accept" | "reject" | "undecided";

/** 键位表（方案 §3.1 V1）：←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 · U 回未定。
 *  Z（按住放大）与 Esc（退出）带持续语义/浮层副作用，不在此映射。 */
export function cullKeyAction(e: {
  key: string;
  code?: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
  shiftKey?: boolean;
}): CullKeyAction | null {
  if (e.ctrlKey || e.metaKey || e.altKey) return null;
  const key = e.key;
  const isSpace = key === " " || key === "Spacebar" || e.code === "Space";
  if (key === "ArrowLeft") return "prev";
  if (key === "ArrowRight") return "next";
  if (isSpace || key === "ArrowUp") return "accept";
  if (key === "x" || key === "X" || key === "ArrowDown") return "reject";
  if (key === "u" || key === "U") return "undecided";
  return null;
}

/** 决定后游标：向后推进（尾张停住——最后一张的决定不再翻页） */
export function indexAfterDecision(index: number, total: number): number {
  if (total <= 0) return 0;
  return Math.min(index + 1, total - 1);
}

// --- 收尾映射组装 -------------------------------------------------------------------

/** 收尾弹窗配置（rating=null 即星级开关关闭） */
export interface CullFinishConfig {
  acceptedFlag: boolean;
  acceptedRating: number | null;
  rejectRejected: boolean;
}

/** 配置 → cullSessionFinish 载荷：星级规整到 1-5 整数（非法/越界回退 null=不应用） */
export function buildFinishApply(config: CullFinishConfig): CullFinishApply {
  const rating =
    config.acceptedRating === null
      ? null
      : Number.isInteger(config.acceptedRating) &&
          config.acceptedRating >= 1 &&
          config.acceptedRating <= 5
        ? config.acceptedRating
        : null;
  return {
    acceptedFlag: config.acceptedFlag === true,
    acceptedRating: rating,
    rejectRejected: config.rejectRejected === true,
  };
}

// --- 画廊筛选结果 id 快照（「从此筛选选片」入口） -------------------------------------

/** keyset 全量收集的单页上限（减少往返；assets_page 服务端无硬上限，取稳定值） */
export const ID_SNAPSHOT_PAGE_LIMIT = 500;

/**
 * 当前筛选条件的全量资产 id（画廊现有 API 只有 keyset 分页——这里按查询参数
 * 循环直取全量：afterId 翻页直到短页；空筛选同样适用=全库快照）。
 * 失败页按 [] 处理即止（assetsPage 契约）；返回 id 序与画廊一致（拍摄时间序）。
 */
export async function collectAssetIdsByFilters(filters?: AssetFilters): Promise<number[]> {
  const ids: number[] = [];
  let afterId = 0;
  for (let guard = 0; guard < 10000; guard += 1) {
    const page = await assetsPage(afterId, ID_SNAPSHOT_PAGE_LIMIT, filters);
    if (page.length === 0) break;
    for (const asset of page) ids.push(asset.id);
    if (page.length < ID_SNAPSHOT_PAGE_LIMIT) break;
    afterId = page[page.length - 1].id;
  }
  return ids;
}

// --- 会话来源描述 -------------------------------------------------------------------

/** scope 的来源描述文案键（相册名由调用方解析后回填 name/subgroup） */
export function scopeDescKey(scope: CullScope): "album" | "albumSubgroup" | "query" {
  if (scope.kind === "album") return scope.subgroup === null ? "album" : "albumSubgroup";
  return "query";
}
