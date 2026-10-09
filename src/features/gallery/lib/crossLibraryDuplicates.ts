import { useState } from "react";

import type { AssetDto, DuplicateGroupDto, PhotoLibrary } from "@/ipc/api";

/**
 * 跨库重复折叠（2026-10-09 单库多照片库定案 §四，纯显示层）：
 * - 数据层不去重（同内容不同库 = 独立资产独立 id，合法状态）；「隐藏跨库重复」
 *   开关只做显示过滤——同哈希组（duplicates_list kind=exact：(size, xxhash)
 *   完全相同）只显示一张，隐藏副本不上画廊、不进查看器、不被多选/右键命中
 *   （操作只作用于可见资产）。
 * - 只折叠**跨库**组（≥2 个互不相同的已知 libraryId）；同库重复组（rename
 *   策略遗留的罕见态）不受开关影响。
 * - 组代表优先级（计划定案）：在线 > 有高清 RAW > 评分高 > 导入早。
 *   在线 = 资产未标 missing 且所属照片库 status=online（库状态未知按在线算，
 *   过渡期不误杀）；有高清 RAW = 本体是 RAW 或有 RAW+JPG 配对（pairId）；
 *   导入早 = id 小（自增主键即登记顺序）。
 * - 「重复项」列表可看全部：徽标点击打开组内全部副本（duplicates_list 的
 *   组数据是权威全量，不受画廊分页影响）。
 * - 折叠只在**已加载**成员中选代表：代表随分页加载可能切换，但同组内容完全
 *   相同（缩略图一致），视觉无感；反向做法（等未加载的代表）会在画廊中段
 *   留下永不填充的洞。
 */

/** localStorage 键：隐藏跨库重复开关（默认关，计划定案） */
export const HIDE_CROSS_LIBRARY_DUPLICATES_KEY = "smartphoto.gallery.hideCrossLibraryDuplicates";

export function loadHideCrossLibraryDuplicates(): boolean {
  try {
    return localStorage.getItem(HIDE_CROSS_LIBRARY_DUPLICATES_KEY) === "1";
  } catch {
    return false;
  }
}

export function saveHideCrossLibraryDuplicates(value: boolean): void {
  try {
    localStorage.setItem(HIDE_CROSS_LIBRARY_DUPLICATES_KEY, value ? "1" : "0");
  } catch {
    // 存储不可用时仅内存态生效
  }
}

export function useHideCrossLibraryDuplicates(): [boolean, (next: boolean) => void] {
  const [value, setValue] = useState(loadHideCrossLibraryDuplicates);
  const change = (next: boolean) => {
    setValue(next);
    saveHideCrossLibraryDuplicates(next);
  };
  return [value, change];
}

/** 重复组是否跨库（≥2 个互不相同的已知 libraryId；同库组不受开关影响） */
export function isCrossLibraryGroup(members: AssetDto[]): boolean {
  const known = new Set(
    members.map((m) => m.libraryId).filter((v): v is string => v != null),
  );
  return known.size >= 2;
}

/** 资产是否在线：missing 标记优先，其次照片库状态（未知库按在线，不误杀） */
function isAssetOnline(
  asset: AssetDto,
  libraryById: Map<string, PhotoLibrary> | null,
): boolean {
  if (asset.missing === true) return false;
  if (libraryById === null || asset.libraryId == null) return true;
  const library = libraryById.get(asset.libraryId);
  return library === undefined ? true : library.status === "online";
}

/** 有高清 RAW：本体是 RAW，或有 RAW+JPG 配对（配对的另一侧必为 RAW） */
function hasHighResRaw(asset: AssetDto): boolean {
  return asset.kind === "raw" || asset.pairId != null;
}

/** 组代表比较：在线 > 有高清 RAW > 评分高 > 导入早（id 小）；负值 = a 优先 */
export function compareDuplicateRank(
  a: AssetDto,
  b: AssetDto,
  libraryById: Map<string, PhotoLibrary> | null,
): number {
  const onlineDiff =
    Number(isAssetOnline(a, libraryById)) - Number(isAssetOnline(b, libraryById));
  if (onlineDiff !== 0) return -onlineDiff;
  const rawDiff = Number(hasHighResRaw(a)) - Number(hasHighResRaw(b));
  if (rawDiff !== 0) return -rawDiff;
  const ratingDiff = (a.rating ?? 0) - (b.rating ?? 0);
  if (ratingDiff !== 0) return -ratingDiff;
  return a.id - b.id;
}

export interface DuplicateCollapseResult {
  /** 折叠后的可见资产（保持输入顺序；未命中跨库重复组的原样通过） */
  visible: AssetDto[];
  /** 可见资产 id → 组内副本总数（含可见项；徽标 ×N 用） */
  badges: Map<number, number>;
  /** 可见资产 id → 组内全部副本（重复项弹窗用；全量，权威来自 duplicates_list） */
  membersByVisibleId: Map<number, AssetDto[]>;
}

/** 空 结果（groups 未加载/为空时原样透传） */
const NO_COLLAPSE: DuplicateCollapseResult = {
  visible: [],
  badges: new Map(),
  membersByVisibleId: new Map(),
};

/**
 * 显示层折叠：在已加载资产中，把跨库重复组压成一张代表卡。
 * @param groups duplicates_list(kind=exact) 全量组（null=未加载完，不折叠）
 * @param libraryById 照片库登记表（在线判定；null=状态未知，按在线）
 */
export function collapseCrossLibraryDuplicates(
  assets: AssetDto[],
  groups: DuplicateGroupDto[] | null,
  libraryById: Map<string, PhotoLibrary> | null,
): DuplicateCollapseResult {
  if (groups === null || groups.length === 0 || assets.length === 0) {
    return { ...NO_COLLAPSE, visible: assets };
  }
  /** 跨库重复组成员 id → 组 */
  const crossMembersById = new Map<number, AssetDto[]>();
  for (const group of groups) {
    if (group.assets.length < 2 || !isCrossLibraryGroup(group.assets)) continue;
    for (const member of group.assets) crossMembersById.set(member.id, group.assets);
  }
  if (crossMembersById.size === 0) {
    return { ...NO_COLLAPSE, visible: assets };
  }
  const loadedIds = new Set(assets.map((a) => a.id));
  /** 已选出代表的组（同组只留一张） */
  const representedGroups = new Set<AssetDto[]>();
  const visible: AssetDto[] = [];
  const badges = new Map<number, number>();
  const membersByVisibleId = new Map<number, AssetDto[]>();
  for (const asset of assets) {
    const members = crossMembersById.get(asset.id);
    if (members === undefined) {
      visible.push(asset);
      continue;
    }
    if (representedGroups.has(members)) continue; // 本组代表已出现，隐藏副本
    // 已加载成员中选代表（分页未加载的成员不参与，防画廊中段出洞）
    const loadedMembers = members.filter((m) => loadedIds.has(m.id));
    const representative =
      loadedMembers.length > 0
        ? [...loadedMembers].sort((a, b) => compareDuplicateRank(a, b, libraryById))[0]
        : asset;
    representedGroups.add(members);
    visible.push(representative);
    badges.set(representative.id, members.length);
    membersByVisibleId.set(representative.id, members);
  }
  return { visible, badges, membersByVisibleId };
}
