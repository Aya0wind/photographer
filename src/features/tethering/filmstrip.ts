import { PAIR_BADGE } from "@/features/gallery/lib/mergeRawJpg";
import type { TetherPhoto } from "@/ipc/api";

/**
 * 联机胶片条合并卡：一次快门 = 一张卡。
 *
 * 配对依据 = 文件名 stem（RAW+JPG 一次拍摄产生同 stem 两文件；相机文件计数器
 * 保证会话内 stem 唯一）。不走 pairId——那是扫描/索引管线异步写入的库级配对，
 * 胶片条收片到达时还不可用；拍摄时序分组在这里反而更准。
 *
 * 代表卡取 JPG（缩略图解码稳定），与画廊 mergeRawJpg 同约定；角标如实标注
 * 格式构成：RAW+JPG / RAW / JPG（格式词不翻译，同 PAIR_BADGE 先例）。
 */

export type FilmBadge = "RAW+JPG" | "RAW" | "JPG" | null;

export interface FilmCard {
  /** 代表资产 id（JPG 优先；缩略图与点击预览锚点）。 */
  repId: number;
  /** 成员资产 id（组内全部；含 RAW 成员）。 */
  memberIds: number[];
  /** 格式构成角标；未知格式为 null。 */
  badge: FilmBadge;
  /** tooltip：成员文件名（合并卡列出两个）。 */
  title: string;
}

function stemOf(name: string): string {
  const dot = name.lastIndexOf(".");
  return (dot > 0 ? name.slice(0, dot) : name).toLowerCase();
}

export function filmCards(photos: TetherPhoto[]): FilmCard[] {
  const groups = new Map<string, TetherPhoto[]>();
  for (const photo of photos) {
    const stem = stemOf(photo.name);
    const list = groups.get(stem);
    if (list) list.push(photo);
    else groups.set(stem, [photo]);
  }
  const cards: FilmCard[] = [];
  for (const members of groups.values()) {
    const hasRaw = members.some((m) => m.kind === "raw");
    const hasPhoto = members.some((m) => m.kind === "photo");
    const rep = members.find((m) => m.kind === "photo") ?? members[0];
    cards.push({
      repId: rep.id,
      memberIds: members.map((m) => m.id),
      badge: hasRaw && hasPhoto ? PAIR_BADGE : hasRaw ? "RAW" : hasPhoto ? "JPG" : null,
      title: members.map((m) => m.name).join(" + "),
    });
  }
  return cards;
}
