import { AnimatePresence, motion } from "motion/react";

import { useViewerTransform } from "../lib/useViewerTransform";
import { motionInitial, TRANS, useMotionOn } from "@/lib/motion";
import { useViewerImage } from "../lib/useViewerImage";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  assetDetail,
  assetFlagSet,
  assetLabelSet,
  assetRatingSet,
  assetRejectSet,
  assetVersions,
  editRecipeGet,
  type AssetDetailDto,
  type AssetDto,
  type AssetVersions,
  type EditRecipe,
  type VersionMember,
} from "@/ipc/api";
import EditorOverlay from "@/features/editor/components/EditorOverlay";
import { asColorLabel, COLOR_DOT_CLASS, COLOR_DOT_RING, COLOR_LABELS, type ColorLabel } from "../lib/colorLabels";
import type { AssetGroup } from "../lib/assetGroups";
import { AssetContextMenu } from "./ContextMenu";
import AssetThumb from "./AssetThumb";
import { formatBytes } from "@/lib/format";

/**
 * 全屏沉浸查看器（M3）：
 * - 大图策略按 kind：photo 优先原图 asset 协议（convertFileSrc(path)），加载失败回退
 *   大档缩略图（名义 1280，后端 snap 512）；RAW 无可载原图（inline-JPEG 提取在 M4），
 *   直接用大档缩略图放大显示。
 * - 交互：wheel 以指针为锚缩放 1x-4x（原生非 passive 监听），scale>1 可拖拽平移，
 *   90° 步进旋转（按钮 / 键盘 . , R，transform 顺序 rotate→scale→translate，
 *   150ms 过渡），双击复位（含旋转与平移）；←/→ 同组切换（首尾禁用）；
 *   Esc 返回画廊（画廊页不卸载，滚动位置保留）。旋转随资产切换重置，不持久化。
 * - 切图在不可见解码层完成后原子替换可见图层，避免新旧图片交叉叠显。
 * - 右侧 EXIF 面板可收起；底部只显示当前照片前后各 8 张缩略图，
 *   点击最多跳 8 张，没有横向滚动；相邻照片预取。
 */

const FILM_THUMB_SIZE = 240;

/** 切图首帧即可显示的基础信息；完整 EXIF 返回后在原位补齐，不切成骨架屏。 */
function detailFromAsset(asset: AssetDto): AssetDetailDto {
  return {
    id: asset.id,
    path: asset.path,
    filename: asset.name,
    size: asset.sizeBytes,
    kind: asset.kind,
    capturedAt: asset.capturedAt,
    camera: asset.camera,
    createdAt: null,
    dupCount: 0,
    aiAnalysis: null,
  };
}

/** 路径 → asset 协议 URL（非 Tauri 环境抛错回退 null） */

function isoLabel(iso: string | null): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())} ` +
    `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`
  );
}

/** 文本元数据容错：null/undefined/空串 → "—"（绝不渲染 "undefined"） */
function formatValue(value: string | null | undefined): string {
  return value === null || value === undefined || value === "" ? "—" : value;
}

/** 快门格式化："1/250" → "1/250s"（已带 s 或描述性文本原样透传） */
function formatShutter(value: string): string {
  const plain = value.trim().replace(/s$/i, "");
  const seconds = Number(plain);
  if (!Number.isFinite(seconds) || seconds <= 0 || plain.includes("/")) {
    return /s$/i.test(value) ? value : `${value}s`;
  }
  if (seconds >= 1) return `${plain}s`;
  const denominator = Math.round(1 / seconds);
  return denominator > 1 ? `1/${denominator}s` : `${plain}s`;
}

/** EXIF 枚举 token → 中文（后端 exif_lite 归一 token 族；未知 token 原样） */
const FLASH_ZH: Record<string, string> = {
  fired: "viewer.value.flash_zh.fired",
  no_flash: "viewer.value.flash_zh.no_flash",
  no_flash_function_not_fired: "viewer.value.flash_zh.no_flash_function_not_fired",
  flash_fired_compulsory: "viewer.value.flash_zh.flash_fired_compulsory",
};
const METERING_ZH: Record<string, string> = {
  average: "viewer.value.metering_zh.average",
  center_weighted_average: "viewer.value.metering_zh.center_weighted_average",
  pattern: "viewer.value.metering_zh.pattern",
  spot: "viewer.value.metering_zh.spot",
  multi_spot: "viewer.value.metering_zh.multi_spot",
  partial: "viewer.value.metering_zh.partial",
};
const WB_ZH: Record<string, string> = { auto: "viewer.value.wb_zh.auto", manual: "viewer.value.wb_zh.manual" };
const PROGRAM_ZH: Record<string, string> = {
  manual: "viewer.value.program_zh.manual",
  program_auto: "viewer.value.program_zh.program_auto",
  aperture_priority: "viewer.value.program_zh.aperture_priority",
  shutter_priority: "viewer.value.program_zh.shutter_priority",
  creative: "viewer.value.program_zh.creative",
  action: "viewer.value.program_zh.action",
  portrait_mode: "viewer.value.program_zh.portrait_mode",
  landscape_mode: "viewer.value.program_zh.landscape_mode",
  bulb: "viewer.value.program_zh.bulb",
};

/** 枚举字段展示：token 查表，非 token（自由文本）原样，空 → "—" */
function formatToken(value: string | null | undefined, table: Record<string, string>, translate: (key: string) => string): string {
  if (value === null || value === undefined || value === "") return "—";
  return table[value] ? translate(table[value]) : value;
}

/** EXIF orientation（1-8）→ 拍摄方向：5-8 为竖拍（含镜像竖拍），1-4 为横拍 */
function isPortraitOrientation(orientation: number): boolean {
  return Number.isInteger(orientation) && orientation >= 5 && orientation <= 8;
}

/** 版本成员角色 → 文案键（B2；成片角色即「成片」标） */
const VERSION_ROLE_KEYS: Record<NonNullable<VersionMember["role"]>, string> = {
  raw: "viewer.version.raw",
  sooc: "viewer.version.sooc",
  derived: "viewer.version.derived",
};

/** 详情面板行/组（LR 式分组：文件 / 图像 / 拍摄 / 位置） */
interface ExifRow {
  label: string;
  value: React.ReactNode;
}
interface ExifSection {
  key: "file" | "image" | "camera" | "location";
  rows: ExifRow[];
}

interface ViewerOverlayProps {
  asset: AssetDto;
  group: AssetGroup;
  index: number;
  /** 当前结果集内切换（缩略图/箭头/键盘） */
  onNavigate: (index: number) => void;
  onClose: () => void;
  /** 本视图内的资产标记变更回传（颜色标签/拒绝旗标；上层同步网格态，可选） */
  onAssetPatched?: (id: number, patch: Partial<AssetDto>) => void;
  /** 版本切换（B2：版本区 chips 点击；上层换 asset；不传则版本区只读不渲染交互） */
  onVersionSelect?: (assetId: number) => void;
}

export default function ViewerOverlay({ asset, group, index, onNavigate, onClose, onAssetPatched, onVersionSelect }: ViewerOverlayProps) {
  const { t } = useTranslation();
  const motionOn = useMotionOn();
  const { stageRef, view, rotate, resetView, toggleZoom, dragging,
    handlePointerDown, handlePointerMove, handlePointerUp } = useViewerTransform(asset.id);
  const [fullscreen, setFullscreen] = useState(false);
  const [exifOpen, setExifOpen] = useState(true);
  const toggleFullscreen = useCallback(async () => {
    if (isTauri()) {
      const next = !(await getCurrentWindow().isFullscreen());
      await getCurrentWindow().setFullscreen(next);
      setFullscreen(next);
      if (next) setExifOpen(false);
    } else if (document.fullscreenElement) {
      await document.exitFullscreen();
      setFullscreen(false);
    } else if (document.documentElement.requestFullscreen) {
      await document.documentElement.requestFullscreen();
      setFullscreen(true);
      setExifOpen(false);
    }
  }, []);
  // 自含退场动画（2026-09-29 动画批次）：closing 态 150ms 淡出后再调 onClose 卸载，
  // 7 个挂载页保持纯条件渲染即可；动画关（或已在退场中）立即关闭。
  const [closing, setClosing] = useState(false);
  const closeViewer = useCallback(() => {
    if (fullscreen) {
      if (isTauri()) void getCurrentWindow().setFullscreen(false);
      else if (document.fullscreenElement) void document.exitFullscreen();
    }
    if (!motionOn || closing) {
      onClose();
      return;
    }
    setClosing(true);
    window.setTimeout(() => onClose(), 150);
  }, [fullscreen, onClose, motionOn, closing]);

  // 切图方向（渲染期从 props 推导，不用 effect——新 key 挂载时 initial 要立即拿到）
  const lastIndexRef = useRef(index);
  const navDirRef = useRef(0);
  if (index !== lastIndexRef.current) {
    navDirRef.current = index > lastIndexRef.current ? 1 : -1;
    lastIndexRef.current = index;
  }
  useEffect(() => {
    const sync = () => setFullscreen(Boolean(document.fullscreenElement));
    document.addEventListener("fullscreenchange", sync);
    return () => document.removeEventListener("fullscreenchange", sync);
  }, []);

  // 大图右键菜单（与瓦片同款：reveal/复制文件/旗标，作用于当前资产）
  const [ctxAt, setCtxAt] = useState<{ x: number; y: number } | null>(null);

  // 操作提示（#6）：首次 3s 后淡出；? 键或 hover 底部提示区重新唤出（再计时 3s）
  const [hintVisible, setHintVisible] = useState(true);
  const hintTimerRef = useRef<number | null>(null);
  const showHint = useCallback(() => {
    setHintVisible(true);
    if (hintTimerRef.current !== null) clearTimeout(hintTimerRef.current);
    hintTimerRef.current = window.setTimeout(() => setHintVisible(false), 3000);
  }, []);
  useEffect(() => {
    showHint();
    return () => {
      if (hintTimerRef.current !== null) clearTimeout(hintTimerRef.current);
    };
  }, [showHint]);

  // --- 大图来源（按 kind，分级回退链） -------------------------------------------------
  // photo：原图 → 中间档缩略图（名义 2048，后端加 2048 档）→ 512 档。WebView2 对
  // 大尺寸/无压缩 TIF 等原生渲染失败，onError 逐级降档，而不是一步到 512 模糊图。
  // raw（Windows 照片同款方案）：512 内嵌 JPEG 秒出 → 最大内嵌全幅 JPEG（raw-embed
  // 直出档，毫秒级 IO）替换变清晰 → 仅当相机没存内嵌预览时才回落 2048 rawler 显影。
  const { stage, setStage, mainSrc, mainFailed, mainMissing, showMissingPlaceholder, sourceKind,
    imageLayers, setImageLayers, slowLoading } = useViewerImage(asset, group, index);

  // --- EXIF 面板 ---------------------------------------------------------------------
  const [detail, setDetail] = useState<AssetDetailDto | null>(() => detailFromAsset(asset));
  // 切图闪缩修复（M4.5）：切换资产时不回退「基本行集」（行数骤减→面板高度跳变），
  // 保留上一份完整详情的行结构直到新详情到达（值随后一次更新，行不重挂）。
  const visibleDetail = detail ?? detailFromAsset(asset);
  useEffect(() => {
    let cancelled = false;
    void assetDetail(asset.id).then((d) => {
      if (cancelled) return;
      if (d) {
        setDetail(d);
      }
    });
    return () => {
      cancelled = true;
    };
  }, [asset.id]);

  // --- 星标条（M4.5）：详情面板顶部 0-5 星点选（assetRatingSet；再点同星=清除） ---
  const [ratingDraft, setRatingDraft] = useState<number | null>(null);
  useEffect(() => {
    setRatingDraft(null);
  }, [asset.id]);
  const currentRating = Math.max(
    0,
    Math.min(5, Math.round(ratingDraft ?? detail?.rating ?? 0)),
  );
  async function applyRating(value: number, toggleSame = true): Promise<void> {
    const next = toggleSame && currentRating === value ? 0 : value; // 鼠标再点同一星=清除
    setRatingDraft(next);
    await assetRatingSet(asset.id, next);
  }

  const [flagDraft, setFlagDraft] = useState<boolean | null>(null);
  useEffect(() => {
    setFlagDraft(null);
  }, [asset.id]);
  const currentFlagged = flagDraft ?? detail?.flagged ?? false;
  async function applyFlagged(flagged: boolean): Promise<void> {
    setFlagDraft(flagged);
    await assetFlagSet(asset.id, flagged);
  }

  // --- 颜色标签（B1，LR 五色标）：星级行旁色点点开设置（单资产） -----------------------
  const [colorDraft, setColorDraft] = useState<string | null | undefined>(undefined);
  useEffect(() => {
    setColorDraft(undefined);
  }, [asset.id]);
  const currentColor = colorDraft !== undefined ? asColorLabel(colorDraft) : asColorLabel(asset.colorLabel);
  const [colorOpen, setColorOpen] = useState(false);
  useEffect(() => {
    setColorOpen(false);
  }, [asset.id]);
  async function applyColorLabel(label: string | null): Promise<void> {
    setColorOpen(false);
    setColorDraft(label);
    await assetLabelSet([asset.id], label);
    onAssetPatched?.(asset.id, { colorLabel: label });
  }

  // --- 拒绝旗标（B1）：与星级分层的独立标记；X 键切换 ---------------------------------
  const [rejectDraft, setRejectDraft] = useState<boolean | null>(null);
  useEffect(() => {
    setRejectDraft(null);
  }, [asset.id]);
  const currentRejected = rejectDraft ?? asset.rejected ?? false;
  async function applyRejected(rejected: boolean): Promise<void> {
    setRejectDraft(rejected);
    await assetRejectSet([asset.id], rejected);
    onAssetPatched?.(asset.id, { rejected });
  }

  // --- 版本关系（B2）：RAW/JPG/成片 chips（点击换 asset；孤片不渲染） -------------
  const [versions, setVersions] = useState<AssetVersions | null>(null);
  useEffect(() => {
    let cancelled = false;
    setVersions(null);
    void assetVersions(asset.id).then((v) => {
      if (!cancelled) setVersions(v);
    });
    return () => {
      cancelled = true;
    };
  }, [asset.id]);
  /** 组员 >1 才成「版本」（孤片 members 只有自己，不渲染版本区） */
  const versionMembers: VersionMember[] | null =
    versions !== null && versions.members.length > 1 ? versions.members : null;

  // --- 编辑器（阶段 D）：配方状态（角标/详情行）+ 全屏编辑浮层 --------------------------
  const [editorOpen, setEditorOpen] = useState(false);
  const [editRecipe, setEditRecipe] = useState<EditRecipe | null>(null);
  useEffect(() => {
    let cancelled = false;
    setEditRecipe(null);
    void editRecipeGet(asset.id).then((state) => {
      if (!cancelled) setEditRecipe(state.recipe);
    });
    return () => {
      cancelled = true;
    };
  }, [asset.id]);

  // LR 风格查看器快捷键：方向键导航、[]/,./R 旋转、0-5 评分、P/U 旗标、
  // Z 在适应窗口与 2 倍之间切换、I 开关信息抽屉。输入控件内不截获按键；
  // 右键菜单打开时 Esc 让给菜单（不关查看器）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "F11") {
        e.preventDefault();
        void toggleFullscreen();
        return;
      }
      const target = e.target;
      if (
        target instanceof HTMLElement &&
        (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable)
      ) {
        return;
      }
      // 编辑浮层打开时，查看器快捷键全部让位（编辑器有自己的键位处理）
      if (editorOpen) return;
      if (e.key === "Escape") {
        if (ctxAt !== null) return; // 菜单自身的 Esc 监听负责关闭
        e.preventDefault();
        closeViewer();
      } else if (e.key === "ArrowLeft" && index > 0) {
        onNavigate(index - 1);
      } else if (e.key === "ArrowRight" && index < group.assets.length - 1) {
        onNavigate(index + 1);
      } else if ([".", "]", "r", "R"].includes(e.key)) {
        rotate(90);
      } else if ([",", "["].includes(e.key)) {
        rotate(-90);
      } else if (/^[0-5]$/.test(e.key)) {
        e.preventDefault();
        void applyRating(Number(e.key), false);
      } else if (e.key === "p" || e.key === "P") {
        void applyFlagged(true);
      } else if (e.key === "u" || e.key === "U") {
        void applyFlagged(false);
      } else if (e.key === "x" || e.key === "X") {
        // X = 拒绝旗标切换（LR 同款语义；与星级分层）
        e.preventDefault();
        void applyRejected(!currentRejected);
      } else if (e.key === "z" || e.key === "Z") {
        toggleZoom();
      } else if (e.key === "i" || e.key === "I") {
        setExifOpen((open) => !open);
      } else if (e.key === "?") {
        showHint();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [asset.id, ctxAt, currentRating, currentRejected, editorOpen, group.assets.length, index, closeViewer, onNavigate, showHint, toggleFullscreen]);

  const exifSections = useMemo<ExifSection[]>(() => {
    const d = visibleDetail;
    const sections: ExifSection[] = [];

    // 【文件】核心行恒在（缺值「—」）；格式行仅在有值时渲染
    sections.push({
      key: "file",
      rows: [
        { label: t("viewer.filename"), value: formatValue(d.filename) },
        ...(d.format ? [{ label: t("viewer.format"), value: d.format }] : []),
        { label: t("viewer.size"), value: formatBytes(d.size) },
        { label: t("viewer.importedAt"), value: isoLabel(d.createdAt) },
        { label: t("viewer.capturedAt"), value: isoLabel(d.capturedAt) },
        {
          label: t("viewer.dupCount"),
          value: (
            <span key="dup" className={d.dupCount > 0 ? "font-medium text-accent" : undefined}>
              {t("viewer.dupItems", { count: d.dupCount })}
            </span>
          ),
        },
        // 编辑配方（阶段 D）：已保存/无；点击进入编辑器
        {
          label: t("viewer.editRecipe"),
          value: (
            <button
              key="edit-recipe"
              type="button"
              onClick={() => setEditorOpen(true)}
              className={editRecipe !== null ? "font-medium text-accent hover:brightness-110" : "text-text-secondary hover:text-accent"}
              data-testid="viewer-recipe-state"
              data-saved={editRecipe !== null}
            >
              {editRecipe !== null ? t("viewer.editRecipeSaved") : t("viewer.editRecipeNone")}
            </button>
          ),
        },
      ],
    });

    // 【图像】任一字段存在才成组（后端未返回 EXIF 扩展时整组隐藏）
    const imageRows: ExifRow[] = [];
    if (d.width != null && d.height != null) {
      imageRows.push({ label: t("viewer.dimensions"), value: `${d.width} × ${d.height}` });
    }
    if (d.megapixels != null) {
      imageRows.push({ label: t("viewer.megapixels"), value: `${d.megapixels} MP` });
    }
    if (d.aspect) imageRows.push({ label: t("viewer.aspect"), value: d.aspect });
    if (d.orientation != null && d.orientation >= 1 && d.orientation <= 8) {
      imageRows.push({
        label: t("viewer.orientation"),
        value: isPortraitOrientation(d.orientation)
          ? t("viewer.orientation.portrait")
          : t("viewer.orientation.landscape"),
      });
    }
    // AI 选片行（C 阶段）：闭眼三态（有人闭眼/可能闭眼/未检出人脸——无人脸≠没闭眼，
    // 文案区分）+ 清晰度分（软片标黄）。标注「AI 建议」弱样式，与用户打的星/色分层。
    const ai = d.aiAnalysis;
    const eyesState =
      ai === null || ai === undefined
        ? "not_analyzed"
        : ai.eyes === undefined
          ? "no_face"
          : ai.eyes.value;
    const eyesText =
      eyesState === "closed"
        ? t("viewer.ai.eyes.closed")
        : eyesState === "maybe"
          ? t("viewer.ai.eyes.maybe")
          : eyesState === "no_face"
            ? t("viewer.ai.eyes.noFace")
            : eyesState === "not_analyzed"
              ? t("viewer.ai.eyes.notAnalyzed")
              : eyesState;
    const blur = ai?.blur;
    imageRows.push({
      label: t("viewer.ai.eyesLabel"),
      value: (
        <span key="ai-eyes" className="flex w-full min-w-0 items-center justify-end gap-1.5" data-testid="viewer-ai-eyes" data-state={eyesState}>
          <span title={eyesText} className={`min-w-0 truncate ${eyesState === "closed" ? "text-red-400" : ""}`}>{eyesText}</span>
          <span className="shrink-0 whitespace-nowrap rounded bg-panel px-1 py-0.5 text-[9px] font-normal leading-none text-text-muted" data-testid="viewer-ai-badge">
            {t("viewer.ai.badge")}
          </span>
        </span>
      ),
    });
    imageRows.push({
      label: t("viewer.ai.blurLabel"),
      value: (
        <span key="ai-blur" className="flex w-full min-w-0 items-center justify-end gap-1.5" data-testid="viewer-ai-blur" data-soft={blur?.value === "soft" ? "true" : "false"}>
          {blur === undefined ? (
            <span className="min-w-0 truncate" title={t("viewer.ai.eyes.notAnalyzed")}>{t("viewer.ai.eyes.notAnalyzed")}</span>
          ) : (
            <>
              <span className={`tabular-nums ${blur.value === "soft" ? "text-amber-300" : undefined}`}>{Math.round(blur.score)}</span>
              <span className="inline-flex h-1.5 w-16 shrink-0 overflow-hidden rounded-full bg-panel align-middle">
                <span
                  className={`h-full rounded-full ${blur.value === "soft" ? "bg-amber-400" : "bg-sky-400"}`}
                  style={{ width: `${Math.max(0, Math.min(100, blur.score))}%` }}
                />
              </span>
            </>
          )}
          <span className="shrink-0 whitespace-nowrap rounded bg-panel px-1 py-0.5 text-[9px] font-normal leading-none text-text-muted" data-testid="viewer-ai-badge">
            {t("viewer.ai.badge")}
          </span>
        </span>
      ),
    });

    if (imageRows.length > 0) sections.push({ key: "image", rows: imageRows });

    // 【拍摄】相机/镜头核心行恒在（缺值「—」）；其余字段有值才渲染
    const shotRows: ExifRow[] = [
      { label: t("viewer.camera"), value: formatValue(d.camera) },
      { label: t("viewer.lens"), value: formatValue(d.lens) },
    ];
    if (d.focalLength != null) {
      shotRows.push({ label: t("viewer.focalLength"), value: `${d.focalLength}mm` });
    }
    if (d.aperture != null) {
      shotRows.push({ label: t("viewer.aperture"), value: `f/${d.aperture}` });
    }
    if (d.shutter) shotRows.push({ label: t("viewer.shutter"), value: formatShutter(d.shutter) });
    if (d.iso != null) shotRows.push({ label: t("viewer.iso"), value: String(d.iso) });
    if (d.flash) shotRows.push({ label: t("viewer.flash"), value: formatToken(d.flash, FLASH_ZH, t) });
    if (d.meteringMode) {
      shotRows.push({ label: t("viewer.meteringMode"), value: formatToken(d.meteringMode, METERING_ZH, t) });
    }
    if (d.whiteBalance) {
      shotRows.push({ label: t("viewer.whiteBalance"), value: formatToken(d.whiteBalance, WB_ZH, t) });
    }
    if (d.exposureProgram) {
      shotRows.push({ label: t("viewer.exposureProgram"), value: formatToken(d.exposureProgram, PROGRAM_ZH, t) });
    }
    if (d.software) shotRows.push({ label: t("viewer.software"), value: d.software });
    if (d.artist) shotRows.push({ label: t("viewer.artist"), value: d.artist });
    sections.push({ key: "camera", rows: shotRows });

    // 【位置】仅有 GPS 坐标时成组（缺单个坐标的行显示「—」）
    if (d.gpsLat != null || d.gpsLon != null) {
      sections.push({
        key: "location",
        rows: [
          { label: t("viewer.gpsLat"), value: d.gpsLat != null ? String(d.gpsLat) : "—" },
          { label: t("viewer.gpsLon"), value: d.gpsLon != null ? String(d.gpsLon) : "—" },
        ],
      });
    }
    return sections;
  }, [visibleDetail, editRecipe, t]);

  const hasPrev = index > 0;
  const hasNext = index < group.assets.length - 1;
  // 固定 17 个槽位，当前照片始终居中。边界处留空，不让缩略图条逐张伸缩。

  return (
    <motion.div
      className="fixed inset-0 z-50 flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={asset.name}
      data-testid="viewer"
      initial={motionInitial(motionOn, { opacity: 0 })}
      animate={closing ? { opacity: 0 } : { opacity: 1 }}
      transition={TRANS.quick}
    >
      {/* 顶栏：文件名（分组一） + 计数（分组二，间隔 16px） + 旋转/EXIF/关闭。
          查看器全屏覆盖了主壳标题栏，顶栏背景层带拖拽区让窗口仍可拖动
          （按钮/文件名 pointer-events 正常，仅空白处落到拖拽层）。 */}
      <div className="relative flex h-12 shrink-0 items-center gap-3 px-4 text-text-primary">
        <div className="absolute inset-0" data-tauri-drag-region />
        <div className="pointer-events-none relative flex min-w-0 items-baseline gap-4">
          <span className="truncate text-sm font-semibold" title={asset.name} data-testid="viewer-name">
            {asset.name}
          </span>
          <span
            className="shrink-0 font-mono text-xs text-text-muted"
            data-testid="viewer-index"
          >
            {t("viewer.index", { index: index + 1, total: group.assets.length })}
          </span>
        </div>
        <div className="pointer-events-none relative ml-auto flex items-center gap-2">
          {/* 编辑（阶段 D）：打开非破坏编辑浮层；已保存配方时按钮高亮 + 「已编辑」角标 */}
          <button
            type="button"
            onClick={() => setEditorOpen(true)}
            title={editRecipe !== null ? t("viewer.editEdited") : t("viewer.edit")}
            className={`pointer-events-auto relative rounded-md border px-2.5 py-1 text-xs transition-colors ${
              editRecipe !== null
                ? "border-accent/70 bg-accent/10 text-accent"
                : "border-edge text-text-secondary hover:border-accent hover:text-accent"
            }`}
            data-testid="viewer-edit"
            data-edited={editRecipe !== null}
          >
            {t("viewer.edit")}
            {editRecipe !== null && (
              <span
                className="absolute -right-1.5 -top-1.5 rounded-full bg-accent px-1 text-[9px] font-medium leading-[14px] text-black"
                data-testid="viewer-edit-badge"
              >
                {t("viewer.editBadge")}
              </span>
            )}
          </button>
          {/* 旋转：90° 步进（逆/顺时针），150ms 过渡；随资产切换重置 */}
          <button
            type="button"
            onClick={() => rotate(-90)}
            aria-label={t("viewer.rotateCcw")}
            title={t("viewer.rotateCcw")}
            className="pointer-events-auto rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-rotate-ccw"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M3.5 6.5a5 5 0 1 1 1.2 5.4" />
              <path d="M3.2 3.2v3.3h3.3" />
            </svg>
          </button>
          <button
            type="button"
            onClick={() => rotate(90)}
            aria-label={t("viewer.rotateCw")}
            title={t("viewer.rotateCw")}
            className="pointer-events-auto rounded-md border border-edge px-2.5 py-1 text-xs text-text-secondary transition-colors hover:border-accent hover:text-accent"
            data-testid="viewer-rotate-cw"
          >
            <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M12.5 6.5a5 5 0 1 0-1.2 5.4" />
              <path d="M12.8 3.2v3.3H9.5" />
            </svg>
          </button>
        </div>

        </div>

      <div className="flex min-h-0 flex-1">
        {/* 左栏整体随详情抽屉伸缩：主图与底部胶片条始终保持同一宽度。 */}
        <div className="flex min-w-0 flex-1 flex-col" data-testid="viewer-left-pane">
        {/* 主区：大图 + 左右切换 */}
        <div className="relative min-h-0 flex-1" data-testid="viewer-preview-pane">
          <div
            ref={stageRef}
            className="absolute inset-0 flex touch-none items-center justify-center overflow-hidden"
            onDoubleClick={resetView}
            onPointerDown={handlePointerDown}
            onPointerMove={handlePointerMove}
            onPointerUp={handlePointerUp}
            onPointerCancel={handlePointerUp}
            onContextMenu={(e) => {
              // 大图右键：自定义资产菜单（原生菜单已被全局 guard 屏蔽，此处兜底）
              e.preventDefault();
              setCtxAt({ x: e.clientX, y: e.clientY });
            }}
            data-testid="viewer-stage"
            data-scale={view.scale.toFixed(2)}
            data-rotation={view.rotation}
            style={{ cursor: view.scale > 1 ? "grab" : "default" }}
          >
            {/* 全屏与退出入口固定在预览区右上角。 */}
            <div className="pointer-events-auto absolute right-3 top-3 z-20 flex gap-2">
            <button type="button" onPointerDown={(event) => event.stopPropagation()} onClick={() => setExifOpen((open) => !open)} aria-label={exifOpen ? t("viewer.detailsHide") : t("viewer.detailsShow")} title={exifOpen ? t("viewer.detailsHide") : t("viewer.detailsShow")} aria-pressed={exifOpen} className="pointer-events-auto flex h-11 w-11 items-center justify-center rounded-full border border-white/15 bg-black/45 text-white/80 shadow-md backdrop-blur-sm transition-colors hover:bg-black/70 hover:text-white" data-testid="viewer-exif-toggle">
              <svg viewBox="0 0 20 20" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true"><circle cx="10" cy="10" r="7.3" /><path d="M10 9v5" strokeLinecap="round" /><circle cx="10" cy="6" r=".9" fill="currentColor" stroke="none" /></svg>
            </button>
            <button type="button" onPointerDown={(event) => event.stopPropagation()} onClick={() => void toggleFullscreen()} aria-label={fullscreen ? t("viewer.leaveFullscreen") : t("viewer.fullscreen")} title={fullscreen ? t("viewer.leaveFullscreen") : t("viewer.fullscreen")} className="flex h-11 w-11 items-center justify-center rounded-full border border-white/15 bg-black/45 text-white/80 shadow-md backdrop-blur-sm transition-colors hover:bg-black/70 hover:text-white" data-testid="viewer-fullscreen">
              <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={fullscreen ? "M2 6h4V2M10 2v4h4M14 10h-4v4M6 14v-4H2" : "M6 2H2v4M10 2h4v4M14 10v4h-4M2 10v4h4"} /></svg>
            </button>
            <button
              type="button"
              onPointerDown={(event) => event.stopPropagation()}
              onClick={closeViewer}
              aria-label={t("viewer.exit")}
              title={t("viewer.exit")}
              className="pointer-events-auto flex h-11 w-11 items-center justify-center rounded-full border border-white/15 bg-black/45 text-white/80 shadow-md backdrop-blur-sm transition-colors hover:bg-black/70 hover:text-white"
              data-testid="viewer-close"
            >
              <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" aria-hidden="true">
                <path d="M4 4l8 8M12 4l-8 8" />
              </svg>
            </button>
            </div>
            {/* 源缺失横幅（琥珀警示条）：不阻塞关闭/翻图（pointer-events-none） */}
            {mainMissing && (
              <div
                className="pointer-events-none absolute left-1/2 top-3 z-20 flex -translate-x-1/2 items-center gap-1.5 rounded-md border border-amber-400/50 bg-amber-500/15 px-3 py-1.5 text-xs text-amber-300"
                role="status"
                data-testid="viewer-missing-banner"
              >
                <svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M8 5.5v3.5" />
                  <circle cx="8" cy="11.6" r=".9" fill="currentColor" stroke="none" />
                  <path d="M8 1.8L15 14H1z" />
                </svg>
                {t("viewer.missingSource")}
              </div>
            )}
            <motion.div
              key={asset.id}
              className="absolute inset-0 flex items-center justify-center"
              initial={motionInitial(motionOn, { opacity: 0, x: navDirRef.current * 36 })}
              animate={{ opacity: 1, x: 0 }}
              transition={TRANS.quick}
            >
            {imageLayers.map((layer) => (
              <img
                key={layer.src}
                src={layer.src}
                alt={layer.phase === "active" ? asset.name : ""}
                aria-hidden={layer.phase === "active" ? undefined : "true"}
                draggable={false}
                loading="eager"
                decoding="async"
                onLoad={
                  layer.phase === "loading"
                    ? async (event) => {
                        const image = event.currentTarget;
                        // onLoad 只代表资源到达；WebView 的解码/纹理上传可能尚未完成。等待
                        // decode() 和下一动画帧后再接管，避免先撤旧图再露出黑底。
                        try {
                          if (typeof image.decode === "function") await image.decode();
                        } catch {
                          // 部分编码器会在可正常显示时仍拒绝 decode；继续交给下一帧绘制。
                        }
                        await new Promise<void>((resolve) => requestAnimationFrame(() => resolve()));
                        // 只提交当前请求；快速连切时迟到的旧 onLoad 不能覆盖新图。
                        if (layer.src !== mainSrc) return;
                        setImageLayers((previous) => {
                          const target = previous.find((item) => item.src === layer.src);
                          if (!target) return previous;
                          return previous.map((item) =>
                            item.src === layer.src
                              ? { ...item, phase: "active" }
                              : { ...item, phase: "retiring" },
                          );
                        });
                      }
                    : undefined
                }
                onError={
                  layer.phase === "loading"
                    ? () => {
                        // 分级降档：原图失败 → 中间档（2048）→ 512 档。
                        // TIF 等大尺寸/特殊编码原图 WebView2 渲染不动，逐级降而不是一步到 512。
                        if (asset.kind === "photo" && stage === "original") setStage("mid");
                        else if (asset.kind === "photo" && stage === "mid") setStage("thumb");
                      }
                    : undefined
                }
                data-testid={
                  layer.phase === "active"
                    ? "viewer-img-prev"
                    : layer.phase === "loading"
                      ? "viewer-img"
                      : "viewer-img-retiring"
                }
                data-fallback={sourceKind(layer.src)}
                className={`${
                  layer.phase === "loading" ? "invisible " : "absolute "
                }max-h-full max-w-full select-none object-contain will-change-transform [backface-visibility:hidden]`}
                style={{
                  // 变换顺序 translate→rotate→scale（origin=center）：图片自身中心先随平移
                  // 移动，旋转恒绕图片当前视觉中心（Windows 照片同款，平移后旋转不绕错轴）
                  transform: `translate(${view.x}px, ${view.y}px) rotate(${view.rotation}deg) scale(${view.scale})`,
                  // 拖拽跟手直通；滚轮/双击/旋转目标值变化走过渡（缩放流畅）
                  transition: dragging ? "none" : "transform 150ms cubic-bezier(0.2, 0, 0, 1)",
                }}
              />
            ))}
            </motion.div>
            {imageLayers.length === 0 && mainFailed && (
              <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="viewer-placeholder">
                <svg
                  viewBox="0 0 24 24"
                  width="56"
                  height="56"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className="text-sky-400"
                  aria-hidden="true"
                >
                  <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                  <circle cx="9" cy="10" r="1.8" />
                  <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                </svg>
                <span className="text-xs">{t("viewer.noPreview")}</span>
              </div>
            )}
            {/* 缺源且无历史缓存：居中图标+文案占位（绝不留空白舞台） */}
            {showMissingPlaceholder && (
              <div className="flex flex-col items-center gap-2 text-text-muted" data-testid="viewer-missing-placeholder">
                <svg
                  viewBox="0 0 24 24"
                  width="56"
                  height="56"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="1.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                  className="text-amber-400"
                  aria-hidden="true"
                >
                  <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                  <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                  <path d="M14.5 5.5l4 4M18.5 5.5l-4 4" />
                </svg>
                <span className="text-xs text-amber-300">{t("viewer.missingSource")}</span>
              </div>
            )}
            {slowLoading && (
              // 大图/新图在途超过 300ms：中央小 spinner（避免黑屏被误判为失败）
              <div
                className="absolute h-10 w-10 animate-spin rounded-full border-2 border-edge border-t-accent"
                data-testid="viewer-loading"
              />
            )}

            {/* 左右切换 */}
            <button
              type="button"
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => onNavigate(index - 1)}
              disabled={!hasPrev}
              aria-label={t("viewer.prev")}
              className="pointer-events-auto absolute left-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
              data-testid="viewer-prev"
            >
              <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M10 3L5 8l5 5" />
              </svg>
            </button>
            <button
              type="button"
              onPointerDown={(event) => event.stopPropagation()}
              onClick={() => onNavigate(index + 1)}
              disabled={!hasNext}
              aria-label={t("viewer.next")}
              className="pointer-events-auto absolute right-3 top-1/2 z-10 flex h-11 w-11 -translate-y-1/2 items-center justify-center rounded-full border border-edge bg-black/50 text-text-primary transition-colors hover:border-accent hover:text-accent disabled:cursor-not-allowed disabled:opacity-30"
              data-testid="viewer-next"
            >
              <svg viewBox="0 0 16 16" width="18" height="18" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                <path d="M6 3l5 5-5 5" />
              </svg>
            </button>
          </div>
          {/* 操作提示：CSS 过渡淡入淡出（确定性，不走动画帧） */}
          <p
            onMouseEnter={showHint}
            className={`absolute bottom-1.5 left-1/2 -translate-x-1/2 rounded bg-black/40 px-2 py-0.5 font-mono text-[10px] text-text-muted/80 transition-opacity duration-300 ${
              hintVisible ? "opacity-100" : "pointer-events-none opacity-0"
            }`}
            data-testid="viewer-hint"
          >
            {t("viewer.zoomHint")}
          </p>
        </div>

        <div className="flex h-[70px] shrink-0 items-center justify-center gap-1 overflow-hidden border-t border-edge px-2" data-testid="viewer-filmstrip">
          {Array.from({ length: 17 }, (_, slot) => {
            const itemIndex = index + slot - 8;
            const item = group.assets[itemIndex];
            if (!item) return <span key={`empty-${slot}`} className="h-14 min-w-0 max-w-16 flex-1" data-testid="viewer-filmstrip-slot" aria-hidden="true" />;
            const current = itemIndex === index;
            return <span key={`asset-${item.id}`} className="h-14 min-w-0 max-w-16 flex-1" data-testid="viewer-filmstrip-slot">
              <button type="button" onClick={() => onNavigate(itemIndex)} aria-label={item.name} aria-current={current} className={`flex h-full w-full overflow-hidden rounded-md border-2 transition-colors ${current ? "border-accent" : "border-transparent hover:border-edge"}`} data-testid="viewer-filmthumb" data-asset-id={item.id} data-current={current}>
                <AssetThumb asset={item} size={FILM_THUMB_SIZE} className="h-full w-full" skeleton={false} miniLabel={String(itemIndex + 1)} testId="viewer-filmthumb-cell" />
              </button>
            </span>;
          })}
        </div>

        </div>

        {/* 详情关闭时完全收回，预览获得全部可用宽度。 */}
            <AnimatePresence initial={false}>
            {exifOpen && (
            <motion.aside
              className="h-full w-72 shrink-0 overflow-hidden border-l border-edge bg-surface"
              data-testid="viewer-exif"
              data-open="true"
              initial={motionInitial(motionOn, { x: 32, opacity: 0 })}
              animate={{ x: 0, opacity: 1 }}
              exit={{ x: 32, opacity: 0 }}
              transition={TRANS.slide}
            >
              <div className="h-full overflow-y-auto p-3" data-testid="viewer-exif-scroll">
                <div className="mb-1 flex items-center gap-2">
                  <h2 className="text-xs font-semibold text-text-primary">{t("viewer.exif")}</h2>
                </div>
                <>
                {/* 星标条：0-5 星点选（再点同星=清除） */}
                <div
                  className="mb-2 flex items-center gap-1 border-b border-edge/60 pb-2"
                  role="radiogroup"
                  aria-label={t("viewer.rating")}
                  data-testid="viewer-rating"
                >
                  {[1, 2, 3, 4, 5].map((value) => (
                    <button
                      key={value}
                      type="button"
                      role="radio"
                      aria-checked={currentRating === value}
                      aria-label={`${t("viewer.rating")} ${value}`}
                      onClick={() => void applyRating(value)}
                      className={`rounded p-0.5 transition-all duration-150 active:scale-125 ${
                        value <= currentRating ? "text-accent" : "text-text-muted hover:text-text-secondary"
                      }`}
                      data-testid="viewer-rating-star"
                      data-value={value}
                      data-filled={value <= currentRating}
                    >
                      <svg viewBox="0 0 16 16" width="15" height="15" fill={value <= currentRating ? "currentColor" : "none"} stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" aria-hidden="true">
                        <path d="M8 1.8l1.8 3.7 4 .6-2.9 2.8.7 4L8 11l-3.6 1.9.7-4L2.2 6.1l4-.6z" />
                      </svg>
                    </button>
                  ))}
                  {/* 颜色标签（LR 五色标）：当前色点点开设置（单资产） */}
                  <div className="relative">
                    <button
                      type="button"
                      onClick={() => setColorOpen((v) => !v)}
                      aria-expanded={colorOpen}
                      aria-label={t("viewer.colorLabel")}
                      title={currentColor === null ? t("viewer.colorLabel") : t(`gallery.color.${currentColor}`)}
                      className="rounded-full p-1 transition-all duration-150 hover:bg-panel active:scale-125"
                      data-testid="viewer-color"
                      data-label={currentColor ?? "none"}
                    >
                      {currentColor === null ? (
                        <span className={`block h-3 w-3 rounded-full border border-dashed border-text-muted`} aria-hidden="true" />
                      ) : (
                        <span className={`block h-3 w-3 rounded-full ${COLOR_DOT_CLASS[currentColor as ColorLabel]} ${COLOR_DOT_RING}`} aria-hidden="true" />
                      )}
                    </button>
                    {colorOpen && (
                      <div
                        className="absolute left-0 top-7 z-10 flex w-max items-center gap-1 rounded-lg border border-edge bg-surface p-1.5 shadow-xl"
                        data-testid="viewer-color-menu"
                      >
                        {COLOR_LABELS.map((label) => (
                          <button
                            key={label}
                            type="button"
                            title={t(`gallery.color.${label}`)}
                            aria-label={t(`gallery.color.${label}`)}
                            onClick={() => void applyColorLabel(label)}
                            className="rounded-full p-1 transition-transform hover:scale-110"
                            data-testid="viewer-color-option"
                            data-label={label}
                          >
                            <span className={`block h-3.5 w-3.5 rounded-full ${COLOR_DOT_CLASS[label]} ${COLOR_DOT_RING}`} aria-hidden="true" />
                          </button>
                        ))}
                        <span className="h-4 w-px bg-edge" aria-hidden="true" />
                        <button
                          type="button"
                          onClick={() => void applyColorLabel(null)}
                          title={t("selection.colorClear")}
                          aria-label={t("selection.colorClear")}
                          className="rounded-full p-1 text-text-muted transition-colors hover:bg-panel hover:text-text-primary"
                          data-testid="viewer-color-clear"
                        >
                          <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden="true">
                            <path d="M4 4l8 8M12 4l-8 8" />
                          </svg>
                        </button>
                      </div>
                    )}
                  </div>
                  <button
                    type="button"
                    onClick={() => void applyFlagged(!currentFlagged)}
                    aria-pressed={currentFlagged}
                    title={currentFlagged ? t("viewer.flagClear") : t("viewer.flagSet")}
                    className={`ml-auto rounded px-1.5 py-0.5 text-[11px] transition-transform duration-150 active:scale-125 ${
                      currentFlagged ? "bg-accent text-black" : "text-text-muted hover:text-text-primary"
                    }`}
                    data-testid="viewer-flag"
                  >
                    ⚑
                  </button>
                  {/* 拒绝旗标（与星级分层；X 键切换） */}
                  <button
                    type="button"
                    onClick={() => void applyRejected(!currentRejected)}
                    aria-pressed={currentRejected}
                    title={currentRejected ? t("viewer.rejectClear") : t("viewer.rejectSet")}
                    className={`rounded px-1.5 py-0.5 text-[11px] transition-colors ${
                      currentRejected
                        ? "bg-red-500 text-white"
                        : "text-text-muted hover:bg-red-400/10 hover:text-red-400"
                    }`}
                    data-testid="viewer-reject"
                  >
                    <svg viewBox="0 0 16 16" width="11" height="11" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="inline-block align-[-1px]">
                      <circle cx="8" cy="8" r="5.6" />
                      <path d="M4.2 11.8l7.6-7.6" />
                    </svg>
                  </button>
                </div>
                {/* 版本区（B2）：RAW/JPG/成片成员 chips；当前项高亮；孤片不渲染 */}
                {versionMembers !== null && (
                  <div className="mb-2 border-b border-edge/60 pb-2" data-testid="viewer-versions">
                    <h3 className="mb-1.5 text-[10px] font-semibold uppercase tracking-wider text-text-muted">
                      {t("viewer.versions")}
                    </h3>
                    <div className="flex flex-wrap gap-1" role="listbox" aria-label={t("viewer.versions")}>
                      {versionMembers.map((member) => {
                        const current = member.assetId === asset.id;
                        const derived = member.role === "derived";
                        const roleLabel =
                          member.role === null ? null : t(VERSION_ROLE_KEYS[member.role]);
                        return (
                          <button
                            key={member.assetId}
                            type="button"
                            role="option"
                            aria-selected={current}
                            onClick={() => onVersionSelect?.(member.assetId)}
                            disabled={onVersionSelect === undefined}
                            className={`flex max-w-full items-center gap-1.5 rounded-md border px-1.5 py-1 text-left transition-colors disabled:cursor-default ${
                              current
                                ? "border-accent bg-accent/10 text-accent"
                                : "border-edge text-text-secondary hover:border-text-muted hover:text-text-primary"
                            } disabled:opacity-70`}
                            data-testid="viewer-version-chip"
                            data-role={member.role ?? "none"}
                            data-asset-id={member.assetId}
                            data-current={current}
                          >
                            {roleLabel !== null && (
                              <span
                                className={`shrink-0 font-mono text-[10px] font-bold leading-4 ${
                                  member.role === "raw"
                                    ? "text-sky-400"
                                    : derived
                                      ? "text-violet-300"
                                      : "text-text-muted"
                                }`}
                              >
                                {roleLabel}
                              </span>
                            )}
                            <span className="max-w-[110px] truncate text-[10px]" title={member.name}>
                              {member.name}
                            </span>
                            {derived && (
                              <span
                                className="shrink-0 rounded bg-violet-400/15 px-1 text-[9px] leading-4 text-violet-300"
                                data-testid="viewer-version-derived-badge"
                              >
                                {t("viewer.version.derived")}
                              </span>
                            )}
                          </button>
                        );
                      })}
                    </div>
                  </div>
                )}
                <div data-testid="viewer-exif-rows" data-asset-id={visibleDetail.id}>
                  {exifSections.map((section) => (
                    <section key={section.key} data-testid={`viewer-exif-group-${section.key}`}>
                      <h3 className="mb-2 mt-4 border-l-2 border-accent pl-2 text-[13px] font-bold tracking-wide text-text-primary first:mt-1">
                        {t(`viewer.group.${section.key}`)}
                      </h3>
                      <dl className="space-y-1.5">
                        {section.rows.map((row) => (
                          <div key={row.label} className="flex items-baseline justify-between gap-2 text-xs">
                            <dt className="shrink-0 text-text-muted">{row.label}</dt>
                            <dd className="min-w-0 flex-1 text-right text-text-secondary">{row.value}</dd>
                          </div>
                        ))}
                      </dl>
                    </section>
                  ))}
                </div>
                </>
              </div>
            </motion.aside>
            )}
            </AnimatePresence>
      </div>

      {/* 大图右键菜单（作用于当前资产；Esc/点击外部关闭，期间查看器 Esc 让位） */}
      {ctxAt !== null && (
        <AssetContextMenu
          at={ctxAt}
          assets={[asset]}
          onClose={() => setCtxAt(null)}
          onColorLabeled={(_, label) => onAssetPatched?.(asset.id, { colorLabel: label })}
          onRejected={(_, rejected) => onAssetPatched?.(asset.id, { rejected })}
        />
      )}

      {/* 非破坏编辑浮层（阶段 D）：保存/重置后回写角标与详情行状态 */}
      {editorOpen && (
        <EditorOverlay
          asset={asset}
          initial={{ recipe: editRecipe, updatedAt: null }}
          onClose={() => setEditorOpen(false)}
          onSaved={(recipe) => setEditRecipe(recipe)}
          onMetadataSaved={(metadata) => {
            onAssetPatched?.(asset.id, { capturedAt: metadata.capturedAt, camera: metadata.camera || null });
            void assetDetail(asset.id).then((value) => { if (value) setDetail(value); });
          }}
        />
      )}
    </motion.div>
  );
}
