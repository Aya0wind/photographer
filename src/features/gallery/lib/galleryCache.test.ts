import { assetFixture } from "@/test/fixtures";
import { beforeEach, describe, expect, it } from "vitest";

import type { AssetDto } from "@/ipc/api";
import {
  clearGallerySnapshotForTests,
  gallerySnapshot,
  saveGallerySnapshot,
} from "./galleryCache";

/** 最小 AssetDto 夹具 */
function asset(id: number): AssetDto {
  return assetFixture(id, {
    path: `X:/p/${id}.jpg`,
    name: `${id}.jpg`,
    capturedAt: "2026-09-19T10:00:00.000Z",
    sizeBytes: 100,
  });
}

describe("galleryCache：画廊会话快照", () => {
  beforeEach(() => {
    clearGallerySnapshotForTests();
  });

  it("无快照 → null（首屏走常规 loading）", () => {
    expect(gallerySnapshot()).toBeNull();
  });

  it("保存后可取回（assets/dates/hasMore/scrollTop 全量）", () => {
    saveGallerySnapshot({
      assets: [asset(1), asset(2)],
      dates: [{ date: "2026-09-19", count: 2, coverAssetId: 1 }],
      hasMore: false,
      scrollTop: 480,
      savedAt: 1726700000000,
    });
    const snap = gallerySnapshot();
    expect(snap).not.toBeNull();
    expect(snap?.assets.map((a) => a.id)).toEqual([1, 2]);
    expect(snap?.dates).toEqual([{ date: "2026-09-19", count: 2, coverAssetId: 1 }]);
    expect(snap?.hasMore).toBe(false);
    expect(snap?.scrollTop).toBe(480);
  });

  it("重复保存为覆盖语义（最新快照生效）", () => {
    saveGallerySnapshot({
      assets: [asset(1)],
      dates: [],
      hasMore: true,
      scrollTop: 0,
      savedAt: 1,
    });
    saveGallerySnapshot({
      assets: [asset(1), asset(2), asset(3)],
      dates: [],
      hasMore: false,
      scrollTop: 960,
      savedAt: 2,
    });
    expect(gallerySnapshot()?.assets).toHaveLength(3);
    expect(gallerySnapshot()?.scrollTop).toBe(960);
  });

  it("clearGallerySnapshotForTests 后回到 null", () => {
    saveGallerySnapshot({
      assets: [asset(1)],
      dates: [],
      hasMore: false,
      scrollTop: 0,
      savedAt: 1,
    });
    clearGallerySnapshotForTests();
    expect(gallerySnapshot()).toBeNull();
  });
});
