import { tierFaceIds, tierSemanticIds } from "@/features/settings/lib/qualityTier";
import type { OnboardingDraft } from "./types";

export function aiSetupPackages(draft: Pick<OnboardingDraft, "aiChoice" | "qualityTier">) {
  if (draft.aiChoice === "none") return [];
  return [
    { feature: "semantic", ids: tierSemanticIds(draft.qualityTier) },
    ...(draft.aiChoice === "all" ? [
      { feature: "face", ids: tierFaceIds(draft.qualityTier) },
      { feature: "selection", ids: ["facemesh"] },
    ] : []),
  ];
}
