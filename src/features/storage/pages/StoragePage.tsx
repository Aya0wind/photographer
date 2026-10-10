import ErrorModal from "@/shared/components/ErrorModal";
import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import {
  openWithSystem,
  photoLibraryList,
  type AppEvent,
  type PhotoLibrary,
  subscribeAppEvents,
} from "@/ipc/api";
import { formatBytes, formatDateTime } from "@/lib/format";
import CreateLibraryDialog from "@/features/storage/components/CreateLibraryDialog";
import RemoveLibraryDialog from "@/features/storage/components/RemoveLibraryDialog";
import {
  useLibraryScan,
  type LibraryScanFinishSummary,
  type LibraryScanState,
} from "@/features/storage/lib/useLibraryScan";

/**
 * 存储页（M3，2026-10-09 单库多照片库定案 §七）：照片库（= 一个文件夹的
 * 登记项）卡片列表——名称/路径/在线状态/照片数/容量，以及登记管理操作：
 * - 新建照片库 / 从文件夹建立：登记入口（CreateLibraryDialog；从文件夹建立
 *   触发的后台递归扫描在卡片上呈现进度/取消，收尾 toast 通知——M4f）。
 * - 在系统中打开：open_with_system 打开库根文件夹。
 * - 移除登记：确认对话框问是否连库内记录一起删，**永不删照片文件**（红线）。
 * 列表数据 photo_library_list：挂载拉一次 + photoLibrariesChanged 事件重拉
 * （新建/移除在后端都会发）；后端不可用自然降级空态。
 * 扫描状态（M4f）：useLibraryScan 汇聚 photo_library_scan_status +
 * libraryScanProgress/Finished 事件——扫描中显示进度并可取消；最近同步
 * 时间会话内记录（重启后未知显示「—」，2026-10-09 定案不进契约）。
 */

/** 卡片扫描状态行（M4f）：扫描中（running/paused/failed）显示进度/暂停/失败
 *  文案 + 取消按钮（软信号，当前目录登记完即停）；空闲显示最近同步时间
 *  （会话内 libraryScanFinished 记录，未知显示「—」，2026-10-09 定案）。 */
function ScanStatusRow({
  scan,
  lastSyncAt,
  onCancel,
}: {
  scan: LibraryScanState | undefined;
  lastSyncAt: number | undefined;
  onCancel: () => void;
}) {
  const { t } = useTranslation();

  if (scan === undefined || scan.status === "done" || scan.status === "idle") {
    return (
      <p
        className="mt-1 text-[11px] tabular-nums text-text-muted"
        data-testid="storage-scan-last-sync"
      >
        {t("storage.scan.lastSync", {
          time: lastSyncAt === undefined ? "—" : formatDateTime(lastSyncAt),
        })}
      </p>
    );
  }

  const pct =
    scan.status === "running" && scan.total > 0
      ? Math.min(100, Math.round((scan.registered / scan.total) * 100))
      : 0;

  return (
    <div
      className="mt-2 flex items-center gap-2"
      data-testid="storage-scan-progress"
      data-scan-status={scan.status}
    >
      {scan.status === "running" && (
        <div
          className="h-1 w-24 shrink-0 overflow-hidden rounded-full bg-edge/70"
          data-testid="storage-scan-bar"
        >
          <div
            className="h-full rounded-full bg-accent"
            style={{ width: `${pct}%` }}
            aria-hidden="true"
          />
        </div>
      )}
      <span className="min-w-0 truncate text-[11px] tabular-nums text-text-secondary">
        {scan.status === "running" &&
          t("storage.scan.scanning", {
            registered: scan.registered,
            total: scan.total,
          })}
        {scan.status === "paused" && t("storage.scan.paused")}
        {scan.status === "failed" &&
          t("storage.scan.failed", { error: scan.error ?? "" })}
      </span>
      {(scan.status === "running" || scan.status === "paused") && (
        <button
          type="button"
          onClick={onCancel}
          className="shrink-0 rounded border border-edge px-1.5 py-0.5 text-[10px] text-text-muted transition-colors hover:border-red-400/70 hover:text-red-400"
          data-testid="storage-scan-cancel"
        >
          {t("storage.scan.cancel")}
        </button>
      )}
    </div>
  );
}

export default function StoragePage() {
  const { t } = useTranslation();
  const [libraries, setLibraries] = useState<PhotoLibrary[]>([]);
  const [status, setStatus] = useState<"loading" | "ready">("loading");
  /** 新建照片库对话框（true=打开；模式在对话框内二选一） */
  const [createOpen, setCreateOpen] = useState(false);
  const [openError,setOpenError]=useState<string|null>(null);
  const [removeTarget, setRemoveTarget] = useState<PhotoLibrary | null>(null);
  /** 操作/扫描完成反馈（hint=跨库重复提示等第二行；自动消失） */
  const [toast, setToast] = useState<{ message: string; hint?: string } | null>(null);
  /** toast 计时器：后一条 toast 顶掉前一条时先清旧计时器，防提前熄灭 */
  const toastTimerRef = useRef<number | null>(null);

  function flash(message: string, hint?: string): void {
    if (toastTimerRef.current !== null) window.clearTimeout(toastTimerRef.current);
    setToast({ message, hint });
    toastTimerRef.current = window.setTimeout(
      () => setToast(null),
      hint !== undefined ? 5000 : 2400,
    );
  }

  const pull = useCallback(async (): Promise<void> => {
    const list = await photoLibraryList();
    setLibraries(list);
    setStatus("ready");
  }, []);

  // 挂载拉一次；photoLibrariesChanged 事件重拉（登记增删全走它）
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | null = null;
    void pull();
    void subscribeAppEvents((event: AppEvent) => {
      if (event.type === "photoLibrariesChanged") void pull();
    })
      .then((off) => {
        if (cancelled) off();
        else unlisten = off;
      })
      .catch(() => {
        // 非 Tauri 环境（vite dev 预览）静默
      });
    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [pull]);

  /** 扫描收尾通知（M4f）：登记/跳过计数 + 跨库重复提示（契约扩展字段，
   *  后端未发则不显示第二行）；同时补拉一次列表让照片数/容量缓存跟上。
   *  useLibraryScan 以 ref 持有本回调，闭包始终是最新 libraries。 */
  function handleScanFinished(summary: LibraryScanFinishSummary): void {
    const lib = libraries.find((l) => l.id === summary.libraryId);
    flash(
      lib === undefined
        ? t("storage.scan.finishedToastUnnamed", {
            registered: summary.registered,
            skipped: summary.skipped,
          })
        : t("storage.scan.finishedToast", {
            name: lib.name,
            registered: summary.registered,
            skipped: summary.skipped,
          }),
      summary.crossLibraryDuplicates !== undefined
        ? t("storage.scan.crossLibraryHint", { count: summary.crossLibraryDuplicates })
        : undefined,
    );
    void pull();
  }

  const { scans, lastSyncAt, cancelScan } = useLibraryScan(handleScanFinished);

  async function openInSystem(lib: PhotoLibrary): Promise<void> {
    try {
      await openWithSystem(lib.rootPath);
    } catch (e) {
      setOpenError(t("storage.openFailed", { error: e instanceof Error ? e.message : String(e) }));
    }
  }

  return (
    <div className="relative h-full" data-testid="storage-page">
      <ErrorModal message={openError} onClose={()=>setOpenError(null)} />
      <div className="flex h-full w-full flex-col">
        {/* 头部：标题 + 说明 + 登记入口 */}
        <header className="flex h-12 shrink-0 items-center gap-3 border-b border-edge px-4">
          <h1 className="text-sm font-semibold text-text-primary">{t("storage.title")}</h1>
          <p className="hidden truncate text-xs text-text-muted md:block">{t("storage.desc")}</p>
          <div className="ml-auto flex shrink-0 gap-2">
            <button
              type="button"
              onClick={() => setCreateOpen(true)}
              className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
              data-testid="storage-create-library"
            >
              {t("storage.create")}
            </button>
          </div>
        </header>

        {/* 卡片列表 / 空态（卡片顶满内容宽，仅留页边距——与其他页面一致） */}
        <div className="sp-scroll min-h-0 flex-1 overflow-y-auto px-5 py-5">
          <div className="flex w-full flex-col gap-3">
            {status === "loading" ? (
              <div
                className="flex h-40 items-center justify-center text-xs text-text-muted"
                data-testid="storage-loading"
              >
                {t("storage.loading")}
              </div>
            ) : libraries.length === 0 ? (
              <div
                className="flex h-72 flex-col items-center justify-center gap-2 px-8 text-center"
                data-testid="storage-empty"
              >
                <svg
                  viewBox="0 0 24 24"
                  width="44"
                  height="44"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className="text-text-muted"
                  aria-hidden="true"
                >
                  <path d="M3 7.5V5.8A1.8 1.8 0 0 1 4.8 4h4.4l2 2.5h8A1.8 1.8 0 0 1 21 8.3v10A1.8 1.8 0 0 1 19.2 20H4.8A1.8 1.8 0 0 1 3 18.2z" />
                </svg>
                <p className="text-sm text-text-secondary">{t("storage.empty")}</p>
                <p className="max-w-md text-xs leading-relaxed text-text-muted">
                  {t("storage.emptyHint")}
                </p>
                <div className="mt-3 flex gap-2">
                  <button
                    type="button"
                    onClick={() => setCreateOpen(true)}
                    className="rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
                    data-testid="storage-empty-create"
                  >
                    {t("storage.create")}
                  </button>
                </div>
              </div>
            ) : (
              libraries.map((lib) => (
                <div
                  key={lib.id}
                  className="rounded-xl border border-edge bg-panel/30 p-4"
                  data-testid="storage-library-card"
                  data-id={lib.id}
                  data-status={lib.status}
                >
                  <div className="flex items-start justify-between gap-4">
                    <div className="min-w-0">
                      <div className="flex items-center gap-2">
                        <h3 className="truncate text-sm font-medium text-text-primary">
                          {lib.name}
                        </h3>
                        <span
                          className={`inline-flex shrink-0 items-center gap-1 rounded-full px-2 py-0.5 text-[10px] leading-none ${
                            lib.status === "online"
                              ? "bg-emerald-400/10 text-emerald-400"
                              : "bg-amber-300/10 text-amber-300"
                          }`}
                          data-testid="storage-library-status"
                        >
                          <span
                            className={`h-1.5 w-1.5 rounded-full ${
                              lib.status === "online" ? "bg-emerald-400" : "bg-amber-300"
                            }`}
                            aria-hidden="true"
                          />
                          {t(`storage.card.status.${lib.status}`)}
                        </span>
                      </div>
                      <p
                        className="mt-1 truncate font-mono text-[11px] text-text-muted"
                        title={lib.rootPath}
                        data-testid="storage-library-path"
                      >
                        {lib.rootPath}
                      </p>
                      <p
                        className="mt-2 text-xs tabular-nums text-text-secondary"
                        data-testid="storage-library-stats"
                      >
                        {t("storage.card.stats", {
                          count: lib.assetCount,
                          size: formatBytes(lib.sizeBytes),
                        })}
                      </p>
                      <ScanStatusRow
                        scan={scans[lib.id]}
                        lastSyncAt={lastSyncAt[lib.id]}
                        onCancel={() => cancelScan(lib.id)}
                      />
                    </div>
                    <div className="flex shrink-0 flex-wrap justify-end gap-2">
                      <button
                        type="button"
                        onClick={() => void openInSystem(lib)}
                        className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
                        data-testid="storage-open-in-system"
                      >
                        {t("storage.card.openInSystem")}
                      </button>
                      <button
                        type="button"
                        onClick={() => setRemoveTarget(lib)}
                        className="rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-red-400/70 hover:text-red-400"
                        data-testid="storage-remove"
                      >
                        {t("storage.card.remove")}
                      </button>
                    </div>
                  </div>
                </div>
              ))
            )}
          </div>
        </div>
      </div>

      {/* 新建照片库对话框（模式在对话框内二选一：新建 / 从已有文件夹建立） */}
      {createOpen && (
        <CreateLibraryDialog
          onClose={() => setCreateOpen(false)}
          onCreated={(library, mode) => {
            // reference 模式登记即触发后台递归扫描——toast 直接说明扫描已开始，
            // 进度/取消/完成通知由卡片扫描状态行接手（M4f 闭环）。
            flash(
              mode === "reference"
                ? t("storage.createdScanning", { name: library.name })
                : t("storage.created", { name: library.name }),
            );
            setCreateOpen(false);
            // 后端登记完成会发 photoLibrariesChanged 事件驱动重拉；事件未到
            // （后端未实装）时本地立即补拉一次，保证对话框关闭即见新卡。
            void pull();
          }}
        />
      )}

      {/* 移除登记确认（两档：仅摘登记 / 连记录删；永不删照片文件） */}
      {removeTarget !== null && (
        <RemoveLibraryDialog
          library={removeTarget}
          onClose={() => setRemoveTarget(null)}
          onRemoved={(result) => {
            const name = removeTarget.name;
            setRemoveTarget(null);
            flash(
              result.recordsDeleted > 0
                ? t("storage.removedDeleted", { name, count: result.recordsDeleted })
                : t("storage.removedKept", { name }),
            );
            void pull();
          }}
        />
      )}

      {/* 操作/扫描完成反馈 toast（与回收站页同形态；第二行为跨库重复等提示） */}
      {toast !== null && (
        <div className="pointer-events-none fixed bottom-20 left-1/2 z-30 -translate-x-1/2">
          <div
            className="rounded-2xl border border-edge bg-surface px-3 py-1 text-center shadow-xl"
            data-testid="storage-toast"
            role="status"
          >
            <p className="text-[11px] text-text-secondary">{toast.message}</p>
            {toast.hint !== undefined && (
              <p
                className="mt-0.5 text-[11px] text-amber-300"
                data-testid="storage-toast-hint"
              >
                {toast.hint}
              </p>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
