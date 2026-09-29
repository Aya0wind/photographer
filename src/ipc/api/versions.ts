import { ipc } from "../index";
import { type AssetVersions, type VersionMember } from "./types";

/** 资产版本查询（查看器版本切换数据源；失败/负载异常返回 null） */
export async function assetVersions(assetId: number): Promise<AssetVersions | null> {
  try {
    const raw = await ipc<unknown>("asset_versions", { assetId });
    if (raw === null || typeof raw !== "object") return null;
    const r = raw as { groupId?: unknown; members?: unknown };
    if (!Array.isArray(r.members)) return null;
    const members: VersionMember[] = [];
    for (const m of r.members) {
      if (m === null || typeof m !== "object") continue;
      const mm = m as Record<string, unknown>;
      if (typeof mm.assetId !== "number" || typeof mm.name !== "string") continue;
      const role =
        mm.role === "raw" || mm.role === "sooc" || mm.role === "derived" ? mm.role : null;
      members.push({
        assetId: mm.assetId,
        role,
        name: mm.name,
        thumbReady: mm.thumbReady === true,
      });
    }
    const groupId = typeof r.groupId === "number" && Number.isFinite(r.groupId) ? r.groupId : null;
    return { groupId, members };
  } catch {
    return null;
  }
}
