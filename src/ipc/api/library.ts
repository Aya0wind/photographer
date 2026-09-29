import { ipc } from "../index";
import { type LibraryDeleteResult, type LibraryRelocateResult, type SidebarCounts } from "./types";

export async function libraryDelete(
  dbDir: string,
  photoRoot?: string,
): Promise<LibraryDeleteResult> {
  return ipc<LibraryDeleteResult>("library_delete", {
    dbDir,
    photoRoot: photoRoot ?? null,
  });
}
export async function libraryRelocate(
  libraryId: string,
  newPhotoRoot: string,
  apply: boolean,
): Promise<LibraryRelocateResult> {
  return ipc<LibraryRelocateResult>("library_relocate", {
    libraryId,
    newPhotoRoot,
    apply,
  });
}

export async function sidebarCounts(): Promise<SidebarCounts | null> {
  try {
    const counts = await ipc<SidebarCounts | null>("sidebar_counts");
    if (counts === null || typeof counts !== "object") return null;
    const c = counts as Partial<Record<keyof SidebarCounts, unknown>>;
    const numOf = (v: unknown): number =>
      typeof v === "number" && Number.isFinite(v) ? v : 0;
    return {
      assets: numOf(c.assets),
      recentViewed: numOf(c.recentViewed),
      onThisDay: numOf(c.onThisDay),
      tags: numOf(c.tags),
      albums: numOf(c.albums),
    };
  } catch {
    return null;
  }
}
