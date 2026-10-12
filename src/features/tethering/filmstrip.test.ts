import { describe, expect, it } from "vitest";

import type { TetherPhoto } from "@/ipc/api";
import { filmCards } from "./filmstrip";

function photo(id: number, name: string, kind: string): TetherPhoto {
  return { id, name, kind };
}

describe("filmCards 胶片条合并卡", () => {
  it("RAW+JPG 同 stem 合一张卡：代表 = JPG，角标 RAW+JPG，成员齐全", () => {
    const cards = filmCards([
      photo(1, "DSC_0001.NEF", "raw"),
      photo(2, "DSC_0001.JPG", "photo"),
    ]);
    expect(cards).toHaveLength(1);
    expect(cards[0]).toEqual({
      repId: 2,
      memberIds: [1, 2],
      badge: "RAW+JPG",
      title: "DSC_0001.NEF + DSC_0001.JPG",
    });
  });

  it("stem 大小写不敏感（相机命名风格差异不拆卡）", () => {
    expect(
      filmCards([photo(1, "dsc_0001.nef", "raw"), photo(2, "DSC_0001.JPG", "photo")]),
    ).toHaveLength(1);
  });

  it("单 RAW / 单 JPG 如实标注；不同 stem 不合并", () => {
    const cards = filmCards([
      photo(1, "DSC_0001.NEF", "raw"),
      photo(2, "DSC_0002.JPG", "photo"),
    ]);
    expect(cards.map((c) => [c.repId, c.badge])).toEqual([[1, "RAW"], [2, "JPG"]]);
  });

  it("无扩展名文件名整名作 stem；未知 kind 无角标但仍出卡", () => {
    const cards = filmCards([photo(7, "DSC_0003", "other")]);
    expect(cards).toHaveLength(1);
    expect(cards[0].badge).toBeNull();
    expect(cards[0].repId).toBe(7);
  });

  it("全 RAW 同 stem 不标合并（缺 JPG）；代表回退首个成员", () => {
    const cards = filmCards([photo(1, "DSC_0001.NEF", "raw"), photo(2, "DSC_0001.DNG", "raw")]);
    expect(cards).toHaveLength(1);
    expect(cards[0].repId).toBe(1);
    expect(cards[0].badge).toBe("RAW");
  });

  it("保持到达顺序（首次出现的 stem 位置）", () => {
    const cards = filmCards([
      photo(1, "A.JPG", "photo"),
      photo(2, "B.NEF", "raw"),
      photo(3, "A.NEF", "raw"),
    ]);
    expect(cards.map((c) => c.repId)).toEqual([1, 2]);
  });
});
