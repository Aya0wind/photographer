import { useMemo } from "react";

import type { AssetDto } from "@/ipc/api";
import { useSettingsStore } from "@/stores/settingsStore";
import { mergeRawJpgCards } from "./mergeRawJpg";

/** 各照片列表统一遵循 RAW+JPG 合并设置。 */
export function usePhotoCards(assets: AssetDto[]) {
  const enabled = useSettingsStore((s) => s.settings.gallery?.mergeRawJpg ?? true);
  return useMemo(() => mergeRawJpgCards(assets, enabled), [assets, enabled]);
}
