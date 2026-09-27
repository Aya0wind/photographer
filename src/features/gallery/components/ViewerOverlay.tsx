import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";

import {
  assetDetail,
  assetFlagSet,
  assetRatingSet,
  type AssetDetailDto,
  type AssetDto,
} from "@/ipc/api";
import type { AssetGroup } from "../lib/assetGroups";
import {
  fetchAssetThumb,
  useAssetThumbUrl,
  warmImageDecode,
} from "../lib/thumbPipeline";
import { AssetContextMenu } from "./ContextMenu";
import AssetThumb from "./AssetThumb";
import ViewerVideo from "./ViewerVideo";
import { formatBytes } from "@/lib/format";

/**
 * 全屏沉浸查看器（M3）：
 * - 大图策略按 kind：photo 优先原图 asset 协议（convertFileSrc(path)），加载失败回退
 *   大档缩略图（名义 1280，后端 snap 512）；RAW 无可载原图（inline-JPEG 提取在 M4），
 *   直接用大档缩略图放大显示；视频恒占位（播放是后续里程碑）。
 * - 交互：wheel 以指针为锚缩放 1x-4x（原生非 passive 监听），scale>1 可拖拽平移，
 *   90° 步进旋转（按钮 / 键盘 . , R，transform 顺序 rotate→scale→translate，
 *   150ms 过渡），双击复位（含旋转与平移）；←/→ 同组切换（首尾禁用）；
 *   Esc 返回画廊（画廊页不卸载，滚动位置保留）。旋转随资产切换重置，不持久化。
 * - 切图在不可见解码层完成后原子替换可见图层，避免新旧图片交叉叠显。
 * - 右侧 EXIF 面板可收起；底部只显示当前照片前后各 8 张缩略图，
 *   点击最多跳 8 张，没有横向滚动；相邻照片预取。
 */

const VIEWER_MID_SIZE = 2048; // 中间档（后端加 2048 档后生效）：原图渲染失败时的清晰回退
/** RAW 内嵌全幅直出档（后端语义：RAW && size > 2048 = 提取最大内嵌 JPEG 原样直出） */
const VIEWER_RAW_EMBED_SIZE = 6000;
const VIEWER_THUMB_SIZE = 1280; // 名义边长；后端 snap 512 档就近
const FILM_THUMB_SIZE = 240;
const MIN_SCALE = 1;
const MAX_SCALE = 4;

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
  };
}

/** 路径 → asset 协议 URL（非 Tauri 环境抛错回退 null） */
function safeConvert(path: string): string | null {
  try {
    return convertFileSrc(path) || null;
  } catch {
    return null;
  }
}

/** ISO 时间 → "YYYY-MM-DD HH:mm:ss"（本地时区）；空值 "—" */
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
  fired: "闪光",
  no_flash: "未闪光",
  no_flash_function_not_fired: "未闪光（无闪光功能）",
  flash_fired_compulsory: "闪光（强制）",
};
const METERING_ZH: Record<string, string> = {
  average: "平均测光",
  center_weighted_average: "中央重点",
  pattern: "矩阵测光",
  spot: "点测光",
  multi_spot: "多点测光",
  partial: "局部测光",
};
const WB_ZH: Record<string, string> = { auto: "自动", manual: "手动" };
const PROGRAM_ZH: Record<string, string> = {
  manual: "手动 (M)",
  program_auto: "程序自动 (P)",
  aperture_priority: "光圈优先 (A)",
  shutter_priority: "快门优先 (S)",
  creative: "创意程序",
  action: "动作程序",
  portrait_mode: "人像",
  landscape_mode: "风景",
  bulb: "B 门",
};

/** 枚举字段展示：token 查表，非 token（自由文本）原样，空 → "—" */
function formatToken(value: string | null | undefined, table: Record<string, string>): string {
  if (value === null || value === undefined || value === "") return "—";
  return table[value] ?? value;
}

/** EXIF orientation（1-8）→ 拍摄方向：5-8 为竖拍（含镜像竖拍），1-4 为横拍 */
function isPortraitOrientation(orientation: number): boolean {
  return Number.isInteger(orientation) && orientation >= 5 && orientation <= 8;
}

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
}

export default function ViewerOverlay({ asset, group, index, onNavigate, onClose }: ViewerOverlayProps) {
  const { t } = useTranslation();
  const stageRef = useRef<HTMLDivElement | null>(null);
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
  const closeViewer = useCallback(() => {
    if (fullscreen) {
      if (isTauri()) void getCurrentWindow().setFullscreen(false);
      else if (document.fullscreenElement) void document.exitFullscreen();
    }
    onClose();
  }, [fullscreen, onClose]);
  useEffect(() => {
    const sync = () => setFullscreen(Boolean(document.fullscreenElement));
    document.addEventListener("fullscreenchange", sync);
    return () => document.removeEventListener("fullscreenchange", sync);
  }, []);

  // 大图右键菜单（与瓦片同款：reveal/复制文件/旗标，作用于当前资产）
  const [ctxAt, setCtxAt] = useState<{ x: number; y: number } | null>(null);

  // --- 缩放/平移/旋转状态（资产切换时复位；旋转不持久化） ----------------------------
  const [view, setView] = useState({ scale: 1, x: 0, y: 0, rotation: 0 });
  useEffect(() => {
    setView({ scale: 1, x: 0, y: 0, rotation: 0 });
  }, [asset.id]);

  const clampScale = (s: number) => Math.min(MAX_SCALE, Math.max(MIN_SCALE, s));
  /** 90° 步进旋转（负=逆时针）；触发拖拽/缩放之外的独立维度 */
  const rotate = (delta: number) =>
    // 保留连续角度，确保 -270 -> -360 的过渡继续向左，而不是归零后反向补间。
    setView((v) => ({ ...v, rotation: v.rotation + delta }));

  // 主预览滚轮始终只负责缩放；前后翻页统一由左右箭头/键盘/胶片条承担。
  useEffect(() => {
    const el = stageRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const cx = e.clientX - rect.left - rect.width / 2;
      const cy = e.clientY - rect.top - rect.height / 2;
      setView((v) => {
        const next = clampScale(v.scale * (e.deltaY < 0 ? 1.2 : 1 / 1.2));
        if (next === v.scale) return v;
        if (next === MIN_SCALE) return { scale: MIN_SCALE, x: 0, y: 0, rotation: v.rotation };
        const ratio = next / v.scale;
        // 指针为锚：保持光标下的图像点不动
        return { scale: next, x: cx - (cx - v.x) * ratio, y: cy - (cy - v.y) * ratio, rotation: v.rotation };
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);

  // 拖拽平移（scale>1 时）：pointer capture，jsdom/老 WebView 缺失时静默退化
  const dragRef = useRef<{ x: number; y: number } | null>(null);
  function handlePointerDown(e: React.PointerEvent<HTMLDivElement>): void {
    if (view.scale <= MIN_SCALE) return;
    // 箭头/工具按钮是操作控件，缩放状态下不能被舞台的 pointer capture 抢走点击。
    if (e.target instanceof Element && e.target.closest("button")) return;
    dragRef.current = { x: e.clientX, y: e.clientY };
    try {
      e.currentTarget.setPointerCapture(e.pointerId);
    } catch {
      // 无指针捕获时 move/up 仍在本元素内生效
    }
  }
  function handlePointerMove(e: React.PointerEvent<HTMLDivElement>): void {
    const drag = dragRef.current;
    if (!drag) return;
    setView((v) => ({ ...v, x: v.x + (e.clientX - drag.x), y: v.y + (e.clientY - drag.y) }));
    dragRef.current = { x: e.clientX, y: e.clientY };
  }
  function handlePointerUp(e: React.PointerEvent<HTMLDivElement>): void {
    dragRef.current = null;
    try {
      e.currentTarget.releasePointerCapture(e.pointerId);
    } catch {
      // 静默
    }
  }

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
  const originalUrl = useMemo(
    () => (asset.kind === "photo" ? safeConvert(asset.path) : null),
    [asset.kind, asset.path],
  );
  const [stage, setStage] = useState<"original" | "mid" | "thumb">("original");
  useEffect(() => {
    setStage("original");
  }, [asset.id, originalUrl]);
  const wantsThumb = asset.kind === "photo" || asset.kind === "raw" || asset.kind === "video";
  const thumb = useAssetThumbUrl(asset.id, VIEWER_THUMB_SIZE, wantsThumb, "high");
  // 内嵌全幅直出档（>2048 为后端语义标记）：RAW 主显示路径
  const rawEmbed = useAssetThumbUrl(
    asset.id,
    VIEWER_RAW_EMBED_SIZE,
    asset.kind === "raw",
    "high",
  );
  // rawler 2048 显影：仅在「无内嵌预览」（embed 结算为 null）时才启用兜底
  const rawFull = useAssetThumbUrl(
    asset.id,
    VIEWER_MID_SIZE,
    asset.kind === "raw" && rawEmbed.status === "failed",
    "high",
  );
  // 中间档仅 photo 且原图已失败时才请求（2048 档后端就位后生效；未就位时 settled null → 继续降 512）
  const mid = useAssetThumbUrl(
    asset.id,
    VIEWER_MID_SIZE,
    asset.kind === "photo" && stage !== "original",
    "high",
  );

  let mainSrc: string | null;
  let mainFailed = false;
  if (asset.kind === "video") {
    mainSrc = null; // 视频不走图片层：舞台渲染 <ViewerVideo>（M8 内建播放+HEVC 回退）
  } else if (asset.kind === "photo" && stage === "original" && originalUrl !== null) {
    mainSrc = originalUrl; // img onError → 降档；渲染失败前不作无图判定
  } else if (asset.kind === "photo" && stage === "mid") {
    mainSrc = mid.url; // 在途 null → 走加载提示；确定无图由下方自动降档
  } else if (asset.kind === "raw") {
    // 512 秒出 → 内嵌全幅替换变清晰；无内嵌预览 → 2048 显影兜底
    mainSrc = rawEmbed.url ?? rawFull.url ?? thumb.url;
    mainFailed =
      rawEmbed.status === "failed" &&
      rawFull.status === "failed" &&
      thumb.status === "failed";
  } else {
    mainSrc = thumb.url;
    mainFailed = thumb.status === "failed";
  }
  // 中间档确定无图（后端未加 2048 档 / 提取失败）→ 自动降到 512 档
  useEffect(() => {
    if (asset.kind === "photo" && stage === "mid" && mid.status === "failed") {
      setStage("thumb");
    }
  }, [asset.kind, stage, mid.status]);
  // photo 无原图可用（非 Tauri 环境 convertFileSrc 抛错）→ 直达 512 档
  useEffect(() => {
    if (asset.kind === "photo" && stage === "original" && originalUrl === null) {
      setStage("thumb");
    }
  }, [asset.kind, stage, originalUrl]);

  // --- 无空窗单层切换 ---------------------------------------------------------------
  // 用稳定 key 保留真正已解码的 DOM 图片节点：新图先在不可见层解码，onLoad 后把同一个
  // 节点提升为可见层并移除旧层。不能只把新 src 写回旧 <img>，否则浏览器仍可能在重新
  // 绘制该节点时短暂清空画面，表现为切图黑闪。
  type ImageLayer = { src: string; phase: "active" | "loading" | "retiring" };
  const [imageLayers, setImageLayers] = useState<ImageLayer[]>([]);
  useEffect(() => {
    if (mainSrc === null) return; // 源在途（等 2048/512 URL）——旧图层继续显示
    setImageLayers((previous) => {
      if (previous.some((layer) => layer.src === mainSrc)) return previous;
      const active = previous.find((layer) => layer.phase === "active");
      return active
        ? [active, { src: mainSrc, phase: "loading" }]
        : [{ src: mainSrc, phase: "loading" }];
    });
  }, [mainSrc]);
  const hasRetiringLayer = imageLayers.some((layer) => layer.phase === "retiring");
  useEffect(() => {
    if (!hasRetiringLayer) return;
    // 新图至少完整绘制一帧后才移除旧图。这样即使 WebView 合成线程比 React 提交稍慢，
    // 也始终有上一张作为后备，不会在两个纹理之间露出黑色舞台背景。
    const frame = requestAnimationFrame(() => {
      setImageLayers((previous) =>
        previous.filter((layer) => layer.phase !== "retiring"),
      );
    });
    return () => cancelAnimationFrame(frame);
  }, [hasRetiringLayer]);
  // 确定无图：清掉残留图层，显示占位
  useEffect(() => {
    if (mainFailed) {
      setImageLayers([]);
    }
  }, [mainFailed]);

  // 大图加载提示：源在途超过 300ms 才转圈（几十 MB 原图加载慢，避免黑屏误判失败）；
  // 切换期间旧图层兜底显示，仅新图 300ms 仍未 onLoad 才叠加 spinner（快速连按不闪）。
  const [slowLoading, setSlowLoading] = useState(false);
  const awaitingImage =
    imageLayers.some((layer) => layer.phase === "loading") ||
    (mainSrc === null && !mainFailed);
  useEffect(() => {
    setSlowLoading(false);
    if (!awaitingImage) return;
    const timer = setTimeout(() => setSlowLoading(true), 300);
    return () => clearTimeout(timer);
  }, [awaitingImage]);

  // 相邻预取（#7 预加载强化）：前后各 1 张——
  // a) 512 回退档 URL 高优先预取；b) new Image() 解码预热（photo 的原图 asset URL 也预热），
  // 让箭头切换时下一张大概率已在解码器缓存里。
  useEffect(() => {
    const neighbors = [
      index > 0 ? group.assets[index - 1] : null,
      index < group.assets.length - 1 ? group.assets[index + 1] : null,
    ];
    for (const neighbor of neighbors) {
      if (!neighbor) continue;
      if (neighbor.kind === "photo") warmImageDecode(safeConvert(neighbor.path));
      void fetchAssetThumb(neighbor.id, VIEWER_THUMB_SIZE, "high").then((r) => {
        if (r.kind === "url") warmImageDecode(r.url);
      });
      if (neighbor.kind === "raw") {
        // RAW 相邻预热内嵌全幅直出档（毫秒级 IO，取最大内嵌 JPEG）
        void fetchAssetThumb(neighbor.id, VIEWER_RAW_EMBED_SIZE, "high").then((r) => {
          if (r.kind === "url") warmImageDecode(r.url);
        });
      }
    }
  }, [index, group.assets]);

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
      } else if (e.key === "z" || e.key === "Z") {
        setView((v) =>
          v.scale > MIN_SCALE
            ? { ...v, scale: MIN_SCALE, x: 0, y: 0 }
            : { ...v, scale: 2, x: 0, y: 0 },
        );
      } else if (e.key === "i" || e.key === "I") {
        setExifOpen((open) => !open);
      } else if (e.key === "?") {
        showHint();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [asset.id, ctxAt, currentRating, group.assets.length, index, closeViewer, onNavigate, showHint, toggleFullscreen]);

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
    if (d.flash) shotRows.push({ label: t("viewer.flash"), value: formatToken(d.flash, FLASH_ZH) });
    if (d.meteringMode) {
      shotRows.push({ label: t("viewer.meteringMode"), value: formatToken(d.meteringMode, METERING_ZH) });
    }
    if (d.whiteBalance) {
      shotRows.push({ label: t("viewer.whiteBalance"), value: formatToken(d.whiteBalance, WB_ZH) });
    }
    if (d.exposureProgram) {
      shotRows.push({ label: t("viewer.exposureProgram"), value: formatToken(d.exposureProgram, PROGRAM_ZH) });
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
  }, [visibleDetail, t]);

  const hasPrev = index > 0;
  const hasNext = index < group.assets.length - 1;
  // 固定 17 个槽位，当前照片始终居中。边界处留空，不让缩略图条逐张伸缩。

  return (
    <div
      className="fixed inset-0 z-50 flex flex-col bg-black/95"
      role="dialog"
      aria-modal="true"
      aria-label={asset.name}
      data-testid="viewer"
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
            onDoubleClick={() => setView({ scale: 1, x: 0, y: 0, rotation: 0 })}
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
            {asset.kind === "video" && (
              <ViewerVideo path={asset.path} posterUrl={thumb.url} />
            )}
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
                data-fallback={
                  layer.src === originalUrl
                    ? "original"
                    : layer.src === rawEmbed.url && asset.kind === "raw"
                      ? "raw-embed"
                      : layer.src === rawFull.url && asset.kind === "raw"
                        ? "raw-full"
                        : layer.src === mid.url
                          ? "mid"
                          : "thumb"
                }
                className={`${
                  layer.phase === "loading" ? "invisible " : "absolute "
                }max-h-full max-w-full select-none object-contain`}
                style={{
                  // 变换顺序 translate→rotate→scale（origin=center）：图片自身中心先随平移
                  // 移动，旋转恒绕图片当前视觉中心（Windows 照片同款，平移后旋转不绕错轴）
                  transform: `translate(${view.x}px, ${view.y}px) rotate(${view.rotation}deg) scale(${view.scale})`,
                  transition: dragRef.current ? "none" : "transform 150ms ease-out",
                }}
              />
            ))}
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
                  className={asset.kind === "raw" ? "text-sky-400" : "text-violet-400"}
                  aria-hidden="true"
                >
                  {asset.kind === "video" ? (
                    <>
                      <rect x="3" y="5" width="18" height="14" rx="2" />
                      <path d="M10 9.5l5 2.5-5 2.5v-5z" />
                    </>
                  ) : (
                    <>
                      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
                      <circle cx="9" cy="10" r="1.8" />
                      <path d="M4.5 17l4.5-4.5 3.5 3.5 3-3 4 4" />
                    </>
                  )}
                </svg>
                <span className="text-xs">{t("viewer.noPreview")}</span>
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
            {exifOpen && <aside
              className="h-full w-72 shrink-0 overflow-hidden border-l border-edge bg-surface"
              data-testid="viewer-exif"
              data-open="true"
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
                      className={`rounded p-0.5 transition-colors ${
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
                  <button
                    type="button"
                    onClick={() => void applyFlagged(!currentFlagged)}
                    aria-pressed={currentFlagged}
                    title={currentFlagged ? t("viewer.flagClear") : t("viewer.flagSet")}
                    className={`ml-auto rounded px-1.5 py-0.5 text-[11px] ${
                      currentFlagged ? "bg-accent text-black" : "text-text-muted hover:text-text-primary"
                    }`}
                    data-testid="viewer-flag"
                  >
                    ⚑
                  </button>
                </div>
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
                            <dd className="min-w-0 text-right text-text-secondary">{row.value}</dd>
                          </div>
                        ))}
                      </dl>
                    </section>
                  ))}
                </div>
                </>
              </div>
            </aside>}
      </div>

      {/* 大图右键菜单（作用于当前资产；Esc/点击外部关闭，期间查看器 Esc 让位） */}
      {ctxAt !== null && (
        <AssetContextMenu at={ctxAt} assets={[asset]} onClose={() => setCtxAt(null)} />
      )}
    </div>
  );
}
