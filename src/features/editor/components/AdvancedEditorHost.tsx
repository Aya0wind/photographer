import { lazy, Suspense } from "react";
import { createPortal } from "react-dom";
import { useAdvancedEditorStore } from "../lib/advancedEditorStore";

const AdvancedEditorOverlay = lazy(() => import("./AdvancedEditorOverlay"));

export default function AdvancedEditorHost() {
  const current = useAdvancedEditorStore((s) => s.current);
  if (!current) return null;
  return createPortal(<Suspense fallback={null}><AdvancedEditorOverlay key={`${current.libraryId}:${current.asset.id}`} asset={current.asset} libraryId={current.libraryId} originAlbumId={current.albumId} originSubgroup={current.subgroup} onClose={useAdvancedEditorStore.getState().close} /></Suspense>, document.body);
}
