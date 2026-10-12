import { useEffect, useMemo, useState } from "react";
import { duplicatesList, photoLibraryList, subscribeAppEvents, type AssetDto, type DuplicateGroupDto, type PhotoLibrary } from "@/ipc/api";
import { collapseCrossLibraryDuplicates, useHideCrossLibraryDuplicates } from "./crossLibraryDuplicates";

/** Pure display folding: scoped cards stay actionable; hidden copies are never selected. */
export function useCrossLibraryDuplicates(assets: AssetDto[], revision: number) {
  const hidden = useHideCrossLibraryDuplicates();
  const [libraries, setLibraries] = useState<PhotoLibrary[] | null>(null);
  const [groups, setGroups] = useState<DuplicateGroupDto[] | null>(null);
  useEffect(() => {
    if (!hidden) { setGroups(null); return; }
    let cancelled = false;
    let off: (() => void) | null = null;
    let generation = 0;
    async function pull() {
      const request = ++generation;
      const libs = await photoLibraryList();
      const collected: DuplicateGroupDto[] = [];
      for (let after = 0; ; ) {
        const page = await duplicatesList("exact", after, 100);
        if (cancelled || request !== generation) return;
        collected.push(...page);
        if (page.length < 100) break;
        after += page.length;
      }
      if (!cancelled && request === generation) { setLibraries(libs); setGroups(collected); }
    }
    void pull().catch(() => {});
    void subscribeAppEvents(event => {
      if (["photoLibrariesChanged", "importSessionFinished", "libraryScanFinished"].includes(event.type)) void pull().catch(() => {});
    }).then(unlisten => { if (cancelled) unlisten(); else off = unlisten; }).catch(() => {});
    return () => { cancelled = true; off?.(); };
  }, [hidden, revision]);
  const libraryById = useMemo(() => libraries ? new Map(libraries.map(lib => [lib.id, lib])) : null, [libraries]);
  const collapse = useMemo(() => hidden ? collapseCrossLibraryDuplicates(assets, groups, libraryById) : null, [hidden, assets, groups, libraryById]);
  return { hidden, libraries, collapse, assets: collapse?.visible ?? assets };
}
