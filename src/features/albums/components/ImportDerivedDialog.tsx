import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { convertFileSrc } from "@tauri-apps/api/core";

import {
  lrExportImport,
  lrExportScan,
  type LrExportCandidate,
  type LrExportMatch,
  type LrImportResult,
} from "@/ipc/api";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import { formatBytes } from "@/lib/format";

/**
 * 「导入成片到相册」对话框（B4 子分组模型，相册详情页工具条入口）：
 * - 选目录（系统目录选择器）→ lr_export_scan（已入库文件后端按指纹排除）
 * - 候选审核：每文件一行——成片预览/文件名/大小 + 候选原片列表（默认最高分，
 *   可改选或标「待关联」= v1 跳过）；basis 徽标（文件名/拍摄时间/相似度）
 * - 目标子分组：默认「成片」，可改/从已有选择（datalist；留空 = 相册根）
 * - 确认 → lr_export_import(matches, albumId, subgroup) → 结果摘要
 *   （imported/skipped/failed 逐条原因）+ 失败重试（只重放失败子集）
 */

/** basis JSON 解析（{bases: string[], time_delta_ms?, phash_hamming?}；脏数据容错） */
interface BasisInfo {
  bases: string[];
}

function parseBasis(basis: string): BasisInfo {
  if (basis === "") return { bases: [] };
  try {
    const o = JSON.parse(basis) as Record<string, unknown>;
    return {
      bases: Array.isArray(o.bases) ? o.bases.filter((b): b is string => typeof b === "string") : [],
    };
  } catch {
    return { bases: [] };
  }
}

/** basis 通道 → 中文徽标词 */
const BASIS_LABEL_KEYS: Record<string, string> = {
  filename: "lr.row.basis.filename",
  exif_time: "lr.row.basis.exif_time",
  phash: "lr.row.basis.phash",
  phash_score: "lr.row.basis.phash",
};

function basename(path: string): string {
  const norm = path.replace(/[\\/]+$/, "");
  const idx = Math.max(norm.lastIndexOf("\\"), norm.lastIndexOf("/"));
  return idx >= 0 ? norm.slice(idx + 1) : norm;
}

/** 成片预览（本地图 asset 协议；非 Tauri 环境回 null） */
function previewSrcOf(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

const DEFAULT_SUBGROUP = "成片";

export default function ImportDerivedDialog({
  albumId,
  albumName,
  subgroups,
  onClose,
  onImported,
}: {
  albumId: number;
  albumName: string;
  /** 相册已有子分组名（目标输入 datalist 提示） */
  subgroups: string[];
  onClose: () => void;
  /** 有成片入库后回调（父层刷新子分组清单/网格/计数） */
  onImported?: () => void;
}) {
  const { t } = useTranslation();
  const [dir, setDir] = useState("");
  const [scanning, setScanning] = useState(false);
  const [rows, setRows] = useState<LrExportCandidate[]>([]);
  /** path → 选中候选原片 assetId；null = 待关联（跳过导入） */
  const [selections, setSelections] = useState<Record<string, number | null>>({});
  const [subgroup, setSubgroup] = useState(DEFAULT_SUBGROUP);
  const [importing, setImporting] = useState(false);
  const [importError, setImportError] = useState<string | null>(null);
  const [result, setResult] = useState<LrImportResult | null>(null);
  const [previewBroken, setPreviewBroken] = useState<Record<string, boolean>>({});
  /** 有过成功导入（通知父层刷新一次即可，重试不再重复触发） */
  const importedOnceRef = useRef(false);

  // Esc 关闭
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const scan = useCallback(async (target: string): Promise<void> => {
    if (target === "") return;
    setScanning(true);
    setRows([]);
    const list = await lrExportScan(target);
    setScanning(false);
    setRows(list);
    const next: Record<string, number | null> = {};
    for (const row of list) {
      // 默认选最高分（后端已按 score 降序）
      next[row.path] = row.candidates.length > 0 ? row.candidates[0].assetId : null;
    }
    setSelections(next);
  }, []);

  async function browse(): Promise<void> {
    try {
      const picked = await openDialog({ directory: true, multiple: false, title: t("lr.pick.title") });
      if (typeof picked === "string" && picked.length > 0) {
        setDir(picked);
        void scan(picked);
      }
    } catch {
      // 非 Tauri 环境或用户取消：静默
    }
  }

  function selectCandidate(path: string, match: LrExportMatch | null): void {
    setSelections((prev) => ({ ...prev, [path]: match === null ? null : match.assetId }));
  }

  /** 将导入的匹配清单（待关联行天然排除） */
  const matches = useMemo(() => {
    const byPath = new Map(rows.map((r) => [r.path, r]));
    const out: Array<{ path: string; sourceAssetId: number; basis?: string }> = [];
    for (const row of rows) {
      const selectedId = selections[row.path];
      if (selectedId === undefined || selectedId === null) continue;
      const match = byPath.get(row.path)?.candidates.find((c) => c.assetId === selectedId);
      if (!match) continue;
      out.push({ path: row.path, sourceAssetId: selectedId, basis: match.basis });
    }
    return out;
  }, [rows, selections]);

  const pendingCount = rows.length - matches.length;

  async function confirmImport(): Promise<void> {
    if (importing || matches.length === 0) return;
    setImporting(true);
    setImportError(null);
    const imported = await lrExportImport(matches, albumId, subgroup);
    setImporting(false);
    if (imported === null) {
      setImportError(t("lr.bar.importFailed"));
      return;
    }
    if (imported.imported > 0 && !importedOnceRef.current) {
      importedOnceRef.current = true;
      onImported?.();
    }
    setResult(imported);
  }

  /** 失败重试：仅重放失败子集 */
  async function retryFailed(): Promise<void> {
    if (result === null || result.failed.length === 0) return;
    setImporting(true);
    setImportError(null);
    const failedPaths = new Set(result.failed.map((f) => f.path));
    const payload = matches.filter((m) => failedPaths.has(m.path));
    const retried = await lrExportImport(payload, albumId, subgroup);
    setImporting(false);
    if (retried === null) {
      setImportError(t("lr.bar.importFailed"));
      return;
    }
    setResult((prev) => {
      if (prev === null) return retried;
      return {
        imported: prev.imported + retried.imported,
        skipped: prev.skipped + retried.skipped,
        failed: retried.failed,
      };
    });
  }

  return (
    <div
      className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
      onClick={onClose}
      data-testid="import-derived-overlay"
    >
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("albums.importDerivedTitle", { name: albumName })}
        className="flex max-h-[85vh] w-[720px] max-w-[92vw] flex-col overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
        onClick={(e) => e.stopPropagation()}
        data-testid="import-derived-dialog"
      >
        {/* 头部 */}
        <div className="flex shrink-0 items-center justify-between border-b border-edge px-4 py-3">
          <h2 className="text-sm font-semibold text-text-primary">
            {t("albums.importDerivedTitle", { name: albumName })}
          </h2>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("common.close")}
            className="rounded px-1.5 text-lg leading-none text-text-muted transition-colors hover:text-text-primary"
            data-testid="import-derived-close"
          >
            ×
          </button>
        </div>

        {/* 目录行 */}
        <div className="flex shrink-0 items-center gap-2.5 border-b border-edge/60 px-4 py-2" data-testid="lr-pick-row">
          <button
            type="button"
            onClick={() => void browse()}
            className="shrink-0 rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
            data-testid="lr-pick-button"
          >
            {t("lr.pick.browse")}
          </button>
          {dir !== "" && (
            <>
              <span className="min-w-0 flex-1 truncate font-mono text-[11px] text-text-secondary" title={dir} data-testid="lr-dir-text">
                {dir}
              </span>
              <button
                type="button"
                onClick={() => void scan(dir)}
                disabled={scanning}
                className="shrink-0 rounded-md border border-edge px-2.5 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:opacity-50"
                data-testid="lr-rescan"
              >
                {scanning ? t("lr.pick.scanning") : t("lr.pick.rescan")}
              </button>
            </>
          )}
        </div>

        {/* 内容区：审核行 / 空态 / 结果摘要 */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-4 py-3">
          {result !== null ? (
            <div data-testid="lr-result">
              <p className="text-sm font-semibold text-text-primary">{t("lr.result.title")}</p>
              <div className="mt-3 flex gap-2" data-testid="lr-result-stats">
                <span className="rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-text-secondary" data-testid="lr-result-imported">
                  {t("lr.result.imported", { count: result.imported })}
                </span>
                <span className="rounded-md border border-edge bg-surface px-2.5 py-1.5 text-xs text-text-secondary" data-testid="lr-result-skipped">
                  {t("lr.result.skipped", { count: result.skipped })}
                </span>
                <span
                  className={`rounded-md border px-2.5 py-1.5 text-xs ${
                    result.failed.length > 0
                      ? "border-red-400/60 bg-red-400/10 text-red-400"
                      : "border-edge bg-surface text-text-secondary"
                  }`}
                  data-testid="lr-result-failed"
                >
                  {t("lr.result.failed", { count: result.failed.length })}
                </span>
              </div>
              {result.failed.length > 0 && (
                <ul className="mt-3 flex flex-col gap-1.5" data-testid="lr-result-failures">
                  {result.failed.map((f) => (
                    <li key={f.path} className="rounded-md border border-edge bg-surface px-3 py-2">
                      <p className="truncate font-mono text-[11px] text-text-secondary" title={f.path}>
                        {basename(f.path)}
                      </p>
                      <p className="mt-0.5 text-[11px] text-red-400">{f.error}</p>
                    </li>
                  ))}
                </ul>
              )}
              {result.failed.length > 0 && (
                <button
                  type="button"
                  onClick={() => void retryFailed()}
                  disabled={importing}
                  className="mt-4 rounded-md border border-accent/60 bg-accent/10 px-3 py-1.5 text-xs font-medium text-accent transition-colors hover:bg-accent/20 disabled:opacity-50"
                  data-testid="lr-result-retry"
                >
                  {importing ? t("lr.bar.importing") : t("lr.result.retry")}
                </button>
              )}
            </div>
          ) : rows.length === 0 ? (
            <div className="flex min-h-32 flex-col items-center justify-center gap-2 py-8 text-center" data-testid="lr-scan-empty">
              <p className="text-sm text-text-secondary">
                {dir === "" ? t("lr.pick.hint") : scanning ? t("lr.pick.scanning") : t("lr.pick.empty")}
              </p>
              {dir !== "" && !scanning && (
                <p className="max-w-sm text-xs leading-relaxed text-text-muted">{t("lr.pick.emptyHint")}</p>
              )}
            </div>
          ) : (
            <div className="flex flex-col gap-2" data-testid="lr-rows">
              {rows.map((row) => {
                const selectedId = selections[row.path] ?? null;
                const isPending = selectedId === null;
                const src = previewSrcOf(row.path);
                return (
                  <div
                    key={row.path}
                    className="flex items-stretch gap-3 rounded-lg border border-edge bg-surface p-2"
                    data-testid="lr-row"
                    data-path={row.path}
                    data-pending={isPending ? "true" : "false"}
                  >
                    <div className="flex w-52 shrink-0 items-center gap-2.5">
                      <div className="h-12 w-12 shrink-0 overflow-hidden rounded-md bg-panel/50" data-testid="lr-row-preview">
                        {src !== null && !previewBroken[row.path] ? (
                          <img
                            src={src}
                            alt=""
                            className="h-full w-full object-cover"
                            onError={() => setPreviewBroken((prev) => ({ ...prev, [row.path]: true }))}
                            draggable={false}
                          />
                        ) : (
                          <span className="flex h-full w-full items-center justify-center font-mono text-[10px] text-text-muted">JPG</span>
                        )}
                      </div>
                      <div className="min-w-0">
                        <p className="truncate text-xs text-text-primary" title={row.path} data-testid="lr-row-name">
                          {basename(row.path)}
                        </p>
                        <p className="font-mono text-[10px] tabular-nums text-text-muted" data-testid="lr-row-size">
                          {formatBytes(row.size)}
                        </p>
                      </div>
                    </div>

                    <div className="flex min-w-0 flex-1 flex-col justify-center gap-1">
                      <p className="text-[10px] font-medium uppercase tracking-wider text-text-muted">
                        {t("lr.row.matchTitle")}
                      </p>
                      {row.candidates.length === 0 ? (
                        <p className="text-[11px] text-text-muted" data-testid="lr-row-no-candidate">
                          {t("lr.row.unmatchedDesc")}
                        </p>
                      ) : (
                        <div className="flex flex-wrap items-center gap-1.5">
                          {row.candidates.map((candidate) => {
                            const selected = selectedId === candidate.assetId;
                            const info = parseBasis(candidate.basis);
                            const basisLabel = info.bases
                              .map((b) => BASIS_LABEL_KEYS[b] ?? null)
                              .filter((k): k is string => k !== null)
                              .map((k) => t(k))
                              .join(" · ");
                            return (
                              <button
                                key={candidate.assetId}
                                type="button"
                                aria-pressed={selected}
                                onClick={() => selectCandidate(row.path, candidate)}
                                className={`flex items-center gap-1.5 rounded-md border px-2 py-1 text-left transition-colors ${
                                  selected ? "border-accent bg-accent/10" : "border-edge hover:border-text-muted"
                                }`}
                                data-testid="lr-candidate"
                                data-asset-id={candidate.assetId}
                                data-selected={selected ? "true" : "false"}
                              >
                                <AssetThumb
                                  asset={{ id: candidate.assetId, kind: "photo", name: candidate.name }}
                                  size={64}
                                  skeleton={false}
                                  className="h-7 w-7 shrink-0 rounded"
                                  testId="lr-candidate-thumb"
                                />
                                <span className="min-w-0">
                                  <span className="block max-w-[140px] truncate font-mono text-[10px] text-text-secondary" title={candidate.name}>
                                    {candidate.name}
                                  </span>
                                  <span className="flex items-center gap-1">
                                    {basisLabel !== "" && (
                                      <span className="rounded bg-panel px-1 font-mono text-[9px] leading-4 text-text-muted" data-testid="lr-basis-badge">
                                        {basisLabel}
                                      </span>
                                    )}
                                    <span className="font-mono text-[9px] leading-4 text-accent">
                                      {t("lr.row.score", { score: candidate.score })}
                                    </span>
                                  </span>
                                </span>
                              </button>
                            );
                          })}
                          <button
                            type="button"
                            aria-pressed={isPending}
                            onClick={() => selectCandidate(row.path, null)}
                            className={`rounded-md border px-2 py-1 text-[11px] transition-colors ${
                              isPending
                                ? "border-amber-400 bg-amber-400/10 text-amber-400"
                                : "border-edge text-text-muted hover:border-amber-400 hover:text-amber-400"
                            }`}
                            data-testid="lr-row-unmatched"
                          >
                            {t("lr.row.unmatched")}
                          </button>
                        </div>
                      )}
                    </div>

                    {isPending && (
                      <span className="flex shrink-0 items-center rounded-full bg-amber-400/10 px-2 text-[10px] text-amber-400" data-testid="lr-row-status">
                        {t("lr.row.unmatched")}
                      </span>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        {/* 底部：目标子分组 + 统计 + 确认（结果阶段换成完成钮） */}
        <div className="flex shrink-0 flex-wrap items-center gap-2.5 border-t border-edge px-4 py-2.5" data-testid="lr-bar">
          {result === null && (
            <>
              <span className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary" data-testid="lr-stats-import">
                {t("lr.bar.willImport", { count: matches.length })}
              </span>
              <span className="shrink-0 font-mono text-[11px] tabular-nums text-amber-400" data-testid="lr-stats-pending">
                {t("lr.bar.pending", { count: pendingCount })}
              </span>
              <label className="ml-auto flex shrink-0 items-center gap-1.5 text-[11px] text-text-muted">
                {t("lr.subgroupLabel")}
                <input
                  type="text"
                  value={subgroup}
                  onChange={(e) => setSubgroup(e.target.value)}
                  list="import-derived-subgroup-options"
                  placeholder={t("lr.subgroupPlaceholder")}
                  aria-label={t("lr.subgroupLabel")}
                  className="h-7 w-36 rounded-md border border-edge bg-bg px-2 text-[11px] text-text-primary outline-none transition-colors placeholder:text-text-muted/60 focus:border-accent"
                  data-testid="lr-subgroup-input"
                />
                <datalist id="import-derived-subgroup-options">
                  {subgroups.map((n) => (
                    <option key={n} value={n} />
                  ))}
                </datalist>
              </label>
              <button
                type="button"
                onClick={() => void confirmImport()}
                disabled={importing || matches.length === 0}
                className="shrink-0 rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="lr-confirm"
              >
                {importing ? t("lr.bar.importing") : t("lr.bar.confirm", { count: matches.length })}
              </button>
              {importError !== null && (
                <span className="text-[11px] text-red-400" role="alert" data-testid="lr-import-error">
                  {importError}
                </span>
              )}
            </>
          )}
          {result !== null && (
            <button
              type="button"
              onClick={onClose}
              className="ml-auto rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-text-muted hover:text-text-primary"
              data-testid="import-derived-done"
            >
              {t("common.done")}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
