import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc } from "@tauri-apps/api/core";

import {
  assetDetail,
  assetsByIds,
  cullDecisionApply,
  cullSessionOpen,
  type AssetDetailDto,
  type AssetDto,
  type CullDecisionPatch,
  type CullFinishResult,
  type CullItemState,
  type CullPrescanDto,
  type CullSessionDto,
  type CullSessionOpenResult,
} from "@/ipc/api";
import {
  fetchAssetThumb,
  useAssetThumbUrl,
  warmImageDecode,
} from "@/features/gallery/lib/thumbPipeline";
import {
  buildKeepOnlyDecisions,
  clampZoomPoint,
  comparisonMembers,
  cullKeyAction,
  deriveProgress,
  indexAfterDecision,
  withDecision,
  zoomEnterPoint,
  type CullDecision,
  type CullDecisionMap,
  type CullZoomPoint,
} from "../lib/cullingCore";
import CullAiDialog from "./CullAiDialog";
import CullCompareGrid from "./CullCompareGrid";
import CullFilmstrip from "./CullFilmstrip";
import CullFinishDialog from "./CullFinishDialog";

/**
 * 全屏选片层（V1 单图过片 §3.1 + V2 对比视图 §3.2 + V3 AI 挑图 §3.3）：
 * - 键盘流：←/→ 翻片 · 空格/↑ 选入 · X/↓ 剔除 · U 回未定 · Z 按住放大 ·
 *   C 单图/对比切换（V2）· Esc 退出。决定即落库（乐观更新+失败回滚）。
 * - V2 放大跨图保持：放大位置+倍率翻片不变（同构图直查谁更锐）；松开 Z
 *   恢复常态，会话内记忆上次锚点——再按 Z 原位复放。
 * - V2 对比视图：同屏 2/3/4 张，组员=同连拍组优先+相邻补位；1-4 选焦后
 *   空格/X/U 作用于焦点张（张上按钮为鼠标等价物）；进出对比保持单图位置。
 * - V2 连拍组集成：当前片 burstSize≥2 显示「组 N 张」徽标 +「本组只留这张」
 *   （当前 accepted + 组内未定项批量 rejected，一次 decision_apply）。
 * - V3 AI 挑图：顶栏「AI 挑图」弹窗（规则 → 预览摘要 → 应用为 origin='ai'
 *   预标记）；AI 决定角标蓝描边+AI 小标（单图/对比/胶片条一致），手动改过
 *   即转 manual 样式。
 * - 接管键盘：window 捕获阶段监听 + stopPropagation；收尾/AI 弹窗打开时让位。
 */

const VIEWER_MID_SIZE = 2048;
const VIEWER_RAW_EMBED_SIZE = 6000;
const VIEWER_THUMB_SIZE = 1280;
/** Z 按住放大倍率（位置=transform-origin 跟随鼠标；跨图保持，V2） */
const ZOOM_SCALE = 2.5;

/** 资产元数据窗口加载（胶片条/主图用；万张会话不整表取回） */
const WINDOW_BEFORE = 24;
const WINDOW_AFTER = 72;
const ASSET_CHUNK = 60;

function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** 断点续选位：首个未定项；全决定完停在末张（复盘用） */
export function resumeIndexOf(items: CullItemState[]): number {
  const first = items.findIndex((item) => item.decision === null);
  return first === -1 ? Math.max(0, items.length - 1) : first;
}

export interface CullingOverlayProps {
  session: CullSessionDto;
  /** Esc/关闭 → 回列表（决定已落库，无需确认） */
  onClose: () => void;
  /** 收尾完成回传（页面出摘要 toast + 刷新清单） */
  onFinished: (summary: CullFinishResult, sessionName: string) => void;
  /** finish 调用失败回传（页面 toast） */
  onFinishFailed: () => void;
}

export default function CullingOverlay({
  session: sessionProp,
  onClose,
  onFinished,
  onFinishFailed,
}: CullingOverlayProps) {
  const { t } = useTranslation();

  // --- 会话装载（open：决定表 + 快照序） ----------------------------------------------
  const [session, setSession] = useState<CullSessionDto>(sessionProp);
  const [items, setItems] = useState<CullItemState[] | null>(null);
  const [openFailed, setOpenFailed] = useState(false);
  const [decisions, setDecisions] = useState<CullDecisionMap>(new Map());
  const [origins, setOrigins] = useState<Map<number, "manual" | "ai">>(new Map());
  const [index, setIndex] = useState(0);
  // --- V2 对比视图状态（单图/对比模式 + 同屏张数 + 焦点屏位） ---------------------------
  const [mode, setMode] = useState<"single" | "compare">("single");
  const [compareCount, setCompareCount] = useState<2 | 3 | 4>(2);
  const [focusedPane, setFocusedPane] = useState(0);

  /** open 结果落地（resume=true 断点续选位；false 保持当前位——AI 应用后刷新） */
  const applyOpened = useCallback((opened: CullSessionOpenResult, resume: boolean) => {
    setSession(opened.session);
    setItems(opened.items);
    setDecisions(
      new Map(
        opened.items
          .filter((item) => item.decision !== null)
          .map((item) => [item.assetId, item.decision] as const),
      ),
    );
    setOrigins(new Map(opened.items.map((item) => [item.assetId, item.origin] as const)));
    setIndex((cur) =>
      resume
        ? resumeIndexOf(opened.items)
        : opened.items.length === 0
          ? 0
          : Math.min(cur, opened.items.length - 1),
    );
  }, []);

  useEffect(() => {
    let cancelled = false;
    void cullSessionOpen(sessionProp.id).then((opened) => {
      if (cancelled) return;
      if (opened === null) {
        setOpenFailed(true);
        return;
      }
      applyOpened(opened, true);
    });
    return () => {
      cancelled = true;
    };
  }, [sessionProp.id, applyOpened]);

  const ids = useMemo(() => items?.map((item) => item.assetId) ?? [], [items]);
  const currentId = items !== null && index < items.length ? items[index].assetId : null;
  const currentBurst = items !== null && index < items.length ? items[index] : null;
  const progress = useMemo(
    () => deriveProgress(session.total, decisions),
    [session.total, decisions],
  );

  // --- 资产元数据窗口加载（assetsByIds 分块；已入队去重） ------------------------------
  const [assetById, setAssetById] = useState<Map<number, AssetDto>>(new Map());
  const requestedRef = useRef<Set<number>>(new Set());
  useEffect(() => {
    if (items === null || items.length === 0) return;
    const from = Math.max(0, index - WINDOW_BEFORE);
    const to = Math.min(items.length, index + WINDOW_AFTER);
    const missing: number[] = [];
    for (let i = from; i < to; i += 1) {
      const id = items[i].assetId;
      if (!requestedRef.current.has(id)) {
        requestedRef.current.add(id);
        missing.push(id);
      }
    }
    if (missing.length === 0) return;
    void (async () => {
      for (let offset = 0; offset < missing.length; offset += ASSET_CHUNK) {
        const list = await assetsByIds(missing.slice(offset, offset + ASSET_CHUNK));
        if (list.length === 0) continue;
        setAssetById((prev) => {
          const next = new Map(prev);
          for (const asset of list) next.set(asset.id, asset);
          return next;
        });
      }
    })();
  }, [items, index]);

  const asset = currentId !== null ? assetById.get(currentId) ?? null : null;
  /** 胶片条/对比格元数据投影（kind/name；未加载格回退序号占位） */
  const assetMeta = useMemo(() => {
    const map = new Map<number, { kind: AssetDto["kind"]; name: string }>();
    for (const a of assetById.values()) map.set(a.id, { kind: a.kind, name: a.name });
    return map;
  }, [assetById]);

  // --- 大图分级链（与查看器同款：photo 原图→2048→512；RAW 内嵌全幅→2048→512） -----------
  const originalUrl = useMemo(
    () => (asset?.kind === "photo" ? safeConvert(asset.path) : null),
    [asset?.kind, asset?.path],
  );
  const [photoStage, setPhotoStage] = useState<"original" | "mid" | "thumb">("original");
  useEffect(() => {
    setPhotoStage("original");
  }, [currentId]);
  const thumb = useAssetThumbUrl(currentId ?? 0, VIEWER_THUMB_SIZE, currentId !== null, "high");
  const mid = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_MID_SIZE,
    asset?.kind === "photo" && photoStage !== "original",
    "high",
  );
  const rawEmbed = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_RAW_EMBED_SIZE,
    asset?.kind === "raw",
    "high",
  );
  const rawFull = useAssetThumbUrl(
    currentId ?? 0,
    VIEWER_MID_SIZE,
    asset?.kind === "raw" && rawEmbed.status === "failed",
    "high",
  );

  let mainSrc: string | null = null;
  let mainFailed = false;
  if (asset?.kind === "photo") {
    if (photoStage === "original" && originalUrl !== null) mainSrc = originalUrl;
    else if (photoStage === "mid") mainSrc = mid.url;
    else mainSrc = thumb.url;
    mainFailed = thumb.status === "failed";
  } else if (asset?.kind === "raw") {
    mainSrc = rawEmbed.url ?? rawFull.url ?? thumb.url;
    mainFailed = rawEmbed.status === "failed" && rawFull.status === "failed" && thumb.status === "failed";
  }
  // 源文件被移动/删除（管线终态）：512 档恒请求，其 missing 结算即缺源信号——
  // 有历史缓存仍尽力显示，无缓存给占位（绝不让面板空白/无限转圈）
  const missing = thumb.status === "missing";
  const showMissingPlaceholder = missing && mainSrc === null;
  // photo 原图不可用（非 Tauri 环境/convert 抛错）→ 直达缩略图档
  useEffect(() => {
    if (asset?.kind === "photo" && photoStage === "original" && originalUrl === null) {
      setPhotoStage("thumb");
    }
  }, [asset?.kind, photoStage, originalUrl]);
  // 中间档确定无图（后端未加 2048 档）→ 自动降到 512 档；缺源且无缓存同样降档
  useEffect(() => {
    if (
      asset?.kind === "photo" &&
      photoStage === "mid" &&
      (mid.status === "failed" || (mid.status === "missing" && mid.url === null))
    ) {
      setPhotoStage("thumb");
    }
  }, [asset?.kind, photoStage, mid.status, mid.url]);

  // --- 无空窗图层（查看器同款精简版：新图解码完成才替换旧图） ---------------------------
  type ImageLayer = { src: string; phase: "active" | "loading" | "retiring" };
  const [imageLayers, setImageLayers] = useState<ImageLayer[]>([]);
  useEffect(() => {
    if (mainSrc === null) return;
    setImageLayers((previous) => {
      if (previous.some((layer) => layer.src === mainSrc)) return previous;
      const active = previous.find((layer) => layer.phase === "active");
      return active
        ? [active, { src: mainSrc, phase: "loading" }]
        : [{ src: mainSrc, phase: "loading" }];
    });
  }, [mainSrc]);
  useEffect(() => {
    if (!imageLayers.some((layer) => layer.phase === "retiring")) return;
    const frame = requestAnimationFrame(() => {
      setImageLayers((previous) => previous.filter((layer) => layer.phase !== "retiring"));
    });
    return () => cancelAnimationFrame(frame);
  }, [imageLayers]);
  useEffect(() => {
    if (mainFailed) setImageLayers([]);
  }, [mainFailed]);

  // --- V2 Z 按住放大：位置+倍率跨图保持，会话内记忆锚点 ---------------------------------
  // 状态三份：zoomed/zoomAt 驱动渲染；ref 镜像供按键监听（免重注册）与松开记忆。
  const [zoomed, setZoomed] = useState(false);
  const [zoomAt, setZoomAt] = useState<CullZoomPoint>({ x: 0.5, y: 0.5 });
  const zoomedRef = useRef(false);
  const zoomAtRef = useRef<CullZoomPoint>({ x: 0.5, y: 0.5 });
  /** 会话内锚点记忆（null=未用过放大，首次居中） */
  const zoomMemoryRef = useRef<CullZoomPoint | null>(null);
  const stageRef = useRef<HTMLDivElement | null>(null);

  function setZoomPoint(point: CullZoomPoint): void {
    zoomAtRef.current = point;
    setZoomAt(point);
  }
  function enterZoom(): void {
    if (zoomedRef.current) return; // 按住期间 keydown 重复（自动重复键）不重置锚点
    zoomedRef.current = true;
    setZoomPoint(zoomEnterPoint(zoomMemoryRef.current));
    setZoomed(true);
  }
  /** saveMemory=true 记住当前锚点（松开 Z）；双击/Esc 视为取消，不记 */
  function exitZoom(saveMemory: boolean): void {
    if (!zoomedRef.current) return;
    zoomedRef.current = false;
    if (saveMemory) zoomMemoryRef.current = zoomAtRef.current;
    setZoomed(false);
  }
  function handleStageMouseMove(e: React.MouseEvent<HTMLDivElement>): void {
    if (!zoomedRef.current) return;
    const rect = e.currentTarget.getBoundingClientRect();
    setZoomPoint(
      clampZoomPoint(
        (e.clientX - rect.left) / Math.max(1, rect.width),
        (e.clientY - rect.top) / Math.max(1, rect.height),
      ),
    );
  }

  // --- 决定：乐观更新 + 失败回滚（决定即落库；单条/批量同接口） -------------------------
  const [rollbackAt, setRollbackAt] = useState<{ assetId: number; decision: CullDecision } | null>(null);
  // 回滚提示自动消退（2.5s；不阻塞继续过片）
  useEffect(() => {
    if (rollbackAt === null) return undefined;
    const timer = window.setTimeout(() => setRollbackAt(null), 2500);
    return () => clearTimeout(timer);
  }, [rollbackAt]);

  /** 批量写决定（单条过片 / 对比格 / 连拍组一键留张共用）；用户手写 → origin manual */
  const applyDecisions = useCallback(
    (patches: CullDecisionPatch[], options?: { advance?: boolean }) => {
      if (patches.length === 0) return;
      const prevEntries = patches.map(
        (p) => [p.assetId, decisions.get(p.assetId) ?? null] as const,
      );
      const prevOrigins = patches.map(
        (p) => [p.assetId, origins.get(p.assetId) ?? "manual"] as const,
      );
      setDecisions((cur) => {
        let next = cur;
        for (const p of patches) next = withDecision(next, p.assetId, p.decision);
        return next;
      });
      // 人工决定覆盖 AI 预标记 → origin 转手动（角标样式随之收敛）
      if (patches.some((p) => p.decision !== null)) {
        setOrigins((cur) => {
          const next = new Map(cur);
          for (const p of patches) if (p.decision !== null) next.set(p.assetId, "manual");
          return next;
        });
      }
      void cullDecisionApply(session.id, patches).then((dto) => {
        if (dto === null || dto.id !== session.id) {
          // 传输失败/脏数据：回滚本批决定（进度以本地态继续派生）
          setDecisions((cur) => {
            let next = cur;
            for (const [id, decision] of prevEntries) next = withDecision(next, id, decision);
            return next;
          });
          setOrigins((cur) => {
            const next = new Map(cur);
            for (const [id, origin] of prevOrigins) next.set(id, origin);
            return next;
          });
          setRollbackAt({ assetId: prevEntries[0][0], decision: prevEntries[0][1] });
          return;
        }
        setSession(dto); // 计数对齐后端真值
      });
      if (options?.advance) setIndex((cur) => indexAfterDecision(cur, ids.length));
    },
    [decisions, ids.length, origins, session.id],
  );

  const applyDecision = useCallback(
    (decision: CullDecision, options?: { advance?: boolean }) => {
      if (currentId === null) return;
      applyDecisions([{ assetId: currentId, decision }], options);
    },
    [currentId, applyDecisions],
  );

  /** V2 连拍组「本组只留这张」：当前 accepted + 组内未定项 rejected（一次批量写） */
  const keepOnlyThis = useCallback(() => {
    if (items === null || currentId === null) return;
    applyDecisions(buildKeepOnlyDecisions(items, currentId, decisions));
  }, [items, currentId, decisions, applyDecisions]);

  const navigate = useCallback(
    (next: number) => {
      if (items === null || items.length === 0) return;
      setIndex(Math.min(Math.max(0, next), items.length - 1));
      setFocusedPane(0); // 对比模式下胶片条跳转 = 重新锚定组员，焦点回首位
    },
    [items],
  );

  // --- 相邻预取（翻片瞬时显示） --------------------------------------------------------
  useEffect(() => {
    if (ids.length === 0) return;
    const neighbors = [ids[index - 1], ids[index + 1]];
    for (const id of neighbors) {
      if (id === undefined) continue;
      const meta = assetById.get(id);
      if (meta?.kind === "photo") warmImageDecode(safeConvert(meta.path));
      void fetchAssetThumb(id, VIEWER_THUMB_SIZE, "high").then((r) => {
        if (r.kind === "url") warmImageDecode(r.url);
      });
    }
  }, [ids, index, assetById]);

  // --- V2 对比视图：组员选取（同连拍组优先、不足补相邻） --------------------------------
  /** 进出对比不动 index——单图位置保持 */
  const comparePaneIndices = useMemo(() => {
    if (items === null) return [];
    return comparisonMembers(items, index, compareCount);
  }, [items, index, compareCount]);
  const comparePanes = useMemo(() => {
    if (items === null) return [];
    return comparePaneIndices.map((i) => {
      const item = items[i];
      return {
        assetId: item.assetId,
        name: assetMeta.get(item.assetId)?.name ?? `#${i + 1}`,
      };
    });
  }, [items, comparePaneIndices, assetMeta]);
  const effectiveFocusedPane = Math.min(focusedPane, Math.max(0, comparePanes.length - 1));

  function enterCompare(): void {
    exitZoom(true); // 对比无放大；锚点入记忆（回单图再按 Z 原位复放）
    setMode("compare");
    setFocusedPane(0);
  }
  function exitCompare(): void {
    setMode("single");
  }
  function toggleCompare(): void {
    if (mode === "single") enterCompare();
    else exitCompare();
  }
  function handleCompareDecide(assetId: number, decision: CullDecision): void {
    applyDecisions([{ assetId, decision }]);
  }

  // --- AI 行内提示（闭眼/失焦 chips；asset_detail 已有字段） ---------------------------
  const [aiDetail, setAiDetail] = useState<AssetDetailDto | null>(null);
  useEffect(() => {
    let cancelled = false;
    setAiDetail(null);
    if (currentId === null) return undefined;
    void assetDetail(currentId).then((detail) => {
      if (!cancelled) setAiDetail(detail);
    });
    return () => {
      cancelled = true;
    };
  }, [currentId]);
  const aiChips = useMemo(() => {
    const ai = aiDetail?.aiAnalysis;
    if (ai === null || ai === undefined) return [];
    const chips: Array<{ key: string; tone: "red" | "amber" }> = [];
    if (ai.eyes?.value === "closed") chips.push({ key: "eyesClosed", tone: "red" });
    else if (ai.eyes?.value === "maybe") chips.push({ key: "eyesMaybe", tone: "amber" });
    if (ai.blur?.value === "soft") chips.push({ key: "blurSoft", tone: "amber" });
    return chips;
  }, [aiDetail]);

  // --- V3 AI 挑图弹窗 + 应用后刷新 -----------------------------------------------------
  const [aiOpen, setAiOpen] = useState(false);
  const [aiToast, setAiToast] = useState<CullPrescanDto | null>(null);
  useEffect(() => {
    if (aiToast === null) return undefined;
    const timer = window.setTimeout(() => setAiToast(null), 4000);
    return () => clearTimeout(timer);
  }, [aiToast]);

  /** 应用成功：关弹窗 + toast + 决定表/origin/计数按后端真值刷新（保持当前位） */
  function handleAiApplied(result: CullPrescanDto): void {
    setAiOpen(false);
    setAiToast(result);
    void cullSessionOpen(session.id).then((opened) => {
      if (opened !== null) applyOpened(opened, false);
    });
  }

  // --- 收尾弹窗 ------------------------------------------------------------------------
  const [finishOpen, setFinishOpen] = useState(false);
  const currentDecision = currentId !== null ? decisions.get(currentId) ?? null : null;
  const currentOrigin = currentId !== null ? origins.get(currentId) ?? "manual" : "manual";

  // --- 键盘接管（window 捕获 + stopPropagation；收尾/AI 弹窗打开时让位） ----------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (finishOpen || aiOpen) return; // 弹窗自处理 Esc（其自身捕获监听）
      const target = e.target;
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
      ) {
        return;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        exitZoom(false);
        onClose();
        return;
      }
      if (e.key === "z" || e.key === "Z") {
        if (e.ctrlKey || e.metaKey || e.altKey) return;
        e.preventDefault();
        e.stopPropagation();
        if (mode === "single") enterZoom(); // 对比模式无放大
        return;
      }
      // 对比模式：数字键 1-4 选焦（屏位；越界收夹）
      if (mode === "compare" && ["1", "2", "3", "4"].includes(e.key)) {
        e.preventDefault();
        e.stopPropagation();
        const pane = Number(e.key) - 1;
        if (comparePanes.length > 0) setFocusedPane(Math.min(pane, comparePanes.length - 1));
        return;
      }
      const action = cullKeyAction(e);
      if (action === null) return;
      e.preventDefault();
      e.stopPropagation();
      if (items === null || items.length === 0) return;
      // 对比模式：空格/X/U 作用于焦点张（不翻页）；←/→ 切焦
      const focusedAssetId =
        mode === "compare" && comparePanes.length > 0
          ? comparePanes[effectiveFocusedPane]?.assetId ?? null
          : null;
      switch (action) {
        case "compare":
          toggleCompare();
          break;
        case "prev":
          if (mode === "compare") setFocusedPane((cur) => Math.max(0, cur - 1));
          else {
            setIndex((cur) => Math.max(0, cur - 1));
            setFocusedPane(0);
          }
          break;
        case "next":
          if (mode === "compare") {
            setFocusedPane((cur) => Math.min(comparePanes.length - 1, cur + 1));
          } else {
            setIndex((cur) => Math.min(items.length - 1, cur + 1));
            setFocusedPane(0);
          }
          break;
        case "accept":
          if (mode === "compare") {
            if (focusedAssetId !== null) handleCompareDecide(focusedAssetId, "accepted");
          } else {
            applyDecision("accepted", { advance: true });
          }
          break;
        case "reject":
          if (mode === "compare") {
            if (focusedAssetId !== null) handleCompareDecide(focusedAssetId, "rejected");
          } else {
            applyDecision("rejected", { advance: true });
          }
          break;
        case "undecided":
          if (mode === "compare") {
            if (focusedAssetId !== null) handleCompareDecide(focusedAssetId, null);
          } else {
            applyDecision(null);
          }
          break;
      }
    };
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === "z" || e.key === "Z") exitZoom(true); // 松开恢复常态 + 记住锚点
    };
    window.addEventListener("keydown", onKey, { capture: true });
    window.addEventListener("keyup", onKeyUp, { capture: true });
    return () => {
      window.removeEventListener("keydown", onKey, { capture: true });
      window.removeEventListener("keyup", onKeyUp, { capture: true });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    aiOpen,
    applyDecision,
    applyDecisions,
    comparePanes,
    effectiveFocusedPane,
    finishOpen,
    index,
    items,
    mode,
    navigate,
    onClose,
  ]);

  const progressPct = progress.total > 0 ? ((progress.accepted + progress.rejected) / progress.total) * 100 : 0;

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={session.name}
      data-theme="dark"
      data-testid="culling-overlay"
      data-mode={mode}
    >
      {/* 顶栏：拖拽层 + 会话名 + 进度计数 + 模式切换 + AI 挑图 + 完成选片 + 退出 */}
      <div className="ui-glass relative flex h-12 shrink-0 items-center gap-3 border-b border-edge px-4 text-text-primary">
        <div className="absolute inset-0" data-tauri-drag-region />
        <div className="pointer-events-none relative flex min-w-0 items-baseline gap-3">
          <span className="truncate text-sm font-semibold" title={session.name} data-testid="culling-name">
            {session.name}
          </span>
          <span
            className="shrink-0 font-mono text-xs tabular-nums text-text-secondary"
            data-testid="culling-progress"
            data-accepted={progress.accepted}
            data-rejected={progress.rejected}
            data-undecided={progress.undecided}
            data-total={progress.total}
          >
            {t("culling.overlay.progress", {
              accepted: progress.accepted,
              rejected: progress.rejected,
              undecided: progress.undecided,
              total: progress.total,
            })}
          </span>
        </div>
        {rollbackAt !== null && (
          <span
            className="pointer-events-none relative rounded bg-red-500/15 px-1.5 py-0.5 text-[10px] text-red-300"
            role="alert"
            data-testid="culling-rollback"
          >
            {t("culling.overlay.rollback")}
          </span>
        )}
        <div className="pointer-events-none relative ml-auto flex items-center gap-2">
          {/* 单图/对比切换（C 键等价物） */}
          <div
            className="pointer-events-auto flex overflow-hidden rounded-md border border-edge"
            role="group"
            aria-label={t("culling.overlay.modeLabel")}
            data-testid="culling-mode-toggle"
          >
            <button
              type="button"
              aria-pressed={mode === "single"}
              onClick={exitCompare}
              className={`px-2.5 py-1 text-xs transition-colors ${
                mode === "single"
                  ? "bg-accent font-medium text-black"
                  : "text-text-secondary hover:bg-panel hover:text-text-primary"
              }`}
              data-testid="culling-mode-single"
              data-active={mode === "single"}
            >
              {t("culling.overlay.modeSingle")}
            </button>
            <button
              type="button"
              aria-pressed={mode === "compare"}
              onClick={enterCompare}
              className={`px-2.5 py-1 text-xs transition-colors ${
                mode === "compare"
                  ? "bg-accent font-medium text-black"
                  : "text-text-secondary hover:bg-panel hover:text-text-primary"
              }`}
              data-testid="culling-mode-compare"
              data-active={mode === "compare"}
            >
              {t("culling.overlay.modeCompare")}
              <span className="ml-1 font-mono text-[10px] opacity-60">C</span>
            </button>
          </div>
          {/* 同屏张数（仅对比模式） */}
          {mode === "compare" && (
            <div
              className="pointer-events-auto flex overflow-hidden rounded-md border border-edge"
              role="group"
              aria-label={t("culling.overlay.compareCountLabel")}
              data-testid="culling-compare-counts"
            >
              {[2, 3, 4].map((n) => (
                <button
                  key={n}
                  type="button"
                  aria-pressed={compareCount === n}
                  onClick={() => setCompareCount(n as 2 | 3 | 4)}
                  className={`w-7 py-1 font-mono text-xs transition-colors ${
                    compareCount === n
                      ? "bg-accent font-medium text-black"
                      : "text-text-secondary hover:bg-panel hover:text-text-primary"
                  }`}
                  data-testid="culling-compare-count"
                  data-count={n}
                  data-active={compareCount === n}
                >
                  {n}
                </button>
              ))}
            </div>
          )}
          <button
            type="button"
            onClick={() => setAiOpen(true)}
            className="pointer-events-auto rounded-md border border-sky-400/60 px-2.5 py-1 text-xs text-sky-300 transition-colors hover:border-sky-300 hover:text-sky-200"
            data-testid="culling-ai-open"
          >
            {t("culling.ai.entry")}
          </button>
          <button
            type="button"
            onClick={() => setFinishOpen(true)}
            disabled={items === null || items.length === 0}
            className="pointer-events-auto rounded-md bg-accent px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110 disabled:opacity-40"
            data-testid="culling-finish"
          >
            {t("culling.overlay.finish")}
          </button>
          <button
            type="button"
            onClick={onClose}
            aria-label={t("culling.overlay.exit")}
            title={t("culling.overlay.exit")}
            className="pointer-events-auto rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="culling-exit"
          >
            {t("culling.overlay.exit")}
          </button>
        </div>
      </div>

      {/* 进度条（已决定占比；已选/已剔除分色） */}
      <div className="flex h-1.5 shrink-0 gap-px bg-panel/60" data-testid="culling-progressbar">
        <div
          className="h-full bg-emerald-500 transition-[width] duration-150"
          style={{ width: `${progress.total > 0 ? (progress.accepted / progress.total) * 100 : 0}%` }}
        />
        <div
          className="h-full bg-red-500 transition-[width] duration-150"
          style={{ width: `${progress.total > 0 ? (progress.rejected / progress.total) * 100 : 0}%` }}
        />
        <span className="sr-only">{Math.round(progressPct)}%</span>
      </div>

      {/* 主区：单图大图 / 对比网格 */}
      <div className="relative min-h-0 flex-1" data-testid="culling-stage-pane">
        {openFailed ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-text-muted" data-testid="culling-open-failed">
            <p className="text-sm text-text-secondary">{t("culling.overlay.openFailed")}</p>
            <button
              type="button"
              onClick={onClose}
              className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            >
              {t("culling.overlay.exit")}
            </button>
          </div>
        ) : items !== null && items.length === 0 ? (
          <div className="flex h-full flex-col items-center justify-center gap-3 text-text-muted" data-testid="culling-empty">
            <p className="text-sm text-text-secondary">{t("culling.overlay.empty")}</p>
            <button
              type="button"
              onClick={onClose}
              className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            >
              {t("culling.overlay.exit")}
            </button>
          </div>
        ) : mode === "compare" ? (
          <CullCompareGrid
            panes={comparePanes}
            count={compareCount}
            decisions={decisions}
            origins={origins}
            assetMeta={assetMeta}
            focusedPane={effectiveFocusedPane}
            onFocusPane={setFocusedPane}
            onDecide={handleCompareDecide}
          />
        ) : (
          <div
            ref={stageRef}
            className="absolute inset-0 flex touch-none items-center justify-center overflow-hidden"
            onMouseMove={handleStageMouseMove}
            onDoubleClick={() => exitZoom(false)}
            style={{ cursor: zoomed ? "zoom-in" : "default" }}
            data-testid="culling-stage"
            data-zoomed={zoomed}
            data-zoom-at={`${zoomAt.x.toFixed(3)},${zoomAt.y.toFixed(3)}`}
          >
            <>
              {imageLayers.map((layer) => (
                <img
                  key={layer.src}
                  src={layer.src}
                  alt={layer.phase === "active" ? asset?.name ?? "" : ""}
                  aria-hidden={layer.phase === "active" ? undefined : "true"}
                  draggable={false}
                  loading="eager"
                  decoding="async"
                  onLoad={
                    layer.phase === "loading"
                      ? (event) => {
                          const image = event.currentTarget;
                          void (async () => {
                            try {
                              if (typeof image.decode === "function") await image.decode();
                            } catch {
                              // 部分编码器可显示但拒绝 decode；继续交下一帧
                            }
                            await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
                            setImageLayers((previous) => {
                              const target = previous.find((item) => item.src === layer.src);
                              if (!target) return previous;
                              return previous.map((item) =>
                                item.src === layer.src
                                  ? { ...item, phase: "active" }
                                  : { ...item, phase: "retiring" },
                              );
                            });
                          })();
                        }
                      : undefined
                  }
                  onError={
                    layer.phase === "loading"
                      ? () => {
                          // 分级降档：原图失败 → 2048 → 512
                          if (asset?.kind === "photo" && photoStage === "original") setPhotoStage("mid");
                          else if (asset?.kind === "photo" && photoStage === "mid") setPhotoStage("thumb");
                        }
                      : undefined
                  }
                  data-testid={layer.phase === "active" ? "culling-img-active" : "culling-img-loading"}
                  className={`${layer.phase === "loading" ? "invisible " : "absolute "}max-h-full max-w-full select-none object-contain`}
                  style={{
                    transform: `scale(${zoomed ? ZOOM_SCALE : 1})`,
                    transformOrigin: `${zoomAt.x * 100}% ${zoomAt.y * 100}%`,
                    transition: "transform 120ms ease-out",
                  }}
                />
              ))}
              {imageLayers.length === 0 && items !== null && !showMissingPlaceholder && (
                <div
                  className="h-10 w-10 animate-spin rounded-full border-2 border-edge border-t-accent"
                  data-testid="culling-loading"
                />
              )}
              {imageLayers.length === 0 && mainFailed && (
                <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="culling-no-preview">
                  <span className="text-xs">{t("culling.overlay.noPreview")}</span>
                </div>
              )}
              {/* 源缺失：小角标恒在（有缓存图也标）；无缓存时居中图标+文案占位 */}
              {missing && (
                <div
                  className="pointer-events-none absolute left-1/2 top-3 z-10 flex -translate-x-1/2 items-center gap-1.5 rounded-md border border-amber-400/50 bg-amber-500/15 px-2.5 py-1 text-[11px] text-amber-300"
                  role="status"
                  data-testid="cull-pane-missing"
                >
                  {t("thumb.missingBadge")}
                </div>
              )}
              {showMissingPlaceholder && (
                <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="cull-pane-missing-placeholder">
                  <svg viewBox="0 0 24 24" width="48" height="48" fill="none" stroke="currentColor" strokeWidth="1.2" strokeLinecap="round" strokeLinejoin="round" className="text-amber-400" aria-hidden="true">
                    <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                    <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                    <path d="M14.5 5.5l4 4M18.5 5.5l-4 4" />
                  </svg>
                  <span className="text-xs text-amber-300">{t("viewer.missingSource")}</span>
                </div>
              )}

              {/* 连拍组徽标 + 一键留张（burstSize≥2；方案 §3.1/§3.2 组集成） */}
              {currentBurst !== null && currentBurst.burstSize >= 2 && (
                <div
                  className="absolute left-3 top-3 flex items-center gap-1.5"
                  data-testid="culling-burst"
                  data-size={currentBurst.burstSize}
                  data-burst-id={currentBurst.burstId ?? ""}
                >
                  <span
                    className="rounded-full bg-black/60 px-2 py-0.5 text-[11px] font-medium text-text-primary"
                    data-testid="culling-burst-badge"
                  >
                    {t("culling.overlay.burstBadge", { count: currentBurst.burstSize })}
                  </span>
                  <button
                    type="button"
                    onClick={keepOnlyThis}
                    className="rounded-full border border-edge bg-black/60 px-2 py-0.5 text-[11px] text-text-secondary transition-colors hover:border-accent hover:text-accent"
                    data-testid="culling-burst-keep"
                  >
                    {t("culling.overlay.burstKeep")}
                  </button>
                </div>
              )}

              {/* AI 行内提示 chips（闭眼/失焦；AI 只建议） */}
              {aiChips.length > 0 && (
                <div
                  className="pointer-events-none absolute bottom-3 left-3 flex flex-wrap gap-1.5"
                  data-testid="culling-ai-chips"
                >
                  {aiChips.map((chip) => (
                    <span
                      key={chip.key}
                      className={`flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium shadow ${
                        chip.tone === "red" ? "bg-red-500/85 text-white" : "bg-amber-400/90 text-black"
                      }`}
                      data-testid="culling-ai-chip"
                      data-kind={chip.key}
                    >
                      {t(`culling.overlay.ai.${chip.key}`)}
                      <span className="rounded bg-black/25 px-1 text-[8px] font-semibold leading-3">
                        {t("culling.overlay.ai.badge")}
                      </span>
                    </span>
                  ))}
                </div>
              )}

              {/* 当前决定态角标（AI 预标记 = 蓝描边 + AI 小标；手动改过转 manual） */}
              {currentDecision !== null && (
                <div
                  className={`pointer-events-none absolute right-3 top-3 flex items-center gap-1 rounded-full px-2.5 py-1 text-xs font-semibold shadow ${
                    currentDecision === "accepted"
                      ? "bg-emerald-500 text-black"
                      : "bg-red-500 text-white"
                  } ${currentOrigin === "ai" ? "ring-2 ring-sky-400" : ""}`}
                  data-testid="culling-decision-badge"
                  data-decision={currentDecision}
                  data-origin={currentOrigin}
                >
                  {currentDecision === "accepted"
                    ? t("culling.overlay.decisionAccepted")
                    : t("culling.overlay.decisionRejected")}
                  {currentOrigin === "ai" && (
                    <span
                      className="rounded bg-black/25 px-1 text-[8px] font-semibold leading-3"
                      data-testid="culling-decision-ai"
                    >
                      {t("culling.overlay.ai.badge")}
                    </span>
                  )}
                </div>
              )}

              {/* 左右翻片 */}
              <button
                type="button"
                onClick={() => navigate(index - 1)}
                disabled={index <= 0}
                aria-label={t("culling.overlay.prev")}
                className="pointer-events-auto absolute left-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
                data-testid="culling-prev"
              >
                <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M10 3L5 8l5 5" />
                </svg>
              </button>
              <button
                type="button"
                onClick={() => navigate(index + 1)}
                disabled={items === null || index >= items.length - 1}
                aria-label={t("culling.overlay.next")}
                className="pointer-events-auto absolute right-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
                data-testid="culling-next"
              >
                <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M6 3l5 5-5 5" />
                </svg>
              </button>

              {/* 键位提示（常驻弱样式） */}
              <p
                className="pointer-events-none absolute bottom-3 left-1/2 -translate-x-1/2 rounded bg-black/40 px-2 py-0.5 font-mono text-[10px] text-text-muted/80"
                data-testid="culling-keys-hint"
              >
                {t("culling.overlay.keysHint")}
              </p>
            </>
          </div>
        )}
      </div>

      {/* 底部：决定操作条（键盘等价物；对比模式的张上按钮在格内） */}
      {mode === "single" && (
        <div
          className="flex h-12 shrink-0 items-center justify-center gap-2 border-t border-edge bg-surface/80 px-4"
          data-testid="culling-actions"
        >
          <span className="mr-2 font-mono text-[11px] tabular-nums text-text-muted" data-testid="culling-index">
            {items !== null && items.length > 0
              ? t("culling.overlay.index", { index: index + 1, total: items.length })
              : "—"}
          </span>
          <button
            type="button"
            onClick={() => applyDecision("accepted", { advance: true })}
            className="rounded-md bg-emerald-500 px-3 py-1.5 text-xs font-medium text-black transition-colors hover:brightness-110"
            data-testid="culling-accept"
          >
            {t("culling.overlay.accept")} <span className="font-mono opacity-60">{t("shortcuts.space")}</span>
          </button>
          <button
            type="button"
            onClick={() => applyDecision(null)}
            className="rounded-md border border-edge px-3 py-1.5 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="culling-undecided"
          >
            {t("culling.overlay.undecided")} <span className="font-mono opacity-60">U</span>
          </button>
          <button
            type="button"
            onClick={() => applyDecision("rejected", { advance: true })}
            className="rounded-md bg-red-500 px-3 py-1.5 text-xs font-medium text-white transition-colors hover:brightness-110"
            data-testid="culling-reject"
          >
            {t("culling.overlay.reject")} <span className="font-mono opacity-60">X</span>
          </button>
        </div>
      )}

      {/* 底部胶片条（虚拟滚动 + 决定角标 + 点击跳转；对比模式同样实时同步） */}
      {items !== null && items.length > 0 && (
        <CullFilmstrip
          ids={ids}
          decisions={decisions}
          origins={origins}
          assetMeta={assetMeta}
          index={index}
          onJump={navigate}
        />
      )}

      {/* AI 应用摘要 toast（进度刷新真值来自重开） */}
      {aiToast !== null && (
        <div
          className="fixed bottom-24 left-1/2 z-[70] -translate-x-1/2 rounded-lg border border-sky-400/50 bg-surface px-4 py-2.5 text-xs shadow-2xl"
          role="status"
          data-testid="culling-ai-toast"
          data-accepted={aiToast.suggestedAccepted}
          data-rejected={aiToast.suggestedRejected}
        >
          {t("culling.ai.appliedToast", {
            accepted: aiToast.suggestedAccepted,
            rejected: aiToast.suggestedRejected,
          })}
        </div>
      )}

      {/* AI 挑图弹窗（打开时本层键盘让位） */}
      {aiOpen && (
        <CullAiDialog
          session={session}
          onClose={() => setAiOpen(false)}
          onApplied={handleAiApplied}
        />
      )}

      {/* 收尾映射弹窗（打开时本层键盘让位） */}
      {finishOpen && (
        <CullFinishDialog
          session={session}
          progress={progress}
          onClose={() => setFinishOpen(false)}
          onApplied={(result) => onFinished(result, session.name)}
          onFailed={onFinishFailed}
        />
      )}
    </div>
  );
}
