import {
  assetsPage,
  type AssetFilters,
  type CullAiRules,
  type CullAiSensitivity,
  type CullDecisionPatch,
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
 * - V2：放大跨图保持锚点记忆 / 对比视图组员选取 / 连拍组一键留张批量组装
 * - V3：AI 挑图表单态 → cull_ai_prescan 规则 DTO
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
export type CullKeyAction =
  | "prev"
  | "next"
  | "accept"
  | "reject"
  | "undecided"
  | "compare";

/** 键位表（方案 §3.1 V1 + §3.2 V2）：←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 ·
 *  U 回未定 · C 单图/对比切换（V2）。Z（按住放大）与 Esc（退出）带持续语义/
 *  浮层副作用，不在此映射。1-4 对比模式选焦由浮层按上下文处理。 */
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
  if (key === "c" || key === "C") return "compare";
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

// --- V2：放大跨图保持（位置 + 倍率会话内记忆） -----------------------------------------

/** 放大锚点（0-1 归一，直接映射 transform-origin 百分比） */
export interface CullZoomPoint {
  x: number;
  y: number;
}

/** 默认锚点：居中 */
export const ZOOM_CENTER: CullZoomPoint = { x: 0.5, y: 0.5 };

/** 归一锚点：越界收夹；NaN 回中心分量（鼠标事件坐标脏值兜底） */
export function clampZoomPoint(x: number, y: number): CullZoomPoint {
  const clamp = (v: number): number =>
    Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : 0.5;
  return { x: clamp(x), y: clamp(y) };
}

/** 按下 Z 的起始锚点：有会话记忆回上次位置（同构图直查谁更锐），无记忆居中 */
export function zoomEnterPoint(memory: CullZoomPoint | null): CullZoomPoint {
  return memory ?? ZOOM_CENTER;
}

// --- V2：对比视图组员选取（同连拍组优先，不足补相邻） -----------------------------------

/**
 * 对比模式屏位成员（返回会话内索引数组，长度 ≤ count）：
 * - 首位恒为当前片（进出对比保持单图位置）
 * - 优先取同连拍组成员（burstId 相同的兄弟，按与当前片距离序——同构图谁更锐）
 * - 不足 count 补相邻片（距离序，等距左侧优先）；会话总张数不足则全量铺开
 *   （仍为当前片优先序）
 */
export function comparisonMembers(
  items: ReadonlyArray<{ burstId: number | null }>,
  index: number,
  count: number,
): number[] {
  const total = items.length;
  if (total === 0 || index < 0 || index >= total) return [];
  const size = Math.max(1, Math.floor(count));

  const burstId = items[index].burstId;
  const members: number[] = [index];
  if (burstId !== null) {
    const siblings: number[] = [];
    for (let i = 0; i < total; i += 1) {
      if (i !== index && items[i].burstId === burstId) siblings.push(i);
    }
    siblings.sort((a, b) => Math.abs(a - index) - Math.abs(b - index));
    members.push(...siblings);
  }
  // 相邻补位（距离序；等距左侧优先）
  for (let dist = 1; members.length < size && dist < total; dist += 1) {
    for (const i of [index - dist, index + dist]) {
      if (i >= 0 && i < total && !members.includes(i)) members.push(i);
    }
  }
  return members.slice(0, size);
}

// --- V2：连拍组「本组只留这张」批量决定组装 ---------------------------------------------

/**
 * 一键留张的 decision_apply 载荷（一次批量写）：
 * - 当前片 → accepted（已 accepted 则跳过免重写）
 * - 组内其余**未定**项 → rejected（已手动决定过的兄弟不动——尊重手动）
 * 无组 / 无可写项返回 []（调用方 no-op）。
 */
export function buildKeepOnlyDecisions(
  items: ReadonlyArray<{ assetId: number; burstId: number | null }>,
  currentAssetId: number,
  decisions: CullDecisionMap,
): CullDecisionPatch[] {
  const current = items.find((item) => item.assetId === currentAssetId) ?? null;
  if (current === null || current.burstId === null) return [];
  const patches: CullDecisionPatch[] = [];
  if (decisions.get(currentAssetId) !== "accepted") {
    patches.push({ assetId: currentAssetId, decision: "accepted" });
  }
  for (const item of items) {
    if (item.assetId === currentAssetId || item.burstId !== current.burstId) continue;
    if (decisions.get(item.assetId) == null) {
      patches.push({ assetId: item.assetId, decision: "rejected" });
    }
  }
  return patches;
}

// --- V3：AI 挑图规则表单 → 契约 DTO ---------------------------------------------------

/** AI 挑图弹窗受控表单态（数字输入为字符串，未规整） */
export interface CullAiRulesForm {
  eyesEnabled: boolean;
  /** 'weak' | 'normal' | 'strong'（脏值回 normal） */
  eyesSensitivity: string;
  blurEnabled: boolean;
  blurSensitivity: string;
  burstKeepSharpest: boolean;
  /** '' 或数字串；0=关 */
  groupExemptFaces: string;
  /** '' = 不限 */
  maxAccepted: string;
}

function normalizeSensitivity(value: string): CullAiSensitivity {
  return value === "weak" || value === "strong" ? value : "normal";
}

/** 表单态 → cull_ai_prescan 规则载荷（预览/应用同形）：
 *  - 敏感度脏值回 normal
 *  - groupExemptFaces：非法/负数回 0（=关）
 *  - maxAccepted：空/非法/<1 回 null（=不限） */
export function buildAiRules(form: CullAiRulesForm): CullAiRules {
  const intOr = (value: string, fallback: number): number => {
    const parsed = Number.parseInt(value, 10);
    return Number.isFinite(parsed) ? parsed : fallback;
  };
  const exemptRaw = intOr(form.groupExemptFaces.trim(), 0);
  const maxRaw = form.maxAccepted.trim() === "" ? null : intOr(form.maxAccepted.trim(), 0);
  return {
    eyes: { enabled: form.eyesEnabled, sensitivity: normalizeSensitivity(form.eyesSensitivity) },
    blur: { enabled: form.blurEnabled, sensitivity: normalizeSensitivity(form.blurSensitivity) },
    burstKeepSharpest: form.burstKeepSharpest,
    groupExemptFaces: Math.max(0, exemptRaw),
    maxAccepted: maxRaw !== null && maxRaw >= 1 ? maxRaw : null,
  };
}
