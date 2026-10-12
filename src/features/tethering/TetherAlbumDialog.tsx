import { useState } from "react";
import { useTranslation } from "react-i18next";
import { albumCreate, type AlbumDto } from "@/ipc/api";
import TetherStartDialog from "./TetherStartDialog";
import ErrorModal from "@/shared/components/ErrorModal";

export default function TetherAlbumDialog({ albums, onCreated, onClose }: {
  albums: AlbumDto[]; onCreated: (album: AlbumDto) => void; onClose: () => void;
}) {
  const { t } = useTranslation();
  const [choice, setChoice] = useState("");
  const [name, setName] = useState("");
  const [target, setTarget] = useState<AlbumDto | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  async function proceed() {
    if (busy) return;
    if (choice !== "new") {
      const album = albums.find(item => String(item.id) === choice);
      if (album) setTarget(album);
      return;
    }
    if (!name.trim()) return;
    setBusy(true);
    try {
      const result = await albumCreate(name.trim());
      if (!result.ok) { setError(result.error ?? t("albums.createFailed")); return; }
      onCreated(result.album);
      setTarget(result.album);
    } catch (err) { setError(err instanceof Error ? err.message : t("albums.createFailed")); }
    finally { setBusy(false); }
  }
  if (target) return <TetherStartDialog albumId={target.id} albumName={target.name} onClose={onClose} />;
  return <div className="fixed inset-0 z-[80] flex items-center justify-center bg-black/55 p-6" role="dialog" aria-modal="true" aria-label={t("albums.tetherAlbumTitle")} data-testid="tether-album-dialog">
    <div className="w-full max-w-md rounded-xl border border-edge bg-surface p-5 shadow-2xl">
      <h2 className="text-sm font-semibold">{t("albums.tetherAlbumTitle")}</h2>
      <p className="mt-2 text-xs leading-relaxed text-text-muted">{t("albums.tetherAlbumHint")}</p>
      <select autoFocus disabled={busy} value={choice} onChange={event => setChoice(event.target.value)} aria-label={t("albums.tetherAlbumTitle")} data-testid="tether-album-select" className="mt-4 w-full rounded-md border border-edge bg-bg px-3 py-2 text-sm">
        <option value="">{t("albums.tetherAlbumChoose")}</option>
        {albums.map(album => <option key={album.id} value={album.id}>{album.name}</option>)}
        <option value="new">＋ {t("albums.createAlbum")}</option>
      </select>
      {choice === "new" && <input disabled={busy} value={name} onChange={event => setName(event.target.value)} aria-label={t("albums.newNamePlaceholder")} placeholder={t("albums.newNamePlaceholder")} data-testid="tether-album-name" className="mt-3 w-full rounded-md border border-edge bg-bg px-3 py-2 text-sm" />}
      <div className="mt-5 flex justify-end gap-2">
        <button disabled={busy} onClick={onClose} className="rounded-md border border-edge px-3 py-2 text-xs">{t("common.cancel")}</button>
        <button disabled={busy || !choice || (choice === "new" && !name.trim())} onClick={() => void proceed()} data-testid="tether-album-next" className="rounded-md bg-accent px-4 py-2 text-xs font-semibold text-black disabled:opacity-40">{t("albums.tetherAlbumNext")}</button>
      </div>
    </div>
    <ErrorModal message={error} onClose={() => setError(null)} />
  </div>;
}
