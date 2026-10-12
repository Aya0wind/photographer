import { useEffect, useState } from "react";
import { useLocation } from "react-router";
import { useTranslation } from "react-i18next";

export default function PhotosAddedNotice() {
  const location = useLocation();
  const { t } = useTranslation();
  const message = typeof location.state?.photoAddNotice === "string" ? location.state.photoAddNotice : null;
  const [dismissed, setDismissed] = useState(false);
  useEffect(() => {
    setDismissed(false);
    const timer = window.setTimeout(() => setDismissed(true), 6000);
    return () => window.clearTimeout(timer);
  }, [location.key]);
  if (!message || dismissed) return null;
  return <div role="status" className="absolute left-1/2 top-3 z-30 flex max-w-[90%] -translate-x-1/2 items-center gap-3 rounded-xl border border-accent/30 bg-surface px-4 py-3 text-xs text-text-primary shadow-xl" data-testid="photos-added-notice">
    <span>{message}</span><button type="button" onClick={() => setDismissed(true)} aria-label={t("ui.close")} className="text-text-secondary hover:text-text-primary">×</button>
  </div>;
}
