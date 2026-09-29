import { usePhotoCards } from "@/features/gallery/lib/usePhotoCards";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { motion } from "motion/react";

import { motionInitial, useMotionOn } from "@/lib/motion";

import {
  peopleAssets,
  peopleList,
  personDelete,
  personRename,
  type AssetDto,
  type PersonCluster,
} from "@/ipc/api";
import { groupAssetsByDate } from "@/features/gallery/lib/assetGroups";
import AssetGrid from "@/features/gallery/components/AssetGrid";
import AssetThumb from "@/features/gallery/components/AssetThumb";
import { useAssetViewer } from "@/features/gallery/lib/useAssetViewer";
import ViewerOverlay from "@/features/gallery/components/ViewerOverlay";

/**
 * 人物页（M4）：人脸聚类结果实化。
 * - 列表：peopleList()（进入页面拉取刷新；v1 无事件，重命名/删除后重拉）。
 *   封面 = coverAssetId 走 asset_thumb_get 管线（AssetThumb）；未命名显示「人物 N」；
 *   faceCount 徽标；inline 重命名 + 红色两步删除确认。
 * - 点击卡片 → 该人物照片列表（peopleAssets → AssetGrid + 查看器，复用画廊管线）。
 * - 后端未就绪/空数据：沿用 v1 空态（说明 + 占位网格）兜底。
 */

/** 单人物照片列表上限（聚类内照片通常远小于此；分页 v2 再说） */
const PERSON_ASSETS_LIMIT = 500;

// --- 重命名 inline 输入 -------------------------------------------------------------

function RenameInput({
  initial,
  onCommit,
  onCancel,
}: {
  initial: string;
  onCommit: (next: string) => void;
  onCancel: () => void;
}) {
  const [value, setValue] = useState(initial);
  const doneRef = useRef(false); // Enter 后紧接 blur 不重复提交

  function finish(commit: boolean): void {
    if (doneRef.current) return;
    doneRef.current = true;
    if (commit) onCommit(value);
    else onCancel();
  }

  return (
    <input
      autoFocus
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          e.preventDefault();
          finish(true);
        } else if (e.key === "Escape") {
          e.preventDefault();
          finish(false);
        }
      }}
      onBlur={() => finish(true)}
      aria-label="person-rename"
      data-testid="people-rename-input"
      className="w-full rounded border border-accent bg-bg px-1.5 py-0.5 text-xs text-text-primary outline-none"
    />
  );
}

// --- 人物卡片 -----------------------------------------------------------------------

function PersonCard({
  person,
  openLabel,
  onOpen,
  onChanged,
}: {
  person: PersonCluster;
  openLabel: string;
  onOpen: () => void;
  /** 重命名/删除成功后回调（父级重拉清单） */
  onChanged: () => void;
}) {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const displayName = person.name ?? t("people.unnamed", { n: person.clusterId + 1 });

  async function commitName(next: string): Promise<void> {
    setEditing(false);
    const trimmed = next.trim();
    if (trimmed === "" || trimmed === person.name) return; // 空名/未变 = 取消
    if (await personRename(person.clusterId, trimmed)) onChanged();
  }

  async function confirmDelete(): Promise<void> {
    setConfirming(false);
    if (await personDelete(person.clusterId)) onChanged();
  }

  return (
    <div
      className="relative flex flex-col items-center gap-1.5 rounded-lg border border-edge/60 bg-surface/50 p-2"
      data-testid="people-card"
      data-cluster-id={person.clusterId}
    >
      <button
        type="button"
        onClick={onOpen}
        title={openLabel}
        aria-label={`${openLabel}：${displayName}`}
        className="w-full overflow-hidden rounded-md outline-none focus-visible:outline-2 focus-visible:outline-accent"
      >
        <AssetThumb
          asset={{ id: person.coverAssetId, kind: "photo", name: displayName }}
          size={240}
          alt={displayName}
          className="aspect-square w-full rounded-md"
          testId="people-card-cover"
        />
      </button>

      {editing ? (
        <RenameInput
          initial={person.name ?? ""}
          onCommit={(next) => void commitName(next)}
          onCancel={() => setEditing(false)}
        />
      ) : (
        <button
          type="button"
          onClick={onOpen}
          className="max-w-full truncate text-xs text-text-primary"
          data-testid="people-card-name"
        >
          {displayName}
        </button>
      )}

      <div className="flex w-full items-center justify-between gap-1">
        <span
          className="rounded bg-panel px-1.5 py-0.5 font-mono text-[10px] tabular-nums text-text-muted"
          data-testid="people-card-count"
        >
          {t("people.faceCount", { count: person.faceCount })}
        </span>
        <span className="flex shrink-0 items-center gap-1">
          <button
            type="button"
            onClick={() => setEditing(true)}
            title={t("people.rename")}
            aria-label={t("people.rename")}
            className="rounded p-1 text-text-muted transition-colors hover:bg-panel/60 hover:text-text-primary"
            data-testid="people-card-rename"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M11.3 2.2l2.5 2.5L5 13.5l-3 .5.5-3z" />
            </svg>
          </button>
          <button
            type="button"
            onClick={() => setConfirming(true)}
            title={t("people.delete")}
            aria-label={t("people.delete")}
            className="rounded p-1 text-text-muted transition-colors hover:bg-red-500/15 hover:text-red-400"
            data-testid="people-card-delete"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M2.5 4h11M6.5 4V2.5h3V4M4 4l.7 9.5h6.6L12 4M6.7 6.8v4.4M9.3 6.8v4.4" />
            </svg>
          </button>
        </span>
      </div>

      {/* 删除两步确认（红色强确认；仅拆聚类，照片不受影响） */}
      {confirming && (
        <div
          className="absolute inset-0 z-10 flex flex-col items-center justify-center gap-2 rounded-lg bg-bg/95 p-2 text-center"
          data-testid="people-delete-confirm"
        >
          <p className="text-[11px] leading-relaxed text-text-secondary">
            {t("people.deleteConfirm")}
          </p>
          <div className="flex gap-1.5">
            <button
              type="button"
              onClick={() => void confirmDelete()}
              className="rounded bg-red-600 px-2 py-1 text-[11px] font-medium text-white transition-colors hover:bg-red-500"
              data-testid="people-delete-confirm-yes"
            >
              {t("people.deleteConfirmYes")}
            </button>
            <button
              type="button"
              onClick={() => setConfirming(false)}
              className="rounded border border-edge px-2 py-1 text-[11px] text-text-secondary transition-colors hover:text-text-primary"
              data-testid="people-delete-confirm-no"
            >
              {t("common.cancel")}
            </button>
          </div>
        </div>
      )}
    </div>
  );
}

// --- 单人物照片视图 -------------------------------------------------------------------

function PersonAssetsView({
  person,
  onBack,
}: {
  person: PersonCluster;
  onBack: () => void;
}) {
  const { t } = useTranslation();
  const [assets, setAssets] = useState<AssetDto[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    setAssets(null);
    void peopleAssets(person.clusterId, PERSON_ASSETS_LIMIT).then((list) => {
      if (!cancelled) setAssets(list);
    });
    return () => {
      cancelled = true;
    };
  }, [person.clusterId]);

  const { cards, badges } = usePhotoCards(assets ?? []);
  const groups = useMemo(() => groupAssetsByDate(cards), [cards]);
  const { viewer, openAsset, closeViewer, navigateTo, selectVersion } = useAssetViewer(groups, assets ?? []);

  return (
    <div className="flex h-full flex-col" data-testid="people-assets-view">
      <div className="flex h-11 shrink-0 items-center gap-3 border-b border-edge px-4">
        <button
          type="button"
          onClick={onBack}
          className="flex shrink-0 items-center gap-1 rounded-md border border-edge px-2 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
          data-testid="people-back"
        >
          <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            <path d="M10 3L5 8l5 5" />
          </svg>
          {t("people.back")}
        </button>
        <h2 className="truncate text-sm font-semibold text-text-primary" data-testid="people-assets-title">
          {person.name ?? t("people.unnamed", { n: person.clusterId + 1 })}
        </h2>
        <span
          className="shrink-0 rounded-full bg-panel px-2 py-0.5 font-mono text-[11px] tabular-nums text-text-secondary"
          data-testid="people-assets-count"
        >
          {t("people.faceCount", { count: person.faceCount })}
        </span>
      </div>

      <div className="min-h-0 flex-1">
        {assets === null ? (
          <div className="flex h-full items-center justify-center text-xs text-text-muted" data-testid="people-assets-loading">
            {t("search.loading")}
          </div>
        ) : assets.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-2 px-8 text-center" data-testid="people-assets-empty">
            <p className="text-sm text-text-secondary">{t("people.assetsEmpty")}</p>
          </div>
        ) : (
          <AssetGrid badges={badges} groups={groups} onOpenAsset={(asset) => openAsset(asset)} scrollTestId="people-grid-scroll" />
        )}
      </div>

      {viewer && (
        <ViewerOverlay
          asset={viewer.asset}
          group={viewer.group}
          index={viewer.index}
          onVersionSelect={selectVersion}
          onNavigate={navigateTo}
          onClose={closeViewer}
        />
      )}
    </div>
  );
}

// --- 页面 ---------------------------------------------------------------------------

export default function PeoplePage() {
  const motionOn = useMotionOn();
  const { t } = useTranslation();
  // null = 首拉进行中；[] = 后端未就绪/无聚类（空态兜底）
  const [people, setPeople] = useState<PersonCluster[] | null>(null);
  const [selected, setSelected] = useState<PersonCluster | null>(null);

  const refresh = useCallback(async () => {
    try {
      setPeople(await peopleList());
    } catch {
      setPeople([]); // 防御：peopleList 契约内不 reject，此处兜底回空态
    }
  }, []);

  // v1 无新事件：进入人物页拉取刷新；重命名/删除后重拉
  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (selected) {
    return (
      <div className="h-full" data-testid="people-page">
        <PersonAssetsView person={selected} onBack={() => setSelected(null)} />
      </div>
    );
  }

  return (
    <div className="h-full overflow-y-auto" data-testid="people-page">
      <div className="flex h-full w-full flex-col px-4 pt-4">
        <div className="flex shrink-0 items-baseline gap-3">
          <h1 className="text-sm font-semibold text-text-primary">{t("people.title")}</h1>
          {people !== null && people.length > 0 && (
            <p className="text-xs text-text-muted" data-testid="people-count">
              {t("people.count", { count: people.length })}
            </p>
          )}
        </div>

        {people === null ? (
          <div className="flex flex-1 items-center justify-center text-xs text-text-muted" data-testid="people-loading">
            {t("people.loading")}
          </div>
        ) : people.length === 0 ? (
          // 空态（未开人脸索引/尚无聚类）：与画廊等页一致的居中提示，
          // 不再铺「待索引」占位卡堆（2026-09-29 用户反馈）
          <div className="flex flex-1 flex-col items-center justify-center gap-2" data-testid="people-empty">
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
              <circle cx="12" cy="8.5" r="3.5" />
              <path d="M5 19.5c1.2-3.4 3.9-5 7-5s5.8 1.6 7 5" />
            </svg>
            <p className="text-xs text-text-muted">{t("people.desc")}</p>
          </div>
        ) : (
          <div
            className="mt-4 grid flex-1 content-start grid-cols-[repeat(auto-fill,minmax(140px,1fr))] gap-3 pb-6"
            data-testid="people-grid"
          >
            {people.map((person, i) => (
              <motion.div
                key={person.clusterId}
                initial={motionInitial(motionOn, { opacity: 0, y: 8 })}
                animate={{ opacity: 1, y: 0 }}
                transition={{ duration: 0.18, ease: "easeOut", delay: Math.min(i * 0.02, 0.25) }}
              >
                <PersonCard
                  person={person}
                  openLabel={t("people.open")}
                  onOpen={() => setSelected(person)}
                  onChanged={() => void refresh()}
                />
              </motion.div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
