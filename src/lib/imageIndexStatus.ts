import type { IndexCounters, IndexStatus } from "@/ipc/api/types";

/** Backend is authoritative. The fallback supports older backend snapshots;
 * separate legacy counts cannot reconstruct the union of completed photos. */
export function imageIndexCounters(status: IndexStatus): IndexCounters {
  if (status.image) return status.image;
  const parts = [status.thumb, status.exif];
  const total = Math.max(...parts.map(p => p.total));
  const done = Math.min(total, ...parts.map(p => p.done));
  const running = Math.min(total - done, Math.max(...parts.map(p => p.running)));
  const failed = Math.min(total - done - running, Math.max(...parts.map(p => p.failed)));
  return { total, done, running, failed, pending: Math.max(0, total - done - running - failed) };
}
