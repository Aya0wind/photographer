/** Browser-side platform check used only for native window chrome decisions. */
import { isTauri } from "@tauri-apps/api/core";

export function isMacPlatform(): boolean {
  return (
    isTauri() &&
    typeof navigator !== "undefined" &&
    /Macintosh|Mac OS X/i.test(navigator.userAgent)
  );
}
