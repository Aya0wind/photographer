import { useCallback, useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useTranslation } from "react-i18next";

import {
  albumList,
  cullSessionDiscard,
  cullSessionList,
  cullSessionRename,
  type AlbumDto,
  type CullFinishResult,
  type CullSessionDto,
} from "@/ipc/api";
import { useCullingStore } from "../cullingStore";
import CullingOverlay from "../components/CullingOverlay";

/**
 * 选片会话页（/culling，V1）：进行中卡片置顶（名称/来源/进度/续选/改名/删除）
 * + 已完成折叠区 + 全屏过片层挂载位。
 * 会话归档不删（方案 §1）：finished 会话仅展示历史摘要，收尾映射在过片层内完成。
 */

/** 进行中卡片 */
function SessionCard({
  session,
  albumNames,
  onContinue,
  onRename,
  onDelete,
}: {
  session: CullSessionDto;
  albumNames: Map<number, string>;
  onContinue: () => void;
  onRename: () => void;
  onDelete: () => void;
}) {
  const { t } = useTranslation();
  const scope = session.scope;
  const scopeDesc =
    scope.kind === "album"
      ? scope.subgroup === null
        ? t("culling.scope.album", { name: albumNames.get(scope.albumId) ?? `#${scope.albumId}` })
        : t("culling.scope.albumSubgroup", {
            name: albumNames.get(scope.albumId) ?? `#${scope.albumId}`,
            subgroup: scope.subgroup,
          })
      : t("culling.scope.query", { count: scope.assetIds.length });
  const decided = session.accepted + session.rejected;
  const decidedPct = session.total > 0 ? (decided / session.total) * 100 : 0;

  return (
    <div
      className="rounded-xl border border-edge bg-surface p-3.5"
      data-testid="culling-session-card"
      data-session-id={session.id}
      data-finished={session.finishedAt !== null}
    >
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onContinue}
          disabled={session.finishedAt !== null}
          className="min-w-0 shrink-0 truncate text-left text-sm font-semibold text-text-primary transition-colors hover:text-accent disabled:hover:text-text-primary"
          title={session.name}
          data-testid="culling-session-name"
        >
          {session.name}
        </button>
        <span className="min-w-0 shrink truncate text-[11px] text-text-muted" data-testid="culling-session-scope">
          {scopeDesc}
        </span>
        <div className="ml-auto flex shrink-0 items-center gap-1.5">
          <button
            type="button"
            onClick={onContinue}
            disabled={session.finishedAt !== null}
            className="rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="culling-session-continue"
          >
            {t("culling.continue")}
          </button>
          <button
            type="button"
            onClick={onRename}
            disabled={session.finishedAt !== null}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="culling-session-rename"
          >
            {t("culling.rename")}
          </button>
          <button
            type="button"
            onClick={onDelete}
            disabled={session.finishedAt !== null}
            className="rounded-md border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:border-red-400 hover:text-red-400 disabled:cursor-not-allowed disabled:opacity-40"
            data-testid="culling-session-delete"
          >
            {t("culling.delete")}
          </button>
        </div>
      </div>
      {/* 进度：已选/已剔除计数 + 已决定占比条（分色同过片层） */}
      <div className="mt-2.5 flex items-center gap-2.5">
        <span
          className="shrink-0 font-mono text-[11px] tabular-nums text-text-secondary"
          data-testid="culling-session-progress"
          data-accepted={session.accepted}
          data-rejected={session.rejected}
          data-undecided={session.undecided}
        >
          {t("culling.card.progress", {
            accepted: session.accepted,
            rejected: session.rejected,
            undecided: session.undecided,
          })}
        </span>
        <div className="flex h-1.5 min-w-0 flex-1 gap-px overflow-hidden rounded-full bg-panel/70">
          <div
            className="h-full bg-emerald-500"
            style={{ width: `${session.total > 0 ? (session.accepted / session.total) * 100 : 0}%` }}
          />
          <div
            className="h-full bg-red-500"
            style={{ width: `${session.total > 0 ? (session.rejected / session.total) * 100 : 0}%` }}
          />
        </div>
        <span className="shrink-0 font-mono text-[10px] tabular-nums text-text-muted">
          {Math.round(decidedPct)}%
        </span>
      </div>
    </div>
  );
}

/** 已完成行（历史摘要，不可续选） */
function FinishedRow({ session }: { session: CullSessionDto }) {
  return (
    <div
      className="flex items-center gap-2.5 rounded-lg border border-edge/60 bg-surface/60 px-3 py-2"
      data-testid="culling-finished-row"
      data-session-id={session.id}
    >
      <svg
        viewBox="0 0 16 16"
        width="12"
        height="12"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        className="shrink-0 text-emerald-400"
        aria-hidden="true"
      >
        <path d="M3 8.5l3.2 3.2L13 5" />
      </svg>
      <span className="min-w-0 shrink truncate text-xs text-text-secondary" title={session.name}>
        {session.name}
      </span>
      <span
        className="ml-auto shrink-0 font-mono text-[10px] tabular-nums text-text-muted"
        data-testid="culling-finished-summary"
        data-accepted={session.accepted}
        data-rejected={session.rejected}
      >
        {session.accepted} / {session.rejected}
      </span>
    </div>
  );
}

interface ToastState {
  kind: "finished" | "finishFailed" | "renamed";
  summary?: CullFinishResult;
}

export default function CullingPage() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const refreshActiveCount = useCullingStore((s) => s.refreshActiveCount);

  const [sessions, setSessions] = useState<CullSessionDto[] | null>(null);
  const [openId, setOpenId] = useState<number | null>(null);
  const [toast, setToast] = useState<ToastState | null>(null);
  const [albumNames, setAlbumNames] = useState<Map<number, string>>(new Map());
  const [renaming, setRenaming] = useState<CullSessionDto | null>(null);
  const [renameName, setRenameName] = useState("");
  const [deleting, setDeleting] = useState<CullSessionDto | null>(null);
  const [finishedExpanded, setFinishedExpanded] = useState(false);

  const refresh = useCallback(async () => {
    const list = await cullSessionList();
    setSessions(list);
    void refreshActiveCount();
  }, [refreshActiveCount]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // 相册名解析（scope 来源描述用；失败不影响主流程）
  useEffect(() => {
    let cancelled = false;
    void albumList().then((albums: AlbumDto[]) => {
      if (cancelled) return;
      setAlbumNames(new Map(albums.map((a) => [a.id, a.name] as const)));
    });
    return () => {
      cancelled = true;
    };
  }, []);

  // 入口跳转协议：navigate("/culling", { state: { open: sessionId } }) 直接续选
  const routeOpen = (location.state as { open?: number } | null)?.open ?? null;
  useEffect(() => {
    if (routeOpen === null) return;
    setOpenId(routeOpen);
    navigate("/culling", { replace: true, state: null });
  }, [routeOpen, navigate]);

  const openSession = useMemo(
    () => sessions?.find((s) => s.id === openId) ?? null,
    [sessions, openId],
  );
  const active = useMemo(
    () => (sessions ?? []).filter((s) => s.finishedAt === null),
    [sessions],
  );
  const finished = useMemo(
    () => (sessions ?? []).filter((s) => s.finishedAt !== null),
    [sessions],
  );

  // toast 自动消退
  useEffect(() => {
    if (toast === null) return undefined;
    const timer = window.setTimeout(() => setToast(null), 4000);
    return () => clearTimeout(timer);
  }, [toast]);

  async function submitRename(): Promise<void> {
    if (renaming === null) return;
    const name = renameName.trim();
    if (name === "" || name === renaming.name) {
      setRenaming(null);
      return;
    }
    const ok = await cullSessionRename(renaming.id, name);
    if (ok) {
      setSessions((prev) =>
        prev ? prev.map((s) => (s.id === renaming.id ? { ...s, name } : s)) : prev,
      );
      setToast({ kind: "renamed" });
    }
    setRenaming(null);
  }

  async function submitDelete(): Promise<void> {
    if (deleting === null) return;
    const ok = await cullSessionDiscard(deleting.id);
    if (ok) {
      setSessions((prev) => (prev ? prev.filter((s) => s.id !== deleting.id) : prev));
      void refreshActiveCount();
    }
    setDeleting(null);
  }

  return (
    <div className="h-full overflow-y-auto" data-testid="culling-page">
      <div className="w-full px-4 pt-4 pb-8">
        {/* 工具条 */}
        <div className="flex shrink-0 items-center gap-3" data-testid="culling-toolbar">
          <h1 className="text-sm font-semibold text-text-primary">{t("culling.title")}</h1>
          <p className="text-xs text-text-muted">{t("culling.desc")}</p>
        </div>

        {/* 进行中区 */}
        <section className="mt-4" data-testid="culling-active-section">
          <h2 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
            {t("culling.activeSection")}
            <span className="ml-1.5 font-mono">{active.length}</span>
          </h2>
          {sessions === null ? (
            <p className="py-4 text-xs text-text-muted" data-testid="culling-loading">
              {t("recent.loading")}
            </p>
          ) : active.length === 0 ? (
            <div
              className="flex flex-col items-center gap-2 rounded-xl border border-dashed border-edge px-6 py-10 text-center"
              data-testid="culling-empty"
            >
              <svg
                viewBox="0 0 24 24"
                width="40"
                height="40"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.2"
                strokeLinecap="round"
                strokeLinejoin="round"
                className="text-text-muted"
                aria-hidden="true"
              >
                <rect x="3.5" y="5" width="17" height="14" rx="2" />
                <path d="M3.5 15l4.5-4.5 3.5 3.5 3-3L20.5 16" />
                <circle cx="9" cy="9.5" r="1.5" />
              </svg>
              <p className="text-sm text-text-secondary">{t("culling.empty")}</p>
              <p className="max-w-md text-xs leading-relaxed text-text-muted">{t("culling.emptyHint")}</p>
            </div>
          ) : (
            <div className="flex flex-col gap-2.5">
              {active.map((session) => (
                <SessionCard
                  key={session.id}
                  session={session}
                  albumNames={albumNames}
                  onContinue={() => setOpenId(session.id)}
                  onRename={() => {
                    setRenaming(session);
                    setRenameName(session.name);
                  }}
                  onDelete={() => setDeleting(session)}
                />
              ))}
            </div>
          )}
        </section>

        {/* 已完成折叠区（历史归档，仅摘要） */}
        {finished.length > 0 && (
          <section className="mt-6" data-testid="culling-finished-section">
            <button
              type="button"
              onClick={() => setFinishedExpanded((v) => !v)}
              aria-expanded={finishedExpanded}
              className="mb-2 flex items-center gap-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted"
              data-testid="culling-finished-toggle"
            >
              <svg
                viewBox="0 0 16 16"
                width="11"
                height="11"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.8"
                className={`transition-transform ${finishedExpanded ? "rotate-90" : ""}`}
                aria-hidden="true"
              >
                <path d="m6 3 5 5-5 5" />
              </svg>
              {t("culling.finishedSection")}
              <span className="ml-1 font-mono">{finished.length}</span>
            </button>
            {finishedExpanded && (
              <div className="flex flex-col gap-1.5">
                {finished.map((session) => (
                  <FinishedRow key={session.id} session={session} />
                ))}
              </div>
            )}
          </section>
        )}
      </div>

      {/* 改名弹窗 */}
      {renaming !== null && (
        <div
          className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
          role="dialog"
          aria-modal="true"
          aria-label={t("culling.renameTitle")}
          onClick={(e) => {
            if (e.target === e.currentTarget) setRenaming(null);
          }}
          data-testid="culling-rename-overlay"
        >
          <div
            className="w-[360px] overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
            onClick={(e) => e.stopPropagation()}
            data-testid="culling-rename-dialog"
          >
            <div className="border-b border-edge px-4 py-3">
              <h2 className="text-sm font-semibold text-text-primary">{t("culling.renameTitle")}</h2>
            </div>
            <div className="p-4">
              <input
                autoFocus
                type="text"
                value={renameName}
                onChange={(e) => setRenameName(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    void submitRename();
                  }
                  if (e.key === "Escape") setRenaming(null);
                }}
                aria-label={t("culling.renameTitle")}
                className="h-8 w-full rounded-md border border-edge bg-bg px-2.5 text-xs text-text-primary outline-none transition-colors focus:border-accent"
                data-testid="culling-rename-input"
              />
            </div>
            <div className="flex justify-end gap-2 border-t border-edge px-4 py-3">
              <button
                type="button"
                onClick={() => setRenaming(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="culling-rename-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void submitRename()}
                disabled={renameName.trim() === "" || renameName.trim() === renaming.name}
                className="rounded-md bg-accent px-4 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:cursor-not-allowed disabled:opacity-40"
                data-testid="culling-rename-confirm"
              >
                {t("culling.renameConfirm")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 删除确认 */}
      {deleting !== null && (
        <div
          className="fixed inset-0 z-[70] flex items-center justify-center bg-black/45"
          role="dialog"
          aria-modal="true"
          aria-label={t("culling.deleteTitle")}
          onClick={(e) => {
            if (e.target === e.currentTarget) setDeleting(null);
          }}
          data-testid="culling-delete-overlay"
        >
          <div
            className="w-[380px] overflow-hidden rounded-xl border border-edge bg-surface shadow-2xl"
            onClick={(e) => e.stopPropagation()}
            data-testid="culling-delete-dialog"
          >
            <div className="border-b border-edge px-4 py-3">
              <h2 className="text-sm font-semibold text-text-primary" data-testid="culling-delete-title">
                {t("culling.deleteTitle", { name: deleting.name })}
              </h2>
            </div>
            <div className="p-4">
              <p className="text-xs leading-relaxed text-text-secondary" data-testid="culling-delete-hint">
                {t("culling.deleteHint", {
                  accepted: deleting.accepted,
                  rejected: deleting.rejected,
                })}
              </p>
            </div>
            <div className="flex justify-end gap-2 border-t border-edge px-4 py-3">
              <button
                type="button"
                onClick={() => setDeleting(null)}
                className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:bg-panel hover:text-text-primary"
                data-testid="culling-delete-cancel"
              >
                {t("common.cancel")}
              </button>
              <button
                type="button"
                onClick={() => void submitDelete()}
                className="rounded-md bg-red-500/90 px-4 py-1.5 text-xs font-medium text-white transition-colors hover:bg-red-500"
                data-testid="culling-delete-confirm"
              >
                {t("culling.deleteConfirm")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* 全屏选片层（决定即落库：关闭=保存，无确认） */}
      {openSession !== null && (
        <CullingOverlay
          session={openSession}
          onClose={() => {
            setOpenId(null);
            void refresh();
          }}
          onFinished={(summary) => {
            setOpenId(null);
            setToast({ kind: "finished", summary });
            void refresh();
          }}
          onFinishFailed={() => setToast({ kind: "finishFailed" })}
        />
      )}

      {/* 摘要 toast */}
      {toast !== null && (
        <div
          className="fixed bottom-6 left-1/2 z-[90] -translate-x-1/2 rounded-lg border border-edge bg-surface px-4 py-2.5 text-xs shadow-2xl"
          role="status"
          data-testid="culling-toast"
          data-kind={toast.kind}
        >
          {toast.kind === "finished" && toast.summary !== undefined ? (
            t("culling.finish.summaryToast", {
              accepted: toast.summary.appliedFlag,
              rating: toast.summary.appliedRating,
              rejected: toast.summary.rejected,
            })
          ) : toast.kind === "finishFailed" ? (
            <span className="text-red-400">{t("culling.finish.failed")}</span>
          ) : (
            t("culling.renamedToast")
          )}
        </div>
      )}
    </div>
  );
}
