import type { ReactNode } from "react";
import { Link } from "react-router";
import { useTranslation } from "react-i18next";

export default function AlbumBreadcrumbs({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <nav aria-label={t("albums.title")} className="flex min-w-0 items-center gap-2 text-sm" data-testid="album-breadcrumbs">
      <Link to="/albums" className="shrink-0 rounded px-1 py-1 text-text-secondary transition-colors hover:bg-panel hover:text-accent focus-visible:outline-accent">
        {t("albums.title")}
      </Link>
      <svg aria-hidden="true" className="shrink-0 text-text-muted" width="14" height="14" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.5"><path d="m7 4 6 6-6 6" /></svg>
      {children}
    </nav>
  );
}
